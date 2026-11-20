//! The file side of device sync and Convert: which top-level folders a root offers, which audio
//! files lie under the ticked ones, the add-only copy, the plan both functions show before they
//! start, and the one runner that carries a plan out ([`run`]).
//!
//! Planning and copying calls block; run them with `spawn_blocking`. [`run`] is async and does
//! that itself.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};

use futures::StreamExt;
use sdl_core::format::is_audio_file;
use sdl_core::paths::{Target, temp_file};
use sdl_core::{Attention, Counts, EventKind, EventSender, Format, PlanSummary};
use walkdir::WalkDir;

use crate::converter::convert_file;
use crate::{LibraryError, blocking, io_err};

/// One file to write: copy `src` to `dest`, or convert it to `convert_to` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    /// The source file.
    pub(crate) src: PathBuf,
    /// Where the result goes, with its final extension.
    pub(crate) dest: PathBuf,
    /// `Some(format)` when the file must be converted; `None` when it is copied as is.
    pub(crate) convert_to: Option<Format>,
    /// Size of the source in bytes.
    pub(crate) bytes: u64,
}

/// The jobs found by [`plan_jobs`], and how many files were left out because they are already
/// there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Planned {
    pub(crate) jobs: Vec<Job>,
    pub(crate) already: usize,
}

/// What a device sync or a Convert would do: the summary the owner sees, and the work behind it.
/// Made by [`crate::converter::plan`] or [`crate::devices::plan`], carried out by [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Counts, bytes, free space and any refusal, for the summary line.
    pub summary: PlanSummary,
    jobs: Vec<Job>,
    /// Where a conversion is written before it is copied to its destination; `None` writes it
    /// beside its destination.
    staging: Option<PathBuf>,
    /// Files worked on at once.
    parallel: NonZeroUsize,
}

impl Plan {
    /// A plan for `planned`. `free_bytes` is the free space of the side written to, when known.
    /// `staging` is where conversions are written first (device sync: `<music_root>/.sdl-tmp`).
    /// A plan with any conversion works on `std::thread::available_parallelism()` files at once;
    /// a plan of plain copies works on one at a time.
    pub(crate) fn new(planned: Planned, free_bytes: Option<u64>, staging: Option<PathBuf>) -> Self {
        let converts = planned.jobs.iter().any(|job| job.convert_to.is_some());
        let parallel = if converts {
            std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
        } else {
            NonZeroUsize::MIN
        };
        let bytes = planned
            .jobs
            .iter()
            .fold(0_u64, |sum, job| sum.saturating_add(job.bytes));
        Self {
            summary: PlanSummary {
                files: planned.jobs.len(),
                bytes,
                already: planned.already,
                free_bytes,
                refusal: None,
            },
            jobs: planned.jobs,
            staging,
            parallel,
        }
    }

    /// A plan that cannot start, for `reason` in plain words. It has no files.
    pub(crate) fn refused(reason: String) -> Self {
        let mut plan = Self::new(Planned::default(), None, None);
        plan.summary.refusal = Some(reason);
        plan
    }
}

/// An audio file found under a root.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFile {
    /// Full path.
    path: PathBuf,
    /// Path relative to the root, starting with its top-level folder.
    rel: PathBuf,
    /// Size in bytes.
    bytes: u64,
}

fn is_dot(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// The names of the top-level folders of `root`, sorted, without dot folders. A missing `root` has
/// none.
///
/// # Errors
/// `root` exists but cannot be read.
pub fn top_level_folders(root: &Path) -> Result<Vec<String>, LibraryError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_err(root)(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_err(root))?;
        let name = entry.file_name();
        if is_dot(&name) || !entry.file_type().map_err(io_err(root))?.is_dir() {
            continue;
        }
        if let Some(name) = name.to_str() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Whether `folder` is one plain, non-dot name: it cannot leave `root` or name a dot folder.
fn is_plain_folder(folder: &str) -> bool {
    let mut parts = Path::new(folder).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(name)), None) if !is_dot(name)
    )
}

