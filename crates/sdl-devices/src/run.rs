//! Running a device plan (F10), in both directions.
//!
//! Every file is copied to a `*.part` name beside its destination and then renamed into place;
//! nothing is ever replaced (`sdl_core::landing`). The files are driven by
//! `sdl_core::run::Run`.

use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use sdl_core::landing::{self, Landed, Landing};
use sdl_core::place::device_music_dir;
use sdl_core::playlist::PLAYLIST_EXTENSION;
use sdl_core::run::{CancellationToken, Outcome, Run, Tally};
use sdl_core::{Attention, Counts, Direction, EventKind, EventSender};
use sdl_library::converter::{Converted, convert_file, parallelism};
use sdl_library::{LibraryError, playlists};
use tokio::sync::RwLock;

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
/// device: files are copied as they are to the place the plan gave each (its tags, else `Unsorted`).
/// A destination that has appeared since the plan (in any format) is skipped, never replaced. Sends
/// the headline, then [`sdl_core::EventKind::Counts`] after each file and an `Attention` for a
/// failed one. Once `stop` is cancelled no further file starts: the files in hand finish, then the
/// run ends (without writing playlists) and the device stays mounted. A write that fails because
/// the device is full or gone ends the run with [`DevicesError::Full`] or [`DevicesError::Gone`];
/// the files already complete stay (their list is [`DevicePlan::written`]). `ffmpeg` missing ends
/// the run too. Never deletes a song on either side.
///
/// # Errors
/// The errors above.
pub async fn run(
    plan: &DevicePlan,
    events: &EventSender,
    stop: &CancellationToken,
) -> Result<Counts, DevicesError> {
    let to_device = plan.choice.direction == Direction::ToDevice;
    let job = Job::new(plan);
    events.send(EventKind::Headline(job.headline()));

    // Copies go one at a time (one USB device); conversions as many as the Mac has cores.
    let limit = if to_device && plan.choice.format.is_some() {
        parallelism()
    } else {
        NonZeroUsize::MIN
    };
    plan.written
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    // A plain loop, not a closure over `&plan.jobs`: such a closure makes the future unprovable as
    // `Send` for every lifetime (rustc), and the task layer spawns it.
    let mut jobs = Vec::with_capacity(plan.jobs.len());
    for (src, dest) in &plan.jobs {
        jobs.push(job.file(src, dest));
    }
    let tally = job.drive(limit, events, stop, jobs).await?;
    let mut counts = tally.counts();
    if to_device && !stop.is_cancelled() {
        events.send(EventKind::Headline("Writing playlists".to_owned()));
        job.write_playlists(events, &mut counts)?;
        events.send(EventKind::Counts(counts));
    }
    Ok(counts)
}

struct Job<'a> {
    plan: &'a DevicePlan,
    /// Held (shared) by every copy on a blocking thread until it has ended, so the run can wait
    /// for them: dropping a job does not stop its blocking thread.
    copies: Arc<RwLock<()>>,
}

impl<'a> Job<'a> {
    fn new(plan: &'a DevicePlan) -> Self {
        Self {
            plan,
            copies: Arc::default(),
        }
    }
}

