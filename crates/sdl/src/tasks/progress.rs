//! Count bookkeeping, the time estimate and the shared download loop.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use sdl_core::paths::tmp_dir;
use sdl_core::{Attention, Counts, EventKind, EventSender, ListChoice};
use tempfile::{TempDir, TempPath};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{Command, TaskError};

/// Wait for the owner's list choice. `None` when the owner cancelled or the UI is gone. Commands
/// that do not answer this step are ignored.
pub(super) async fn wait_for_choices(
    commands: &mut UnboundedReceiver<Command>,
) -> Option<Vec<ListChoice>> {
    loop {
        match commands.recv().await? {
            Command::Choices(chosen) => return Some(chosen),
            Command::Cancel => return None,
            Command::Go => {}
        }
    }
}

/// Wait for `Go`. `false` when the owner cancelled or the UI is gone. Commands that do not answer
/// this step are ignored.
pub(super) async fn wait_for_go(commands: &mut UnboundedReceiver<Command>) -> bool {
    loop {
        match commands.recv().await {
            Some(Command::Go) => return true,
            Some(Command::Cancel) | None => return false,
            Some(Command::Choices(_)) => {}
        }
    }
}

/// Run `work` unless the owner cancels (or the UI goes) first: then `None`, and `work` is dropped,
/// which frees what it holds (the sign-in listener's port, for one). Other commands cannot arrive
/// while a step works, because the UI sends them only after the step's event.
pub(super) async fn unless_cancelled<T>(
    commands: &mut UnboundedReceiver<Command>,
    work: impl Future<Output = T>,
) -> Option<T> {
    let cancelled = async {
        while let Some(command) = commands.recv().await {
            if command == Command::Cancel {
                break;
            }
        }
    };
    tokio::select! {
        out = work => Some(out),
        () = cancelled => None,
    }
}

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
/// library, so the final rename is atomic), and the path in it that yt-dlp writes to, which does
/// not exist yet. The directory and anything in it go when the [`TempDir`] is dropped.
pub(super) fn download_dir(root: &Path) -> std::io::Result<(TempDir, PathBuf)> {
    let parent = tmp_dir(root);
    std::fs::create_dir_all(&parent)?;
    let dir = tempfile::Builder::new().prefix("sdl-").tempdir_in(parent)?;
    let file = dir.path().join("audio.opus");
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

/// The last-run line of a download run: `412 added, 12 skipped, 3 need attention`.
#[must_use]
pub(super) fn summary_line(counts: &Counts) -> String {
    let mut line = format!("{} added, {} skipped", counts.added, counts.skipped);
    let attention = counts.need_attention();
    if attention > 0 {
        let _ = write!(line, ", {attention} need attention");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl_core::{EventKind, TaskId, channel};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn item(title: &str) -> Attention {
        Attention {
            title: title.into(),
            artist: "A".into(),
            reason: "why".into(),
        }
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
            "5 added, 2 skipped, 2 need attention"
        );
        let clean = Counts {
            unmatched: 0,
            failed: 0,
            ..counts
        };
        assert_eq!(summary_line(&clean), "5 added, 2 skipped");
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
        let (first_dir, first) = download_dir(root.path())?;
        let (second_dir, second) = download_dir(root.path())?;
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

    #[tokio::test]
    async fn cancel_drops_the_work_in_progress() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(tx.send(Command::Cancel).is_ok());
        let out = unless_cancelled(&mut rx, std::future::pending::<()>()).await;
        assert_eq!(out, None);
        assert_eq!(unless_cancelled(&mut rx, async { 7 }).await, Some(7));
    }

    #[tokio::test]
    async fn waits_ignore_commands_for_other_steps() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let list = ListChoice {
            name: "Liked Songs".into(),
            id: "liked-songs".into(),
            count: 3,
        };
        assert!(tx.send(Command::Go).is_ok());
        assert!(tx.send(Command::Choices(vec![list.clone()])).is_ok());
        assert_eq!(wait_for_choices(&mut rx).await, Some(vec![list.clone()]));
        assert!(tx.send(Command::Choices(vec![list])).is_ok());
        assert!(tx.send(Command::Go).is_ok());
        assert!(wait_for_go(&mut rx).await);
        assert!(tx.send(Command::Cancel).is_ok());
        assert!(!wait_for_go(&mut rx).await);
        drop(tx);
        assert_eq!(wait_for_choices(&mut rx).await, None);
    }
}