/// The audio files ([`is_audio_file`]) under the ticked top-level `folders` of `root`, folder by
/// folder in name order. Files and directories whose name starts with a dot are never included, so
/// no dot-named path can reach a device. A ticked folder that no longer exists, or that is not a
/// plain folder name, contributes nothing.
///
/// # Errors
/// A read or walk failure.
fn audio_files(root: &Path, folders: &[String]) -> Result<Vec<SourceFile>, LibraryError> {
    let mut files = Vec::new();
    for folder in folders.iter().filter(|folder| is_plain_folder(folder)) {
        let walker = WalkDir::new(root.join(folder))
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_dot(entry.file_name()));
        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error)
                    if error.depth() == 0
                        && error
                            .io_error()
                            .is_some_and(|io| io.kind() == ErrorKind::NotFound) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
                continue;
            }
            if let Ok(rel) = entry.path().strip_prefix(root) {
                files.push(SourceFile {
                    path: entry.path().to_path_buf(),
                    rel: rel.to_path_buf(),
                    bytes: entry.metadata()?.len(),
                });
            }
        }
    }
    Ok(files)
}

/// Copy `src` to `dest` as is: `File::create` + `io::copy` to `<dest>.part`, renamed when
/// complete, creating `dest`'s folder. A failed copy leaves neither `dest` nor the `.part` file,
/// so a full device is not filled further by half-copied files.
///
/// # Errors
/// A read or write failure.
fn copy_file(src: &Path, dest: &Path) -> Result<(), LibraryError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let mut part_name: OsString = dest.as_os_str().to_owned();
    part_name.push(".part");
    let part = PathBuf::from(part_name);
    let mut from = File::open(src).map_err(io_err(src))?;
    let mut to = File::create(&part).map_err(io_err(&part))?;
    let copied = std::io::copy(&mut from, &mut to)
        .map_err(io_err(&part))
        .and_then(|_| std::fs::rename(&part, dest).map_err(io_err(dest)));
    if copied.is_err() {
        // Best effort: the copy's own error is the one worth reporting.
        drop(std::fs::remove_file(&part));
    }
    copied
}

/// The jobs that write the audio files under the ticked `folders` of `source_root` to
/// `<dest_root>/<same relative path>`. With `format`, a file not already in it is converted and
/// takes its extension; otherwise the file is copied as is. A file is left out (counted in
/// `already`) when a song with its destination stem is present there in any format
/// ([`Target::is_present`]) or is already planned by an earlier file of this plan, so two sources
/// with one stem never write the same song twice. Never plans a dot-named path. Blocks.
///
/// # Errors
/// A read or walk failure.
pub(crate) fn plan_jobs(
    source_root: &Path,
    folders: &[String],
    dest_root: &Path,
    format: Option<Format>,
) -> Result<Planned, LibraryError> {
    let mut planned = Planned::default();
    let mut taken: HashSet<Target> = HashSet::new();
    for file in audio_files(source_root, folders)? {
        let as_is = dest_root.join(&file.rel);
        let Some(target) = Target::of_file(&as_is) else {
            continue;
        };
        if taken.contains(&target) || target.is_present().map_err(io_err(&target.dir))? {
            planned.already = planned.already.saturating_add(1);
            continue;
        }
        let convert_to = format.filter(|wanted| !wanted.holds(&file.path));
        let dest = convert_to.map_or(as_is, |wanted| target.path(wanted));
        taken.insert(target);
        planned.jobs.push(Job {
            src: file.path,
            dest,
            convert_to,
            bytes: file.bytes,
        });
    }
    Ok(planned)
}