impl Job<'_> {
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

    /// Drive `jobs` (`sdl_core::run::Run`), then wait for every copy still on a blocking thread
    /// (a job's error drops the jobs in hand, not their threads), so nothing writes once this
    /// returns and every file that landed is in [`DevicePlan::written`].
    async fn drive<F>(
        &self,
        limit: NonZeroUsize,
        events: &EventSender,
        stop: &CancellationToken,
        jobs: Vec<F>,
    ) -> Result<Tally, DevicesError>
    where
        F: Future<Output = Result<Outcome, DevicesError>>,
    {
        let driven = Run {
            limit,
            events,
            stop,
            settled: Vec::new(),
        }
        .drive(jobs)
        .await;
        drop(self.copies.write().await);
        driven
    }

    /// Note `paths` as written by this run.
    fn record(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.plan
            .written
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(paths);
    }

    /// One file. `Err` ends the whole run.
    async fn file(&self, src: &Path, dest: &Path) -> Result<Outcome, DevicesError> {
        match existing_in_any_format(dest) {
            Ok(None) => {}
            Ok(Some(_)) => return Ok(Outcome::Skipped),
            Err(DevicesError::Io { source, .. }) => return self.failed(src, dest, &source),
            Err(other) => return Err(other),
        }
        let Some(folder) = dest.parent() else {
            return Ok(Outcome::Skipped);
        };
        let made = missing_folders(folder, &self.plan.device.mount_point);
        // In this thread, not `tokio::fs`: a dropped job must leave nothing making folders.
        if let Err(source) = std::fs::create_dir_all(folder) {
            return self.failed(src, dest, &source);
        }
        // Folders this run made are its own too: clean-up may remove the `._` file macOS leaves
        // beside each (F10).
        self.record(made);
        let preset = self.plan.choice.preset();
        let to_device = self.plan.choice.direction == Direction::ToDevice;
        match preset {
            Some(preset) if to_device && !preset.format.holds(src) => {
                let converted = convert_file(src, dest, preset).await;
                self.converted(src, dest, converted)
            }
            _ => {
                let landing = if to_device {
                    Landing::Destination
                } else {
                    Landing::Library
                };
                let (from, to) = (src.to_path_buf(), dest.to_path_buf());
                let (copying, written) = (
                    Arc::clone(&self.copies).read_owned().await,
                    Arc::clone(&self.plan.written),
                );
                // Recorded on the thread itself: it lands even when this job is dropped.
                let copied = tokio::task::spawn_blocking(move || {
                    let landed = copy_in(&from, &to, landing);
                    if matches!(landed, Ok(Landed::Written)) {
                        written
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(to);
                    }
                    drop(copying);
                    landed
                })
                .await
                .map_err(|error| DevicesError::Library(LibraryError::Task(error.to_string())))?;
                match copied {
                    Ok(Landed::Written) => Ok(Outcome::Added),
                    Ok(Landed::Existed) => Ok(Outcome::Skipped),
                    Err(source) => self.failed(src, dest, &source),
                }
            }
        }
    }

    /// What a converter's answer for `src` -> `dest` makes of the file.
    fn converted(
        &self,
        src: &Path,
        dest: &Path,
        result: Result<Converted, LibraryError>,
    ) -> Result<Outcome, DevicesError> {
        match result {
            Ok(Converted::Written) => {
                self.record([dest.to_path_buf()]);
                Ok(Outcome::Added)
            }
            Ok(Converted::Existed) => Ok(Outcome::Skipped),
            Err(LibraryError::Full { .. }) => Err(self.full()),
            Err(LibraryError::FfmpegNotFound) => {
                Err(DevicesError::Library(LibraryError::FfmpegNotFound))
            }
            Err(LibraryError::Io { source, .. }) => self.failed(src, dest, &source),
            Err(other) => self.failed(src, dest, &std::io::Error::other(other.to_string())),
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
        // The artist folder of an `<artist>/<album>/<file>` place; a file at another depth (F7)
        // has none.
        let artist = match src.strip_prefix(self.plan.library.root()) {
            Ok(relative) if to_device && relative.components().count() == 3 => relative
                .components()
                .next()
                .map(|name| name.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default(),
            _ => String::new(),
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
            match playlists::write_for_destination(&self.plan.library, &music_dir, stem) {
                Ok(_) => {
                    let file = music_dir.join(format!("{stem}.{PLAYLIST_EXTENSION}"));
                    if file.try_exists().unwrap_or(false) {
                        self.record([file]);
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

/// The folders from `folder` up to (not including) `root` that do not exist yet, so a run can tell
/// which folders it made. Empty outside `root`.
fn missing_folders(folder: &Path, root: &Path) -> Vec<PathBuf> {
    folder
        .ancestors()
        .take_while(|dir| *dir != root && dir.starts_with(root))
        .take_while(|dir| !dir.exists())
        .map(Path::to_path_buf)
        .collect()
}

/// Copy `src` to a new `.part` file beside `dest` (data only, no extended attributes), then put it
/// in place. The part is removed on every way out: landed, place taken, or failed.
fn copy_in(src: &Path, dest: &Path, landing: Landing) -> std::io::Result<Landed> {
    let mut part = landing::part_beside(dest)?;
    std::io::copy(&mut std::fs::File::open(src)?, part.as_file_mut())?;
    landing::land(part.into_temp_path(), dest, landing)
}

#[cfg(test)]
mod tests {
    use sdl_core::{Device, DeviceChoice, Selection, TaskId, channel};

    use super::*;
    use crate::plan::plan;
    use crate::testing::lib;

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
            disk_image: false,
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
        let plan = plan(&lib(root.path()), &device, &choice(&["Mix"])).await?;
        // Taken between the plan and the run, in another format: never replaced.
        touch(&dev.path().join("Music/A/Al/two.m4a"), b"theirs")?;
        let (events, mut rx) = sender();
        let report = run(&plan, &events, &CancellationToken::new()).await?;
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
        let mut written = plan.written();
        written.sort();
        // The folders were there already (made for `two.m4a` above), so only files are listed.
        let expected = ["A/Al/Mr. Three.opus", "A/Al/one.opus", "Mix.m3u8"]
            .map(|relative| music.join(relative));
        assert_eq!(written, expected, "two songs and the playlist");
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
        let plan = plan(&lib(root.path()), &device(dev.path()), &choice(&[])).await?;
        let (events, mut rx) = sender();
        let stop = CancellationToken::new();
        let asker = stop.clone();
        let watcher = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if matches!(event.kind, EventKind::Counts(counts) if counts.done >= 1) {
                    asker.cancel();
                    break;
                }
            }
        });
        let report = run(&plan, &events, &stop).await?;
        watcher.await?;
        assert!(report.added >= 1 && report.added < 40, "{report:?}");
        let music = dev.path().join("Music");
        let made: Vec<PathBuf> = plan.written().into_iter().filter(|p| p.is_dir()).collect();
        assert_eq!(made.len(), 3, "the folders this run made: {made:?}");
        assert!(made.contains(&music) && made.contains(&music.join("A/Al")));
        let files: Vec<PathBuf> = plan.written().into_iter().filter(|p| p.is_file()).collect();
        assert_eq!(files.len(), report.added);
        for file in &files {
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
        let plan = plan(&lib(root.path()), &gone, &choice(&[])).await?;
        let (events, _rx) = sender();
        let error = run(&plan, &events, &CancellationToken::new()).await.err();
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
        let plan = plan(&lib(root.path()), &device(dev.path()), &choice(&[])).await?;
        std::fs::remove_file(root.path().join("A/Al/one.opus"))?;
        let (events, mut rx) = sender();
        let report = run(&plan, &events, &CancellationToken::new()).await?;
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
        for landing in [Landing::Destination, Landing::Library] {
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
    async fn a_converted_place_taken_mid_run_is_skipped_and_a_full_device_ends_the_run()
    -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), b"one")?;
        let plan = plan(&lib(root.path()), &device(dev.path()), &choice(&[])).await?;
        let job = Job::new(&plan);
        let (src, dest) = (
            root.path().join("A/Al/one.opus"),
            dev.path().join("one.mp3"),
        );
        let skipped = job.converted(&src, &dest, Ok(Converted::Existed))?;
        assert_eq!(skipped, Outcome::Skipped);
        let full = job.converted(&src, &dest, Err(LibraryError::Full { path: dest.clone() }));
        assert!(
            matches!(full, Err(DevicesError::Full { ref device }) if device == "TESTDEV"),
            "{full:?}"
        );
        assert_eq!(plan.written(), Vec::<PathBuf>::new());
        Ok(())
    }

    #[tokio::test]
    async fn a_copy_in_hand_when_the_run_ends_on_an_error_lands_before_it_returns_and_is_recorded()
    -> TestResult {
        use futures::future::Either;
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        // Already MP3, so with MP3 chosen it is copied as it is, on a blocking thread.
        let big = vec![7_u8; 67_108_864];
        touch(&root.path().join("A/Al/big.mp3"), &big)?;
        let choice = DeviceChoice {
            format: Some(sdl_core::Format::Mp3),
            ..choice(&[])
        };
        let plan = plan(&lib(root.path()), &device(dev.path()), &choice).await?;
        let (src, dest) = plan.jobs.first().cloned().ok_or("no job")?;
        let folder = dest.parent().ok_or("no folder")?.to_path_buf();
        let job = Job::new(&plan);
        // A conversion that finds the device full, once the copy is under way.
        let full = async {
            while !dest.exists() && walk_names(&folder).iter().all(|name| !is_part(name)) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            Err(DevicesError::Full {
                device: "TESTDEV".into(),
            })
        };
        let jobs = vec![Either::Left(job.file(&src, &dest)), Either::Right(full)];
        let (events, _rx) = sender();
        let limit = NonZeroUsize::new(2).ok_or("zero")?;
        let ended = job
            .drive(limit, &events, &CancellationToken::new(), jobs)
            .await;
        assert!(matches!(ended, Err(DevicesError::Full { .. })), "{ended:?}");
        let parts: Vec<_> = walk_names(&folder)
            .into_iter()
            .filter(|name| is_part(name))
            .collect();
        assert!(
            parts.is_empty(),
            "still writing after the run returned: {parts:?}"
        );
        assert_eq!(std::fs::read(&dest)?.len(), big.len());
        assert!(plan.written().contains(&dest), "{:?}", plan.written());
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_file_names_an_artist_only_from_an_artist_album_file_place() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        let plan = plan(&lib(root.path()), &device(dev.path()), &choice(&[])).await?;
        let job = Job::new(&plan);
        let why = std::io::Error::other("no");
        let artist = |relative: &str| -> Result<String, Box<dyn std::error::Error>> {
            match job.failed(&root.path().join(relative), dev.path(), &why)? {
                Outcome::Failed(item) => Ok(item.artist),
                other => Err(format!("{other:?}").into()),
            }
        };
        assert_eq!(artist("A/Al/one.opus")?, "A");
        assert_eq!(artist("Unsorted/Old/sub/two.mp3")?, "");
        assert_eq!(artist("loose.opus")?, "");
        Ok(())
    }

    #[tokio::test]
    async fn a_song_at_another_depth_goes_to_the_same_path_on_the_device_and_back() -> TestResult {
        use crate::testing::mp3;
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        let untagged = mp3(None, None, None);
        touch(&root.path().join("Unsorted/Old/sub/x.mp3"), &untagged)?;
        touch(&root.path().join("A/Al/one.opus"), b"one")?;
        let device = device(dev.path());
        let to = plan(&lib(root.path()), &device, &choice(&[])).await?;
        let (events, _rx) = sender();
        let report = run(&to, &events, &CancellationToken::new()).await?;
        assert_eq!((report.added, report.failed), (2, 0));
        let music = dev.path().join("Music");
        assert_eq!(
            std::fs::read(music.join("Unsorted/Old/sub/x.mp3"))?,
            untagged
        );
        let again = plan(&lib(root.path()), &device, &choice(&[])).await?;
        assert_eq!((again.summary().files, again.summary().already), (0, 2));
        // Back from the device: the untagged file's place is `Unsorted/Music/...`, a new place.
        let from = DeviceChoice {
            direction: Direction::FromDevice,
            from_folders: vec!["Music".into()],
            ..DeviceChoice::default()
        };
        let back = plan(&lib(root.path()), &device, &from).await?;
        let dests: Vec<_> = back.jobs.iter().map(|(_, dest)| dest.clone()).collect();
        assert!(
            dests.contains(&root.path().join("Unsorted/Music/Unsorted/Old/sub/x.mp3")),
            "{dests:?}"
        );
        run(&back, &events, &CancellationToken::new()).await?;
        let twice = plan(&lib(root.path()), &device, &from).await?;
        assert_eq!(
            twice.summary().files,
            0,
            "a second From device run copies nothing"
        );
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
        let first = plan(&lib(root.path()), &device, &choice).await?;
        let (events, _rx) = sender();
        let report = run(&first, &events, &CancellationToken::new()).await?;
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
        let again = run(&first, &events, &CancellationToken::new()).await?;
        assert_eq!((again.added, again.skipped), (0, 2));
        let fresh = plan(&lib(root.path()), &device, &choice).await?;
        assert_eq!((fresh.summary().files, fresh.summary().already), (0, 2));
        // Nothing on the device was touched.
        assert_eq!(std::fs::read(dev.path().join("Old/tagged.mp3"))?, tagged);
        Ok(())
    }
}
