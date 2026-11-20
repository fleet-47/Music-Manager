//! Running a device plan (F10), in both directions.
//!
//! Every file is copied to a `*.part` name beside its destination and then renamed into place;
//! nothing is ever replaced. On a device (FAT32, where macOS refuses `persist_noclobber`) the place
//! is checked free and the part renamed; in the library the part lands with `persist_noclobber`,
//! which fails instead of replacing (F3).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use futures::StreamExt;
use sdl_core::place::device_music_dir;
use sdl_core::playlist::PLAYLIST_EXTENSION;
use sdl_core::{Attention, Counts, Direction, EventKind, EventSender, StopFlag};
use sdl_library::converter::{convert_file, parallelism};
use sdl_library::{LibraryError, playlists};

use crate::DevicesError;
use crate::paths::{existing_in_any_format, stays_inside};
use crate::plan::DevicePlan;

/// Run a plan and return its counts. The files it wrote, on every way out (finished, stopped,
/// full or gone), are [`DevicePlan::written`]: [`crate::clean_up_and_eject`] removes `._X` only
/// for an `X` listed there.
///
/// To device: each file is copied to a `*.part` name and renamed, or converted first with
/// `sdl_library::converter::convert_file` (`converter::parallelism()` at a time) when a format is
/// chosen and the file is not already in it; then each ticked playlist is written to
/// `<device>/Music/<List name>.m3u8` with `sdl_library::playlists::write_for_destination`. From
/// device: files are copied as they are to the place [`crate::place_incoming`] gave in the plan.
/// A destination that has appeared since the plan (in any format) is skipped, never replaced. Sends
/// [`sdl_core::EventKind::Counts`] after each file and an `Attention` for a failed one. `stop` is
/// checked before each file starts: the files in hand finish, then the run ends (without writing
/// playlists) and the device stays mounted. A write that fails because the device is full or gone
/// ends the run with [`DevicesError::Full`] or [`DevicesError::Gone`]; the files already complete
/// stay (their list is [`DevicePlan::written`]). `ffmpeg`
/// missing ends the run too. Never deletes a song on either side.
///
/// # Errors
/// The errors above.
pub async fn run(
    plan: &DevicePlan,
    events: &EventSender,
    stop: &StopFlag,
) -> Result<Counts, DevicesError> {
    let to_device = plan.choice.direction == Direction::ToDevice;
    let run = Run {
        plan,
        stop,
        halt: StopFlag::new(),
    };
    let mut counts = Counts {
        total: Some(plan.jobs.len()),
        ..Counts::default()
    };
    events.send(EventKind::Headline(run.headline()));
    events.send(EventKind::Counts(counts));

    // Copies go one at a time (one USB device); conversions as many as the Mac has cores.
    let width = if to_device && plan.choice.format.is_some() {
        parallelism().get()
    } else {
        1
    };
    plan.written
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let mut fatal = None;
    let mut results = futures::stream::iter(&plan.jobs)
        .map(|(src, dest)| run.file(src, dest))
        .buffer_unordered(width);
    while let Some(result) = results.next().await {
        match result {
            Ok(Outcome::NotStarted) => continue,
            Ok(Outcome::Written(dest)) => {
                counts.added = counts.added.saturating_add(1);
                plan.written
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(dest);
            }
            Ok(Outcome::Skipped) => counts.skipped = counts.skipped.saturating_add(1),
            Ok(Outcome::Failed(attention)) => {
                counts.failed = counts.failed.saturating_add(1);
                events.send(EventKind::Attention(attention));
            }
            Err(error) => {
                run.halt.request();
                fatal.get_or_insert(error);
                continue;
            }
        }
        counts.done = counts.done.saturating_add(1);
        events.send(EventKind::Counts(counts));
    }
    drop(results);
    if let Some(error) = fatal {
        return Err(error);
    }
    if to_device && !stop.is_requested() {
        events.send(EventKind::Headline("Writing playlists".to_owned()));
        run.write_playlists(events, &mut counts)?;
        events.send(EventKind::Counts(counts));
    }
    Ok(counts)
}