/// Carry out `plan`, the same way for Convert and device sync: each job is copied as is
/// ([`copy_file`]) or converted ([`convert_file`]: beside its destination, or into the plan's
/// staging folder and then copied), several at once when the plan converts. Sends a `Counts`
/// snapshot at the start and after every file (`total` the job count, `done` the files finished,
/// `added` the files written), and an `Attention` (title: the file name) for every file that
/// failed; the rest continue. Returns the final counts. Never changes or deletes a source.
///
/// Stopping (dropping this future) kills a running `ffmpeg` and removes its temp file; a copy
/// already started runs to the end on its blocking thread, so the file being copied is finished.
///
/// # Errors
/// [`LibraryError::FfmpegNotFound`] ends the run; every other failure is per file.
pub async fn run(plan: &Plan, events: &EventSender) -> Result<Counts, LibraryError> {
    let mut counts = Counts {
        total: plan.jobs.len(),
        ..Counts::default()
    };
    events.send(EventKind::Counts(counts));
    // Collected first, so the stream holds owned futures and no closure over a borrowed job
    // (which would make the task's future fail `Send`).
    let writes: Vec<_> = plan
        .jobs
        .iter()
        .map(|job| write(job.clone(), plan.staging.clone()))
        .collect();
    let mut results = futures::stream::iter(writes).buffer_unordered(plan.parallel.get());
    while let Some((job, result)) = results.next().await {
        counts.done = counts.done.saturating_add(1);
        match result {
            Ok(()) => counts.added = counts.added.saturating_add(1),
            Err(LibraryError::FfmpegNotFound) => return Err(LibraryError::FfmpegNotFound),
            Err(error) => {
                counts.failed = counts.failed.saturating_add(1);
                let title = job
                    .src
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                events.send(EventKind::Attention(Attention::failed(
                    title,
                    String::new(),
                    error.to_string(),
                )));
            }
        }
        events.send(EventKind::Counts(counts));
    }
    Ok(counts)
}

/// Write one job's file; returns the job with how it went. Owns its arguments, so the future
/// borrows nothing from the plan.
async fn write(job: Job, staging: Option<PathBuf>) -> (Job, Result<(), LibraryError>) {
    let result = write_one(&job, staging.as_deref()).await;
    (job, result)
}

async fn write_one(job: &Job, staging: Option<&Path>) -> Result<(), LibraryError> {
    let Some(format) = job.convert_to else {
        return copy_blocking(job.src.clone(), job.dest.clone()).await;
    };
    let Some(staging) = staging else {
        return convert_file(&job.src, &job.dest, format).await;
    };
    std::fs::create_dir_all(staging).map_err(io_err(staging))?;
    // Removed with what is in it when this function returns or is dropped.
    let dir = tempfile::Builder::new()
        .prefix("sdl-")
        .tempdir_in(staging)
        .map_err(io_err(staging))?;
    let converted = temp_file(dir.path(), format);
    convert_file(&job.src, &converted, format).await?;
    copy_blocking(converted, job.dest.clone()).await
}

