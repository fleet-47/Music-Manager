//! Count bookkeeping, the time estimate, landing files and the run-line wording shared by the
//! functions' tasks.

use std::fmt::Write as _;
use std::path::Path;
use std::pin::pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use sdl_core::place::tmp_dir;
use sdl_core::{Attention, Counts, EventKind, EventSender};
use tempfile::{TempDir, TempPath};

use crate::TaskError;
use sdl_core::words::{need_attention, thousands, x_of_y};

/// How one item ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Downloaded and placed in the library (or converted, or copied).
    Added,
    /// Downloaded and placed in the library, but left without a genre and/or a cover (F6):
    /// counted as added, and in the last-run line's "without a genre" and "without a cover".
    AddedMissing {
        /// `MusicBrainz` gave no genre.
        genre: bool,
        /// The cover could not be downloaded or stored.
        cover: bool,
    },
    /// Already in the library.
    Skipped,
    /// Held for review: no confident match.
    Held(Attention),
    /// Failed.
    Failed(Attention),
}

/// The counts of one run: what the UI keeps ([`Counts`]) and the songs added without a genre or
/// without a cover, which only the last-run line uses (F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    counts: Counts,
    without_genre: usize,
    without_cover: usize,
}

impl Tally {
    /// A tally for `total` items, nothing done.
    #[must_use]
    pub fn new(total: usize) -> Self {
        Self {
            counts: Counts {
                total: Some(total),
                ..Counts::default()
            },
            without_genre: 0,
            without_cover: 0,
        }
    }