/// What became of one planned file.
enum Outcome {
    /// Not started because a stop was asked for or the run ended early.
    NotStarted,
    /// Complete at its destination.
    Written(PathBuf),
    /// Its place was taken before it could be written.
    Skipped,
    /// This file failed; the run goes on.
    Failed(Attention),
}

struct Run<'a> {
    plan: &'a DevicePlan,
    stop: &'a StopFlag,
    /// Set when the run must end early (full or gone device); never the caller's flag.
    halt: StopFlag,
}

impl Run<'_> {
    fn headline(&self) -> String {
        let name = &self.plan.device.name;
        match (self.plan.choice.direction, self.plan.choice.format) {
            (Direction::FromDevice, _) => format!("Copying from {name} into the library"),
            (Direction::ToDevice, None) => format!("Copying to {name}"),
            (Direction::ToDevice, Some(format)) => {
                format!("Converting to {} for {name}", format.label())
            }
        }
    }

    /// One file. `Err` ends the whole run.
    async fn file(&self, src: &Path, dest: &Path) -> Result<Outcome, DevicesError> {
        if self.stop.is_requested() || self.halt.is_requested() {
            return Ok(Outcome::NotStarted);
        }
        match existing_in_any_format(dest) {
            Ok(None) => {}
            Ok(Some(_)) => return Ok(Outcome::Skipped),
            Err(DevicesError::Io { source, .. }) => return self.failed(src, dest, &source),
            Err(other) => return Err(other),
        }
        let Some(folder) = dest.parent() else {
            return Ok(Outcome::NotStarted);
        };
        if let Err(source) = tokio::fs::create_dir_all(folder).await {
            return self.failed(src, dest, &source);
        }
        let preset = self.plan.choice.preset();
        let to_device = self.plan.choice.direction == Direction::ToDevice;
        match preset {
            Some(preset) if to_device && !preset.format.holds(src) => {
                match convert_file(src, dest, preset).await {
                    Ok(()) => Ok(Outcome::Written(dest.to_path_buf())),
                    Err(LibraryError::FfmpegNotFound) => {
                        Err(DevicesError::Library(LibraryError::FfmpegNotFound))
                    }
                    Err(LibraryError::Io { source, .. }) => self.failed(src, dest, &source),
                    Err(LibraryError::Ffmpeg { message, .. })
                        if message.contains("No space left on device") =>
                    {
                        Err(self.full())
                    }
                    Err(other) => self.failed(src, dest, &std::io::Error::other(other.to_string())),
                }
            }
            _ => {
                let landing = if to_device {
                    Landing::RenameIfFree
                } else {
                    Landing::NoClobber
                };
                let (from, to) = (src.to_path_buf(), dest.to_path_buf());
                let copied = tokio::task::spawn_blocking(move || copy_in(&from, &to, landing))
                    .await
                    .map_err(|error| {
                        DevicesError::Library(LibraryError::Task(error.to_string()))
                    })?;
                match copied {
                    Ok(Landed::Written) => Ok(Outcome::Written(dest.to_path_buf())),
                    Ok(Landed::Existed) => Ok(Outcome::Skipped),
                    Err(source) => self.failed(src, dest, &source),
                }
            }
        }
    }

    /// A file failed with `source`: the run ends when the device is gone or full, otherwise it is
    /// that file's problem and the run goes on.
    fn failed(
        &self,
        src: &Path,
        dest: &Path,
        source: &std::io::Error,
    ) -> Result<Outcome, DevicesError> {
        let to_device = self.plan.choice.direction == Direction::ToDevice;
        if !self.plan.device.mount_point.exists() {
            return Err(self.gone());
        }
        if matches!(
            source.kind(),
            ErrorKind::StorageFull | ErrorKind::QuotaExceeded
        ) {
            return Err(if to_device {
                self.full()
            } else {
                DevicesError::Io {
                    path: dest.to_path_buf(),
                    source: std::io::Error::new(source.kind(), "the library's disk is full"),
                }
            });
        }
        let what = if to_device {
            "could not be written to the device"
        } else {
            "could not be copied into the library"
        };
        let title = src
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let artist = if to_device {
            src.parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Ok(Outcome::Failed(Attention::failed(
            title,
            artist,
            format!("The file {what}: {source}."),
        )))
    }

    fn full(&self) -> DevicesError {
        DevicesError::Full {
            device: self.plan.device.name.clone(),
        }
    }

    fn gone(&self) -> DevicesError {
        DevicesError::Gone {
            device: self.plan.device.name.clone(),
        }
    }

    /// Write each ticked playlist to `<device>/Music/<name>.m3u8`, listing only what is there.
    fn write_playlists(
        &self,
        events: &EventSender,
        counts: &mut Counts,
    ) -> Result<(), DevicesError> {
        let music_dir = device_music_dir(&self.plan.device.mount_point);
        for stem in &self.plan.choice.what.playlists {
            if !stays_inside(Path::new(stem)) {
                continue;
            }
            match playlists::write_for_destination(&self.plan.music_root, &music_dir, stem) {
                Ok(_) => {
                    let file = music_dir.join(format!("{stem}.{PLAYLIST_EXTENSION}"));
                    if file.try_exists().unwrap_or(false) {
                        self.plan
                            .written
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(file);
                    }
                }
                Err(LibraryError::Io { path, source }) => {
                    if !self.plan.device.mount_point.exists() {
                        return Err(self.gone());
                    }
                    if matches!(
                        source.kind(),
                        ErrorKind::StorageFull | ErrorKind::QuotaExceeded
                    ) {
                        return Err(self.full());
                    }
                    counts.failed = counts.failed.saturating_add(1);
                    events.send(EventKind::Attention(Attention::failed(
                        stem.clone(),
                        String::new(),
                        format!(
                            "The playlist could not be written ({}): {source}.",
                            path.display()
                        ),
                    )));
                }
                Err(other) => return Err(DevicesError::Library(other)),
            }
        }
        Ok(())
    }
}

