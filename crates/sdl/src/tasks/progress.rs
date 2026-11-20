//! Count bookkeeping, the time estimate and the shared download loop.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::Duration;

use super::TaskError;
use futures::{Stream, StreamExt};
use sdl_core::paths::temp_file;
use sdl_core::paths::tmp_dir;
use sdl_core::{Attention, Counts, EventKind, EventSender, Format};
use tempfile::{TempDir, TempPath};

/// How one item ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Downloaded and placed in the library.
    Added,
    /// Already in the library.
    Skipped,
    /// No accepted match.
    Unmatched(Attention),
    /// Failed.
    Failed(Attention),
}

/// The counts of one run.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tally {
    counts: Counts,
}

impl Tally {
    /// A tally for `total` items, nothing done.
    pub(super) fn new(total: usize) -> Self {
        Self {
            counts: Counts {
                total,
                ..Counts::default()
            },
        }
    }

    /// The counts so far.
    pub(super) fn counts(&self) -> Counts {
        self.counts
    }

    /// Count one finished item. Returns the attention item when the outcome needs attention.
    pub(super) fn record(&mut self, outcome: Outcome) -> Option<Attention> {
        self.counts.done = self.counts.done.saturating_add(1);
        match outcome {
            Outcome::Added => {
                self.counts.added = self.counts.added.saturating_add(1);
                None
            }
            Outcome::Skipped => {
                self.counts.skipped = self.counts.skipped.saturating_add(1);
                None
            }
            Outcome::Unmatched(item) => {
                self.counts.unmatched = self.counts.unmatched.saturating_add(1);
                Some(item)
            }
            Outcome::Failed(item) => {
                self.counts.failed = self.counts.failed.saturating_add(1);
                Some(item)
            }
        }
    }
}

/// Run `outcomes` to the end, sending a `Counts` snapshot at the start and after every item, and an
/// `Attention` for every item that needs it. The first `Err` ends the loop and is returned; the
/// stream is dropped with it, which drops the items in flight.
pub(super) async fn drive<S>(
    outcomes: S,
    total: usize,
    events: &EventSender,
) -> Result<Counts, TaskError>
where
    S: Stream<Item = Result<Outcome, TaskError>>,
{
    let mut tally = Tally::new(total);
    events.send(EventKind::Counts(tally.counts()));
    let mut outcomes = pin!(outcomes);
    while let Some(outcome) = outcomes.next().await {
        if let Some(item) = tally.record(outcome?) {
            events.send(EventKind::Attention(item));
        }
        events.send(EventKind::Counts(tally.counts()));
    }
    Ok(tally.counts())
}

/// A new temp directory for one download, inside `<music_root>/.sdl-tmp` (same volume as the
/// library, so the final rename is atomic), and the path in it that yt-dlp writes to for `format`,
/// which does not exist yet. The directory and anything in it go when the [`TempDir`] is dropped.
pub(super) fn download_dir(root: &Path, format: Format) -> std::io::Result<(TempDir, PathBuf)> {
    let parent = tmp_dir(root);
    std::fs::create_dir_all(&parent)?;
    let dir = tempfile::Builder::new().prefix("sdl-").tempdir_in(parent)?;
    let file = temp_file(dir.path(), format);
    Ok((dir, file))
}

/// Move a finished download to `dest` (creating its folder). A file already there is never
/// replaced: that is [`Outcome::Skipped`].
pub(super) async fn place(file: &Path, dest: &Path) -> std::io::Result<Outcome> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match TempPath::try_from_path(file)?.persist_noclobber(dest) {
        Ok(()) => Ok(Outcome::Added),
        Err(failure) if failure.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(Outcome::Skipped)
        }
        Err(failure) => Err(failure.error),
    }
}

/// Time left: `elapsed / done * remaining`. `None` until something is done, and on overflow.
#[must_use]
pub fn estimate_remaining(elapsed: Duration, done: usize, total: usize) -> Option<Duration> {
    let done = u32::try_from(done).ok().filter(|done| *done > 0)?;
    let remaining = u32::try_from(total.saturating_sub(usize::try_from(done).ok()?)).ok()?;
    elapsed.checked_mul(remaining)?.checked_div(done)
}

/// The top-level folders of `dir`, read off the async threads.
pub(super) async fn folders_in(dir: PathBuf) -> Result<Vec<String>, TaskError> {
    let found = tokio::task::spawn_blocking(move || sdl_library::files::top_level_folders(&dir))
        .await
        .map_err(|error| sdl_library::LibraryError::Task(error.to_string()))??;
    Ok(found)
}

/// `counts` with the `already` items that were left out of the run before it started counted as
/// skipped, for the last-run line. The run itself counts only the work it does, so its bar and its
/// time estimate are not thrown off by songs that were never going to be touched.
#[must_use]
pub(super) fn with_already(counts: Counts, already: usize) -> Counts {
    Counts {
        skipped: counts.skipped.saturating_add(already),
        ..counts
    }
}

/// The last-run line of a run that writes files: `12 copied`, `3 converted, 1 needs attention`.
#[must_use]
pub(super) fn written_line(counts: &Counts, verb: &str) -> String {
    let mut line = format!("{} {verb}", crate::fmt::thousands(counts.added));
    push_attention(&mut line, counts);
    line
}