/// [`copy_file`] on a blocking thread.
async fn copy_blocking(src: PathBuf, dest: PathBuf) -> Result<(), LibraryError> {
    blocking(move || copy_file(&src, &dest)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(path: &Path, bytes: &[u8]) -> TestResult {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn rels(files: &[SourceFile]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.rel.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn top_level_folders_are_sorted_plain_directories() -> TestResult {
        let root = tempfile::tempdir()?;
        write(&root.path().join("Spotify/a.opus"), b"a")?;
        write(&root.path().join("Youtube/a.opus"), b"a")?;
        write(&root.path().join(".sdl-tmp/x"), b"x")?;
        write(&root.path().join("loose.opus"), b"x")?;
        assert_eq!(top_level_folders(root.path())?, ["Spotify", "Youtube"]);
        assert_eq!(
            top_level_folders(&root.path().join("nope"))?,
            Vec::<String>::new()
        );
        Ok(())
    }

    #[test]
    fn only_ticked_folders_audio_and_no_dot_names() -> TestResult {
        let root = tempfile::tempdir()?;
        write(&root.path().join("Spotify/L/a.opus"), b"a")?;
        write(&root.path().join("Spotify/L/b.MP3"), b"bb")?;
        write(&root.path().join("Spotify/L/cover.jpg"), b"j")?;
        write(&root.path().join("Spotify/L/c.opus.part"), b"p")?;
        write(&root.path().join("Spotify/.cache/c.opus"), b"c")?;
        write(&root.path().join("Spotify/.DS_Store"), b"x")?;
        write(&root.path().join("Youtube/y.m4a"), b"y")?;
        write(&root.path().join(".hidden/h.opus"), b"h")?;
        let files = audio_files(root.path(), &["Spotify".into(), ".hidden".into()])?;
        assert_eq!(rels(&files), ["Spotify/L/a.opus", "Spotify/L/b.MP3"]);
        assert_eq!(files.iter().map(|f| f.bytes).sum::<u64>(), 3);
        Ok(())
    }

    #[test]
    fn folders_that_could_leave_the_root_or_are_gone_give_nothing() -> TestResult {
        let root = tempfile::tempdir()?;
        let lib = root.path().join("lib");
        write(&lib.join("A/a.opus"), b"a")?;
        write(&root.path().join("outside.opus"), b"o")?;
        let bad: Vec<String> = ["..", "../lib", "A/..", "/etc", "Gone", ""]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(audio_files(&lib, &bad)?, Vec::new());
        assert_eq!(
            rels(&audio_files(&lib, &["Gone".into(), "A".into()])?),
            ["A/a.opus"]
        );
        Ok(())
    }

    #[test]
    fn copy_makes_folders_and_leaves_no_part_file() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("a.opus");
        write(&src, b"data")?;
        let dest = dir.path().join("x/y/a.opus");
        copy_file(&src, &dest)?;
        assert_eq!(std::fs::read(&dest)?, b"data");
        assert!(!dir.path().join("x/y/a.opus.part").exists());
        assert!(copy_file(&dir.path().join("missing.opus"), &dest).is_err());
        assert_eq!(std::fs::read(&dest)?, b"data");
        // A source that fails mid-copy (a folder opens, then cannot be read) leaves no part file.
        let failing = dir.path().join("x/y/b.opus");
        assert!(copy_file(dir.path(), &failing).is_err());
        assert!(!failing.exists());
        assert!(!dir.path().join("x/y/b.opus.part").exists());
        Ok(())
    }

    #[test]
    fn as_is_jobs_keep_the_name_and_skip_songs_present_in_any_format() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        write(&src.join("A/x/new.MP3"), b"nn")?;
        write(&src.join("A/x/old.opus"), b"o")?;
        write(&src.join("A/.hidden/h.opus"), b"h")?;
        write(&dest.join("A/x/old.m4a"), b"o")?;
        let planned = plan_jobs(&src, &["A".into()], &dest, None)?;
        assert_eq!(planned.already, 1);
        assert_eq!(
            planned.jobs,
            [Job {
                src: src.join("A/x/new.MP3"),
                dest: dest.join("A/x/new.MP3"),
                convert_to: None,
                bytes: 2,
            }]
        );
        Ok(())
    }

    #[test]
    fn a_format_converts_only_files_not_already_in_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        write(&src.join("A/a.opus"), b"a")?;
        write(&src.join("A/b.m4a"), b"b")?;
        let planned = plan_jobs(&src, &["A".into()], &dest, Some(Format::M4a))?;
        let jobs: Vec<_> = planned
            .jobs
            .iter()
            .map(|job| (job.dest.clone(), job.convert_to))
            .collect();
        assert_eq!(
            jobs,
            [
                (dest.join("A/a.m4a"), Some(Format::M4a)),
                (dest.join("A/b.m4a"), None),
            ]
        );
        let plan = Plan::new(planned, Some(1), None);
        assert_eq!(plan.summary.bytes, 2);
        assert!(!plan.summary.can_start());
        Ok(())
    }

    #[tokio::test]
    async fn run_copies_counts_and_lists_failures() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        write(&src.join("A/a.opus"), b"a")?;
        write(&src.join("A/b.opus"), b"b")?;
        let plan = Plan::new(plan_jobs(&src, &["A".into()], &dest, None)?, None, None);
        std::fs::remove_file(src.join("A/b.opus"))?;
        let (tx, mut rx) = sdl_core::channel();
        let events = EventSender::new(sdl_core::TaskId::SyncDevice, tx);
        let counts = run(&plan, &events).await?;
        assert_eq!(
            (counts.total, counts.done, counts.added, counts.failed),
            (2, 2, 1, 1)
        );
        assert_eq!(std::fs::read(dest.join("A/a.opus"))?, b"a");
        let mut snapshots = 0_usize;
        let mut failed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event.kind {
                EventKind::Counts(_) => snapshots = snapshots.saturating_add(1),
                EventKind::Attention(item) => failed.push(item.title),
                _ => {}
            }
        }
        assert_eq!(snapshots, 3);
        assert_eq!(failed, ["b.opus"]);
        Ok(())
    }
}
