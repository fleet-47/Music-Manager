//! The Convert function (F9): converted copies of the library (or ticked playlists) written to a
//! separate destination, with the library's layout. Never changes or deletes the library.

use std::path::{Path, PathBuf};

use sdl_core::format::existing_format;
use sdl_core::run::{CancellationToken, Outcome, Run};
use sdl_core::{
    Attention, ConvertChoice, Counts, EventSender, Format, Library, PlanSummary, Preset,
};

use crate::converter::{Converted, convert_file, output_size, parallelism};
use crate::playlists::write_for_destination;
use crate::selection;
use crate::{LibraryError, io_err};

/// What a Convert run would do: the files to convert and the summary shown before Start.
#[derive(Debug, Clone)]
pub struct ConvertPlan {
    summary: PlanSummary,
    library: Library,
    destination: PathBuf,
    preset: Preset,
    /// The ticked playlists' file stems.
    playlists: Vec<String>,
    /// `(source in the library, destination file)`.
    jobs: Vec<(PathBuf, PathBuf)>,
}

impl ConvertPlan {
    /// The summary line's numbers: `N files to convert · M already there`. A destination inside
    /// `music_root` is a plan with a `refusal` in plain words (not an error), so the form stays
    /// open; such a plan has no files and cannot be run.
    #[must_use]
    pub const fn summary(&self) -> &PlanSummary {
        &self.summary
    }
}

/// Plan a Convert from `library` (F12): the files of the ticked selection (the whole library via
/// [`sdl_core::library_files`], and each ticked playlist's songs that exist), each mapped to
/// `<destination>/<same relative path>` with the preset's extension. Files whose stem is already
/// there in any format count as "already there". Sizes are estimated from each file's duration
/// and [`sdl_core::Preset::nominal_kbps`] (`estimated` is set), as
/// [`crate::converter::output_size`] gives them. `free_bytes` is `None`:
/// the destination's free space is not asked for.
///
/// The scan runs on a blocking thread.
///
/// # Errors
/// [`LibraryError::Io`] when a library folder or a file cannot be read; [`LibraryError::Task`]
/// when the scan thread fails.
pub async fn plan(library: &Library, choice: &ConvertChoice) -> Result<ConvertPlan, LibraryError> {
    let (library, choice) = (library.clone(), choice.clone());
    tokio::task::spawn_blocking(move || plan_now(&library, &choice))
        .await
        .map_err(|error| LibraryError::Task(error.to_string()))?
}

fn plan_now(library: &Library, choice: &ConvertChoice) -> Result<ConvertPlan, LibraryError> {
    let preset = choice.preset();
    let mut plan = ConvertPlan {
        summary: PlanSummary {
            files: 0,
            bytes: 0,
            estimated: false,
            already: 0,
            free_bytes: None,
            refusal: None,
        },
        library: library.clone(),
        destination: choice.destination.clone(),
        preset,
        playlists: choice.what.playlists.clone(),
        jobs: Vec::new(),
    };
    let Some(destination) = choice.destination_path() else {
        plan.summary.refusal = Some(
            "The destination starts with ~ but your home folder could not be found. \
             Type the full path of the folder."
                .to_owned(),
        );
        return Ok(plan);
    };
    plan.destination.clone_from(&destination);
    let music_root = library.music_root();
    if resolve(&destination).starts_with(resolve(music_root)) {
        plan.summary.refusal = Some(format!(
            "The destination is inside your music library ({}). Convert never writes into the \
             library: choose another folder.",
            music_root.display()
        ));
        return Ok(plan);
    }
    for selection::Song {
        file: source,
        relative,
    } in selection::songs(library, &choice.what)?
    {
        if existing_in(&destination, &relative)?.is_some() {
            plan.summary.already = plan.summary.already.saturating_add(1);
            continue;
        }
        let size = output_size(&source, Some(preset))?;
        plan.summary.bytes = plan.summary.bytes.saturating_add(size.bytes);
        plan.summary.estimated |= size.estimated;
        plan.summary.files = plan.summary.files.saturating_add(1);
        let target = destination.join(relative.with_extension(preset.format.extension()));
        plan.jobs.push((source, target));
    }
    // Nothing to write: no file, and no playlist to write either.
    if plan.jobs.is_empty() && plan.playlists.is_empty() {
        plan.summary.refusal = Some(if plan.summary.already > 0 {
            "Nothing to convert: every chosen song is already in the destination.".to_owned()
        } else {
            "Nothing to convert: what is ticked holds no songs. Tick something else.".to_owned()
        });
    }
    Ok(plan)
}

/// The format of the file with `relative`'s name under `root` in any format, if there is one.
fn existing_in(root: &Path, relative: &Path) -> Result<Option<Format>, LibraryError> {
    let path = root.join(relative);
    existing_format(&path).map_err(io_err(&path))
}