/// How a finished part is put in its final place.
#[derive(Clone, Copy)]
enum Landing {
    /// Check the place is free, then rename (a device, where `persist_noclobber` fails on FAT32
    /// with os error 45).
    RenameIfFree,
    /// `tempfile`'s `persist_noclobber`, which fails instead of replacing (the library, F3).
    NoClobber,
}

enum Landed {
    Written,
    /// The place was taken first; nothing was replaced.
    Existed,
}

/// Copy `src` to a new `<name><random>.part` file beside `dest` (`tempfile`: a fresh name, so nothing
/// that is there is ever truncated; data only, no extended attributes), then put it in place.
/// The part is removed on every way out: landed, place taken, or failed.
fn copy_in(src: &Path, dest: &Path, landing: Landing) -> std::io::Result<Landed> {
    let (Some(folder), Some(name)) = (dest.parent(), dest.file_name()) else {
        return Err(std::io::Error::other("not a file path"));
    };
    let mut part = tempfile::Builder::new()
        .prefix(name)
        .suffix(".part")
        .tempfile_in(folder)?;
    std::io::copy(&mut std::fs::File::open(src)?, part.as_file_mut())?;
    let part = part.into_temp_path();
    match landing {
        Landing::RenameIfFree => {
            if dest.try_exists()? {
                return Ok(Landed::Existed);
            }
            part.persist(dest).map_err(|failure| failure.error)?;
            Ok(Landed::Written)
        }
        Landing::NoClobber => match part.persist_noclobber(dest) {
            Ok(()) => Ok(Landed::Written),
            Err(failure) if failure.error.kind() == ErrorKind::AlreadyExists => Ok(Landed::Existed),
            Err(failure) => Err(failure.error),
        },
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{Device, DeviceChoice, Selection, TaskId, channel};

    use super::*;
    use crate::plan::plan;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn touch(path: &Path, content: &[u8]) -> TestResult {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    fn device(mount: &Path) -> Device {
        Device {
            name: "TESTDEV".into(),
            mount_point: mount.to_path_buf(),
            volume_uuid: None,
            free_bytes: u64::MAX,
        }
    }

    fn choice(playlists: &[&str]) -> DeviceChoice {
        DeviceChoice {
            what: Selection {
                whole_library: true,
                playlists: playlists.iter().map(|name| (*name).to_owned()).collect(),
            },
            ..DeviceChoice::default()
        }
    }

    fn sender() -> (EventSender, sdl_core::EventReceiver) {
        let (tx, rx) = channel();
        (EventSender::new(TaskId::SyncDevice, tx), rx)
    }

    #[tokio::test]
    async fn to_device_copies_whole_files_skips_what_is_there_and_writes_playlists_last()
    -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), b"one")?;
        touch(&root.path().join("A/Al/two.opus"), b"two")?;
        touch(&root.path().join("A/Al/Mr. Three.opus"), b"three")?;
        touch(&root.path().join("A/Al/._junk.opus"), b"junk")?;
        std::fs::write(
            root.path().join("Mix.m3u8"),
            "#EXTM3U\n#PLAYLIST:Mix\nA/Al/one.opus\nA/Al/two.opus\n",
        )?;
        let device = device(dev.path());
        let plan = plan(root.path(), &device, &choice(&["Mix"])).await?;
        // Taken between the plan and the run, in another format: never replaced.
        touch(&dev.path().join("Music/A/Al/two.m4a"), b"theirs")?;
        let (events, mut rx) = sender();
        let report = run(&plan, &events, &StopFlag::new()).await?;
        let music = dev.path().join("Music");
        assert_eq!(std::fs::read(music.join("A/Al/one.opus"))?, b"one");
        assert_eq!(std::fs::read(music.join("A/Al/Mr. Three.opus"))?, b"three");
        assert!(!music.join("A/Al/two.opus").exists());
        assert_eq!(std::fs::read(music.join("A/Al/two.m4a"))?, b"theirs");
        assert!(!music.join("A/Al/._junk.opus").exists());
        let leftovers: Vec<_> = walk_names(&music)
            .into_iter()
            .filter(|name| is_part(name))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert_eq!((report.total, report.done, report.added), (Some(3), 3, 2));
        assert_eq!((report.skipped, report.failed), (1, 0));
        assert_eq!(plan.written().len(), 3, "two songs and the playlist");
        assert_eq!(
            std::fs::read_to_string(music.join("Mix.m3u8"))?,
            "#EXTM3U\n#PLAYLIST:Mix\nA/Al/one.opus\nA/Al/two.m4a\n"
        );
        let mut last = None;
        while let Ok(event) = rx.try_recv() {
            last = Some(event.kind);
        }
        assert!(matches!(last, Some(EventKind::Counts(counts)) if counts.done == 3));
        Ok(())
    }

    fn is_part(name: &str) -> bool {
        Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("part"))
    }

    fn walk_names(dir: &Path) -> Vec<String> {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_stop_finishes_the_file_in_hand_and_leaves_complete_files() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        for index in 0..40 {
            touch(
                &root.path().join(format!("A/Al/song{index:02}.opus")),
                b"data",
            )?;
        }
        let plan = plan(root.path(), &device(dev.path()), &choice(&[])).await?;
        let (events, mut rx) = sender();
        let stop = StopFlag::new();
        let asker = stop.clone();
        let watcher = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if matches!(event.kind, EventKind::Counts(counts) if counts.done >= 1) {
                    asker.request();
                    break;
                }
            }
        });
        let report = run(&plan, &events, &stop).await?;
        watcher.await?;
        assert!(report.added >= 1 && report.added < 40, "{report:?}");
        assert_eq!(plan.written().len(), report.added);
        for file in &plan.written() {
            assert_eq!(std::fs::read(file)?, b"data");
        }
        assert!(
            walk_names(&dev.path().join("Music"))
                .iter()
                .all(|name| !is_part(name))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_device_that_is_gone_ends_the_run_in_plain_words() -> TestResult {
        let root = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), b"one")?;
        let gone = device(Path::new("/this-volume-is-not-mounted/DEV"));
        let plan = plan(root.path(), &gone, &choice(&[])).await?;
        let (events, _rx) = sender();
        let error = run(&plan, &events, &StopFlag::new()).await.err();
        assert!(
            matches!(error, Some(DevicesError::Gone { ref device }) if device == "TESTDEV"),
            "{error:?}"
        );
        assert_eq!(
            error.map(|e| e.to_string()).as_deref(),
            Some("TESTDEV was removed during the copy")
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_read_fails_alone_and_is_listed() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), b"one")?;
        touch(&root.path().join("A/Al/two.opus"), b"two")?;
        let plan = plan(root.path(), &device(dev.path()), &choice(&[])).await?;
        std::fs::remove_file(root.path().join("A/Al/one.opus"))?;
        let (events, mut rx) = sender();
        let report = run(&plan, &events, &StopFlag::new()).await?;
        assert_eq!((report.added, report.failed), (1, 1));
        let mut reason = None;
        while let Ok(event) = rx.try_recv() {
            if let EventKind::Attention(attention) = event.kind {
                reason = Some((attention.title, attention.reason));
            }
        }
        let (title, reason) = reason.ok_or("no attention item")?;
        assert_eq!(title, "one");
        assert!(
            reason.starts_with("The file could not be written to the device"),
            "{reason}"
        );
        Ok(())
    }

    #[test]
    fn a_taken_place_is_never_replaced_in_either_landing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src.opus");
        std::fs::write(&src, b"new")?;
        let only_src =
            || -> Result<bool, std::io::Error> { Ok(std::fs::read_dir(dir.path())?.count() == 1) };
        for landing in [Landing::RenameIfFree, Landing::NoClobber] {
            let dest = dir.path().join("Mr. Song.opus");
            std::fs::write(&dest, b"old")?;
            assert!(matches!(copy_in(&src, &dest, landing)?, Landed::Existed));
            assert_eq!(std::fs::read(&dest)?, b"old");
            std::fs::remove_file(&dest)?;
            assert!(only_src()?, "no part left behind");
            assert!(matches!(copy_in(&src, &dest, landing)?, Landed::Written));
            assert_eq!(std::fs::read(&dest)?, b"new");
            std::fs::remove_file(&dest)?;
            assert!(only_src()?, "no part left behind");
        }
        Ok(())
    }

    #[tokio::test]
    async fn from_device_copies_into_the_library_and_a_second_run_changes_nothing() -> TestResult {
        use crate::testing::mp3;
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        let tagged = mp3(Some("Song One"), Some("Band"), Some("Rec"));
        let untagged = mp3(None, None, None);
        touch(&dev.path().join("Old/tagged.mp3"), &tagged)?;
        touch(&dev.path().join("Old/sub/untagged.mp3"), &untagged)?;
        touch(&dev.path().join("Old/.hidden.mp3"), &tagged)?;
        let choice = DeviceChoice {
            direction: Direction::FromDevice,
            from_folders: vec!["Old".into()],
            ..DeviceChoice::default()
        };
        let device = device(dev.path());
        let first = plan(root.path(), &device, &choice).await?;
        let (events, _rx) = sender();
        let report = run(&first, &events, &StopFlag::new()).await?;
        assert_eq!((report.added, report.failed), (2, 0));
        assert_eq!(
            std::fs::read(root.path().join("Band/Rec/Song One.mp3"))?,
            tagged
        );
        assert_eq!(
            std::fs::read(root.path().join("Unsorted/Old/sub/untagged.mp3"))?,
            untagged
        );
        assert!(walk_names(root.path()).iter().all(|name| !is_part(name)));
        // The same plan again: the places are taken now, so nothing is written or replaced.
        let again = run(&first, &events, &StopFlag::new()).await?;
        assert_eq!((again.added, again.skipped), (0, 2));
        let fresh = plan(root.path(), &device, &choice).await?;
        assert_eq!((fresh.summary().files, fresh.summary().already), (0, 2));
        // Nothing on the device was touched.
        assert_eq!(std::fs::read(dev.path().join("Old/tagged.mp3"))?, tagged);
        Ok(())
    }
}