    /// The counts so far.
    #[must_use]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Count one finished item. Returns the attention item when the outcome needs attention.
    pub fn record(&mut self, outcome: Outcome) -> Option<Attention> {
        self.counts.done = self.counts.done.saturating_add(1);
        match outcome {
            Outcome::Added => {
                self.counts.added = self.counts.added.saturating_add(1);
                None
            }
            Outcome::AddedMissing { genre, cover } => {
                self.counts.added = self.counts.added.saturating_add(1);
                if genre {
                    self.without_genre = self.without_genre.saturating_add(1);
                }
                if cover {
                    self.without_cover = self.without_cover.saturating_add(1);
                }
                None
            }
            Outcome::Skipped => {
                self.counts.skipped = self.counts.skipped.saturating_add(1);
                None
            }
            Outcome::Held(item) => {
                self.counts.held = self.counts.held.saturating_add(1);
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
///
/// # Errors
/// The first error the stream yields.
pub async fn drive<S>(outcomes: S, total: usize, events: &EventSender) -> Result<Tally, TaskError>
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
    Ok(tally)
}

/// A new scratch directory inside `<music_root>/.sdl-tmp` (same volume as the library, so the
/// final rename is atomic), for a conversion's output. Downloads make their own
/// (`sdl_youtube::Downloaded`). The directory and anything in it go when the [`TempDir`] is
/// dropped.
///
/// # Errors
/// The directory could not be created.
pub fn scratch_dir(root: &Path) -> std::io::Result<TempDir> {
    let parent = tmp_dir(root);
    std::fs::create_dir_all(&parent)?;
    tempfile::Builder::new().prefix("sdl-").tempdir_in(parent)
}

/// Move a finished download to `dest` (creating its folder). A file already there is never
/// replaced: that is [`Outcome::Skipped`].
///
/// # Errors
/// The folder could not be made or the file could not be moved.
pub async fn place(file: &Path, dest: &Path) -> std::io::Result<Outcome> {
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

/// The last-run line of a stopped run, with what had finished: `stopped after 412 of 2,117`, or
/// `stopped after 3` when the total was not known yet.
#[must_use]
pub fn stopped_line(counts: &Counts) -> String {
    match counts.total {
        Some(total) => format!("stopped after {}", x_of_y(counts.done, total)),
        None => format!("stopped after {}", thousands(counts.done)),
    }
}

/// `tally` with the `already` items that were left out of the run before it started counted as
/// skipped, for the last-run line. The run itself counts only the work it does, so its bar and its
/// time estimate are not thrown off by songs that were never going to be touched.
#[must_use]
pub fn with_already(tally: Tally, already: usize) -> Tally {
    Tally {
        counts: Counts {
            skipped: tally.counts.skipped.saturating_add(already),
            ..tally.counts
        },
        ..tally
    }
}

/// The last-run line of a run that writes files: `12 copied`, `3 converted, 1 needs attention`.
#[must_use]
pub fn written_line(counts: &Counts, verb: &str) -> String {
    let mut line = format!("{} {verb}", thousands(counts.added));
    push_attention(&mut line, counts);
    line
}

/// The last-run line of a download run: `412 added, 12 already there, 30 without a genre, 2
/// without a cover, 3 need attention`. A part that is zero is left out, except the number added.
#[must_use]
pub fn summary_line(tally: &Tally) -> String {
    let counts = tally.counts();
    let mut line = format!("{} added", thousands(counts.added));
    if counts.skipped > 0 {
        let _ = write!(line, ", {} already there", thousands(counts.skipped));
    }
    if tally.without_genre > 0 {
        let _ = write!(line, ", {} without a genre", thousands(tally.without_genre));
    }
    if tally.without_cover > 0 {
        let _ = write!(line, ", {} without a cover", thousands(tally.without_cover));
    }
    push_attention(&mut line, &counts);
    line
}

fn push_attention(line: &mut String, counts: &Counts) {
    let attention = counts.need_attention();
    if attention > 0 {
        let _ = write!(line, ", {}", need_attention(attention));
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
        assert_eq!(tally.record(Outcome::Held(item("u"))), Some(item("u")));
        assert_eq!(tally.record(Outcome::Failed(item("f"))), Some(item("f")));
        let missing = Outcome::AddedMissing {
            genre: true,
            cover: false,
        };
        assert_eq!(tally.record(missing), None);
        assert_eq!(tally.without_genre, 1);
        assert_eq!(tally.without_cover, 0);
        assert_eq!(
            tally.counts(),
            Counts {
                total: Some(5),
                done: 5,
                added: 2,
                skipped: 1,
                held: 1,
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
        let mut tally = Tally::new(2);
        tally.record(Outcome::Added);
        tally.record(Outcome::AddedMissing {
            genre: true,
            cover: false,
        });
        let line = summary_line(&with_already(tally, 10));
        assert_eq!(line, "2 added, 10 already there, 1 without a genre");
    }

    #[test]
    fn songs_without_a_cover_are_counted_in_the_line_only_when_there_are_some() {
        let mut tally = Tally::new(3);
        tally.record(Outcome::Added);
        assert_eq!(summary_line(&tally), "1 added");
        tally.record(Outcome::AddedMissing {
            genre: false,
            cover: true,
        });
        tally.record(Outcome::AddedMissing {
            genre: true,
            cover: true,
        });
        assert_eq!(
            summary_line(&tally),
            "3 added, 1 without a genre, 2 without a cover"
        );
    }

    #[test]
    fn summary_line_mentions_attention_only_when_present() {
        let mut tally = Tally::new(4);
        tally.record(Outcome::Added);
        assert_eq!(summary_line(&tally), "1 added");
        tally.record(Outcome::Skipped);
        assert_eq!(summary_line(&tally), "1 added, 1 already there");
        tally.record(Outcome::Held(item("h")));
        tally.record(Outcome::Failed(item("f")));
        assert_eq!(
            summary_line(&tally),
            "1 added, 1 already there, 2 need attention"
        );
    }

    #[tokio::test]
    async fn drive_sends_counts_and_attention() -> TestResult {
        let (tx, mut rx) = channel();
        let events = EventSender::new(TaskId::DownloadLink, tx);
        let outcomes =
            futures::stream::iter(vec![Ok(Outcome::Added), Ok(Outcome::Failed(item("f")))]);
        let counts = drive(outcomes, 2, &events).await?.counts();
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

    #[test]
    fn a_stopped_run_says_how_far_it_got() {
        let counts = Counts {
            total: Some(2117),
            done: 412,
            ..Counts::default()
        };
        assert_eq!(stopped_line(&counts), "stopped after 412 of 2,117");
        let unknown = Counts {
            total: None,
            done: 3,
            ..Counts::default()
        };
        assert_eq!(stopped_line(&unknown), "stopped after 3");
    }

    #[tokio::test]
    async fn scratch_dirs_are_separate_and_landing_never_replaces_a_file() -> TestResult {
        let root = tempfile::tempdir()?;
        let dest = root.path().join("a/b.opus");
        let first_dir = scratch_dir(root.path())?;
        let second_dir = scratch_dir(root.path())?;
        assert_ne!(first_dir.path(), second_dir.path());
        let first = first_dir.path().join("audio.opus");
        let second = second_dir.path().join("audio.opus");
        assert!(!first.exists());
        std::fs::write(&first, b"new")?;
        assert_eq!(place(&first, &dest).await?, Outcome::Added);
        std::fs::write(&second, b"other")?;
        assert_eq!(place(&second, &dest).await?, Outcome::Skipped);
        assert_eq!(std::fs::read(&dest)?, b"new");
        drop((first_dir, second_dir));
        sdl_core::place::remove_tmp_dir_if_empty(root.path())?;
        assert!(!tmp_dir(root.path()).exists());
        Ok(())
    }
}