/// `path` with symbolic links and `..` resolved as far as it exists, the missing rest appended:
/// a destination that is not made yet still compares by where it will really be.
fn resolve(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut base = path;
    loop {
        if let Ok(real) = base.canonicalize() {
            return missing.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                base = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Run a plan: convert each file with [`crate::converter::convert_file`],
/// [`crate::converter::parallelism`] at a time, through [`sdl_core::run::Run`], which sends the
/// counts and an `Attention` for each failed file. Then write each ticked playlist to the
/// destination with [`crate::playlists::write_for_destination`]. Once `stop` is cancelled no new
/// file starts: files in hand finish, and the run returns the counts so far without writing
/// playlists. A file that appeared at its place meanwhile counts as skipped.
///
/// # Errors
/// A failure that ends the whole run: `ffmpeg` not found, the destination not writable or full,
/// a refused plan. A single file that fails is counted and listed, not an error.
pub async fn run(
    plan: &ConvertPlan,
    events: &EventSender,
    stop: &CancellationToken,
) -> Result<Counts, LibraryError> {
    if let Some(refusal) = &plan.summary.refusal {
        return Err(LibraryError::Task(refusal.clone()));
    }
    std::fs::create_dir_all(&plan.destination).map_err(io_err(&plan.destination))?;
    // A plain loop, not a closure over `&plan.jobs`: such a closure makes the future unprovable as
    // `Send` for every lifetime (rustc), and the task layer spawns it.
    let mut jobs = Vec::with_capacity(plan.jobs.len());
    for (source, target) in &plan.jobs {
        jobs.push(convert_one(
            source,
            target,
            plan.preset,
            plan.library.root(),
        ));
    }
    let tally = Run {
        limit: parallelism(),
        events,
        stop,
        settled: Vec::new(),
    }
    .drive(jobs)
    .await?;
    if !stop.is_cancelled() {
        for stem in &plan.playlists {
            write_for_destination(&plan.library, &plan.destination, stem)?;
        }
    }
    Ok(tally.counts())
}

/// One planned file, ended as an outcome.
async fn convert_one(
    source: &Path,
    target: &Path,
    preset: Preset,
    library_root: &Path,
) -> Result<Outcome, LibraryError> {
    outcome_of(
        convert_file(source, target, preset).await,
        source,
        library_root,
    )
}

/// What a converter result means for the run: an outcome, or an error that would repeat for every
/// file and so ends the run.
fn outcome_of(
    result: Result<Converted, LibraryError>,
    source: &Path,
    library_root: &Path,
) -> Result<Outcome, LibraryError> {
    match result {
        Ok(Converted::Written) => Ok(Outcome::Added),
        Ok(Converted::Existed) => Ok(Outcome::Skipped),
        Err(error) if ends_the_run(&error) => Err(error),
        Err(error) => Ok(Outcome::Failed(attention(source, library_root, &error))),
    }
}

/// Whether a failed file means every further file would fail too.
fn ends_the_run(error: &LibraryError) -> bool {
    use std::io::ErrorKind::{PermissionDenied, ReadOnlyFilesystem};
    match error {
        LibraryError::Full { .. } | LibraryError::FfmpegNotFound => true,
        LibraryError::Io { source, .. } => {
            matches!(source.kind(), PermissionDenied | ReadOnlyFilesystem)
        }
        _ => false,
    }
}

/// The attention item of a failed file: its title and artist from its place in the library.
fn attention(source: &Path, library_root: &Path, error: &LibraryError) -> Attention {
    let title = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let artist = source
        .strip_prefix(library_root)
        .ok()
        .and_then(|relative| relative.components().next())
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    Attention::failed(title, artist, error.to_string())
}

#[cfg(test)]
mod tests {
    use sdl_core::{
        EventKind, Format, Quality, Selection, SongSource, TaskId, WantedSong, channel,
    };

    use super::*;
    use crate::playlists;
    use crate::testkit::{TestResult, lib, make_song, probe};

    fn song(id: &str, title: &str) -> WantedSong {
        WantedSong {
            source: SongSource::Spotify {
                track_id: id.into(),
            },
            title: title.into(),
            artists: vec!["A".into()],
            album: Some("Al".into()),
            album_artist: None,
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: None,
            cover_url: None,
        }
    }

    fn choice(dest: &Path, what: Selection, format: Format) -> ConvertChoice {
        ConvertChoice {
            library: sdl_core::LibraryId::Own,
            what,
            format,
            quality: Quality::Medium,
            keep_cover: true,
            destination: dest.to_path_buf(),
        }
    }

    fn whole() -> Selection {
        Selection {
            whole_library: true,
            playlists: Vec::new(),
        }
    }

    /// A library folder `root` of three songs: `a` and `b` as Opus, `c` as an old MP3; a playlist
    /// `Mix` of a, c. Tests make `root` a folder `Me` under their music root.
    fn library(root: &Path) -> TestResult {
        make_song(&root.join("A/Al/a.opus"), 1, "a")?;
        make_song(&root.join("A/Al/b.opus"), 1, "b")?;
        make_song(&root.join("A/Al/c.mp3"), 1, "c")?;
        playlists::write(
            &lib(root),
            "Mix",
            "Mix.m3u8",
            &[song("1", "a"), song("3", "c")],
        )?;
        Ok(())
    }

    fn sender() -> (EventSender, sdl_core::EventReceiver) {
        let (tx, rx) = channel();
        (EventSender::new(TaskId::Convert, tx), rx)
    }

    #[tokio::test]
    async fn whole_library_is_planned_and_run_with_the_layout_kept() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        let out = tempfile::tempdir()?;
        library(&me)?;
        // `b` is already there, in another format.
        std::fs::create_dir_all(out.path().join("A/Al"))?;
        std::fs::write(out.path().join("A/Al/b.m4a"), b"x")?;
        let picked = choice(out.path(), whole(), Format::Mp3);
        let planned = plan(&lib(&me), &picked).await?;
        let summary = planned.summary();
        assert_eq!((summary.files, summary.already), (2, 1));
        assert!(summary.estimated && summary.can_start() && summary.free_bytes.is_none());

        let (events, mut rx) = sender();
        let counts = run(&planned, &events, &CancellationToken::new()).await?;
        assert_eq!(
            (counts.total, counts.done, counts.added, counts.failed),
            (Some(2), 2, 2, 0)
        );
        assert!(out.path().join("A/Al/a.mp3").exists());
        // `c` was an MP3 already: copied as it is, still tagged.
        assert_eq!(
            std::fs::read(out.path().join("A/Al/c.mp3"))?,
            std::fs::read(me.join("A/Al/c.mp3"))?
        );
        assert!(!out.path().join("A/Al/b.mp3").exists());
        assert_eq!(
            probe(&out.path().join("A/Al/a.mp3"))?
                .get("tag:title")
                .map(String::as_str),
            Some("a")
        );
        // The library is untouched.
        assert!(me.join("A/Al/a.opus").exists());
        let mut last = None;
        while let Ok(event) = rx.try_recv() {
            if let EventKind::Counts(counts) = event.kind {
                last = Some(counts);
            }
        }
        assert_eq!(last, Some(counts));
        Ok(())
    }

    #[tokio::test]
    async fn a_playlist_resolves_to_its_songs_and_is_written_at_the_destination() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        let out = tempfile::tempdir()?;
        library(&me)?;
        // The playlist line says `.opus` but only an MP3 exists for `c`: the stem is what counts.
        let picked = choice(
            out.path(),
            Selection {
                whole_library: false,
                playlists: vec!["Mix".into(), "Missing".into()],
            },
            Format::M4a,
        );
        let planned = plan(&lib(&me), &picked).await?;
        assert_eq!(planned.summary().files, 2);
        let (events, _rx) = sender();
        run(&planned, &events, &CancellationToken::new()).await?;
        assert!(out.path().join("A/Al/a.m4a").exists() && out.path().join("A/Al/c.m4a").exists());
        assert!(!out.path().join("A/Al/b.m4a").exists());
        assert_eq!(
            std::fs::read_to_string(out.path().join("Mix.m3u8"))?,
            "#EXTM3U\n#PLAYLIST:Mix\nA/Al/a.m4a\nA/Al/c.m4a\n"
        );
        assert!(!out.path().join("Missing.m3u8").exists());
        Ok(())
    }

    #[tokio::test]
    async fn a_title_with_a_dot_is_found_through_its_playlist_and_kept_whole() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        let out = tempfile::tempdir()?;
        make_song(&me.join("A/Al/Mr. Brightside.opus"), 1, "Mr. Brightside")?;
        playlists::write(&lib(&me), "Dot", "Dot.m3u8", &[song("1", "Mr. Brightside")])?;
        let what = Selection {
            whole_library: false,
            playlists: vec!["Dot".into()],
        };
        let planned = plan(&lib(&me), &choice(out.path(), what, Format::Mp3)).await?;
        assert_eq!(planned.summary().files, 1);
        let (events, _rx) = sender();
        run(&planned, &events, &CancellationToken::new()).await?;
        assert!(out.path().join("A/Al/Mr. Brightside.mp3").exists());
        let again = plan(
            &lib(&me),
            &choice(
                out.path(),
                Selection {
                    whole_library: false,
                    playlists: vec!["Dot".into()],
                },
                Format::M4a,
            ),
        )
        .await?;
        assert_eq!((again.summary().files, again.summary().already), (0, 1));
        Ok(())
    }

    #[tokio::test]
    async fn a_song_in_the_library_and_a_playlist_is_converted_once() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        let out = tempfile::tempdir()?;
        library(&me)?;
        let mut what = whole();
        what.playlists.push("Mix".into());
        let planned = plan(&lib(&me), &choice(out.path(), what, Format::Mp3)).await?;
        assert_eq!(planned.summary().files, 3);
        Ok(())
    }

    #[tokio::test]
    async fn a_destination_inside_the_library_is_refused_by_where_it_really_is() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        library(&me)?;
        let other = tempfile::tempdir()?;
        let inside = root.path().join("sub/deeper");
        let link = other.path().join("shortcut");
        std::os::unix::fs::symlink(root.path(), &link)?;
        for typed in [
            inside,
            root.path().to_path_buf(),
            link.join("via-link"),
            me.join("A/../A/new"),
        ] {
            let planned = plan(&lib(&me), &choice(&typed, whole(), Format::Mp3)).await?;
            let summary = planned.summary();
            assert!(
                summary.refusal.is_some() && !summary.can_start(),
                "{typed:?}"
            );
            assert_eq!(summary.files, 0);
            let (events, _rx) = sender();
            assert!(
                run(&planned, &events, &CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        assert!(!root.path().join("sub").exists() && !me.join("A/new").exists());
        // A folder beside the library is fine, made or not.
        let beside = other.path().join("not/made/yet");
        let planned = plan(&lib(&me), &choice(&beside, whole(), Format::Mp3)).await?;
        assert!(planned.summary().refusal.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn stop_lets_the_run_end_without_starting_more_files() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        let out = tempfile::tempdir()?;
        library(&me)?;
        let planned = plan(&lib(&me), &choice(out.path(), whole(), Format::Mp3)).await?;
        let stop = CancellationToken::new();
        stop.cancel();
        let (events, _rx) = sender();
        let counts = run(&planned, &events, &stop).await?;
        assert_eq!((counts.done, counts.added), (0, 0));
        assert_eq!(std::fs::read_dir(out.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn a_full_destination_ends_the_run_and_one_bad_file_does_not() -> TestResult {
        let root = Path::new("/lib");
        let source = Path::new("/lib/A/Al/a.opus");
        let ended = |error: LibraryError| outcome_of(Err(error), source, root).is_err();
        assert!(ended(LibraryError::Full {
            path: PathBuf::from("/out/a.mp3")
        }));
        assert!(ended(LibraryError::FfmpegNotFound));
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::ReadOnlyFilesystem,
        ] {
            assert!(ended(LibraryError::Io {
                path: PathBuf::from("/out"),
                source: kind.into()
            }));
        }
        let bad = outcome_of(
            Err(LibraryError::Ffmpeg {
                path: source.to_path_buf(),
                message: "broken".into(),
            }),
            source,
            root,
        )?;
        assert!(matches!(bad, Outcome::Failed(item) if item.title == "a" && item.artist == "A"));
        assert_eq!(
            outcome_of(Ok(Converted::Written), source, root)?,
            Outcome::Added
        );
        assert_eq!(
            outcome_of(Ok(Converted::Existed), source, root)?,
            Outcome::Skipped
        );
        Ok(())
    }

    #[tokio::test]
    async fn one_broken_file_is_listed_and_the_rest_go_on() -> TestResult {
        let root = tempfile::tempdir()?;
        let me = root.path().join("Me");
        let out = tempfile::tempdir()?;
        library(&me)?;
        std::fs::write(me.join("A/Al/b.opus"), b"not audio")?;
        let planned = plan(&lib(&me), &choice(out.path(), whole(), Format::M4a)).await?;
        // The broken file counts at its own size, so the plan still starts.
        assert_eq!(planned.summary().files, 3);
        let (events, mut rx) = sender();
        let counts = run(&planned, &events, &CancellationToken::new()).await?;
        assert_eq!((counts.done, counts.added, counts.failed), (3, 2, 1));
        let mut failed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let EventKind::Attention(item) = event.kind {
                failed.push(item);
            }
        }
        assert_eq!(failed.len(), 1);
        assert_eq!(
            (failed[0].title.as_str(), failed[0].artist.as_str()),
            ("b", "A")
        );
        assert!(failed[0].reason.contains("ffmpeg"), "{}", failed[0].reason);
        assert!(!out.path().join("A/Al/b.m4a").exists());
        Ok(())
    }
}