/// The last-run line of a download run: `412 added, 12 already there, 3 need attention`. A part
/// that is zero is left out, except the number added.
#[must_use]
pub(super) fn summary_line(counts: &Counts) -> String {
    let mut line = format!("{} added", crate::fmt::thousands(counts.added));
    if counts.skipped > 0 {
        let _ = write!(
            line,
            ", {} already there",
            crate::fmt::thousands(counts.skipped)
        );
    }
    push_attention(&mut line, counts);
    line
}

fn push_attention(line: &mut String, counts: &Counts) {
    let attention = counts.need_attention();
    if attention > 0 {
        let _ = write!(line, ", {}", crate::fmt::need_attention(attention));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl_core::{EventKind, TaskId, channel};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn item(title: &str) -> Attention {
        Attention::failed(title.into(), "A".into(), "why".into())
    }

    #[test]
    fn tally_counts_every_outcome() {
        let mut tally = Tally::new(5);
        assert_eq!(tally.record(Outcome::Added), None);
        assert_eq!(tally.record(Outcome::Skipped), None);
        assert_eq!(tally.record(Outcome::Unmatched(item("u"))), Some(item("u")));
        assert_eq!(tally.record(Outcome::Failed(item("f"))), Some(item("f")));
        assert_eq!(
            tally.counts(),
            Counts {
                total: 5,
                done: 4,
                added: 1,
                skipped: 1,
                unmatched: 1,
                failed: 1,
            }
        );
        assert_eq!(tally.counts().need_attention(), 2);
    }

    #[test]
    fn estimate_is_elapsed_over_done_times_remaining() {
        let estimate = estimate_remaining(Duration::from_secs(60), 10, 40);
        assert_eq!(estimate, Some(Duration::from_secs(180)));
        assert_eq!(
            estimate_remaining(Duration::from_secs(60), 40, 40),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn estimate_needs_something_done() {
        assert_eq!(estimate_remaining(Duration::from_secs(60), 0, 40), None);
    }

    #[test]
    fn written_line_names_the_verb() {
        let counts = Counts {
            added: 3,
            failed: 1,
            ..Counts::default()
        };
        assert_eq!(
            written_line(&counts, "converted"),
            "3 converted, 1 needs attention"
        );
        assert_eq!(written_line(&Counts::default(), "copied"), "0 copied");
    }

    #[test]
    fn songs_left_out_before_the_run_count_as_skipped_in_its_line() {
        let counts = Counts {
            total: 2,
            done: 2,
            added: 2,
            ..Counts::default()
        };
        let line = summary_line(&with_already(counts, 10));
        assert_eq!(line, "2 added, 10 already there");
    }

    #[test]
    fn summary_line_mentions_attention_only_when_present() {
        let counts = Counts {
            total: 9,
            done: 9,
            added: 5,
            skipped: 2,
            unmatched: 1,
            failed: 1,
        };
        assert_eq!(
            summary_line(&counts),
            "5 added, 2 already there, 2 need attention"
        );
        let clean = Counts {
            unmatched: 0,
            failed: 0,
            ..counts
        };
        assert_eq!(summary_line(&clean), "5 added, 2 already there");
        let none_there = Counts {
            skipped: 0,
            ..clean
        };
        assert_eq!(summary_line(&none_there), "5 added");
    }

    #[tokio::test]
    async fn drive_sends_counts_and_attention() -> TestResult {
        let (tx, mut rx) = channel();
        let events = EventSender::new(TaskId::DownloadLink, tx);
        let outcomes =
            futures::stream::iter(vec![Ok(Outcome::Added), Ok(Outcome::Failed(item("f")))]);
        let counts = drive(outcomes, 2, &events).await?;
        assert_eq!((counts.done, counts.added, counts.failed), (2, 1, 1));
        let mut kinds = Vec::new();
        while let Ok(event) = rx.try_recv() {
            assert_eq!(event.task, TaskId::DownloadLink);
            kinds.push(event.kind);
        }
        assert_eq!(kinds.len(), 4);
        assert!(matches!(kinds.first(), Some(EventKind::Counts(c)) if c.done == 0));
        assert!(kinds.contains(&EventKind::Attention(item("f"))));
        Ok(())
    }

    #[tokio::test]
    async fn drive_stops_at_the_first_error() {
        let (tx, _rx) = channel();
        let events = EventSender::new(TaskId::SyncSpotify, tx);
        let outcomes = futures::stream::iter(vec![
            Ok(Outcome::Added),
            Err(TaskError::Youtube(sdl_youtube::YoutubeError::YtDlpNotFound)),
            Ok(Outcome::Added),
        ]);
        assert!(drive(outcomes, 3, &events).await.is_err());
    }

    #[tokio::test]
    async fn downloads_get_their_own_dir_and_never_replace_a_file() -> TestResult {
        let root = tempfile::tempdir()?;
        let dest = root.path().join("a/b.opus");
        let (first_dir, first) = download_dir(root.path(), Format::Opus)?;
        let (second_dir, second) = download_dir(root.path(), Format::Opus)?;
        assert_ne!(first_dir.path(), second_dir.path());
        assert!(!first.exists());
        std::fs::write(&first, b"new")?;
        assert_eq!(place(&first, &dest).await?, Outcome::Added);
        std::fs::write(&second, b"other")?;
        assert_eq!(place(&second, &dest).await?, Outcome::Skipped);
        assert_eq!(std::fs::read(&dest)?, b"new");
        drop((first_dir, second_dir));
        sdl_core::paths::remove_tmp_dir_if_empty(root.path())?;
        assert!(!tmp_dir(root.path()).exists());
        Ok(())
    }
}
