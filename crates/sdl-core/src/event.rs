//! Task identity, per-task counts and the events that tasks send to the UI.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::Device;

/// Id of the Liked Songs entry in a list of [`ListChoice`]. It contains a hyphen, which a Spotify
/// playlist id (base 62) never does, so it cannot collide with one.
pub const LIKED_SONGS_ID: &str = "liked-songs";

/// Name of the Liked Songs list, and so of its playlist file (`Liked Songs.m3u8`).
pub const LIKED_SONGS_NAME: &str = "Liked Songs";

/// A function of the app, and so the identity of the task that runs it. Every event carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskId {
    /// Sync Spotify (F4).
    SyncSpotify,
    /// Download link (F5).
    DownloadLink,
    /// Review (F8).
    Review,
    /// Sync device (F10). Old state files call it `sync_card`.
    #[serde(alias = "sync_card")]
    SyncDevice,
    /// Convert (F9).
    Convert,
}

impl TaskId {
    /// The five functions, in the order of the main panel.
    pub const ALL: [Self; 5] = [
        Self::SyncSpotify,
        Self::DownloadLink,
        Self::Review,
        Self::SyncDevice,
        Self::Convert,
    ];

    /// Display name of the function.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SyncSpotify => "Sync Spotify",
            Self::DownloadLink => "Download link",
            Self::Review => "Review",
            Self::SyncDevice => "Sync device",
            Self::Convert => "Convert",
        }
    }
}

/// Per-task counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Items to process; `None` while the total is not known yet (a link or a list still being
    /// read), when the running panel shows a spinner instead of a bar.
    pub total: Option<usize>,
    /// Items finished, whatever the outcome.
    pub done: usize,
    /// Items added (downloaded, converted, or copied).
    pub added: usize,
    /// Items skipped because they already exist.
    pub skipped: usize,
    /// Songs held for review: no confident match.
    pub held: usize,
    /// Items that failed.
    pub failed: usize,
}

impl Counts {
    /// Items that need the owner's attention: held plus failed.
    #[must_use]
    pub fn need_attention(&self) -> usize {
        self.held.saturating_add(self.failed)
    }
}

/// Why an item is in the attention list; the UI colours it (held yellow, failed red).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// Held for review: no confident match.
    Held,
    /// The item failed.
    Failed,
}

/// A held or failed item, as listed in the attention list. Saved with the function's last run
/// (F11), so the list survives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    /// Held or failed.
    pub kind: AttentionKind,
    /// Song or item title.
    pub title: String,
    /// Artist (empty for a link item that has none).
    pub artist: String,
    /// Plain reason, a sentence that wraps.
    pub reason: String,
}

impl Attention {
    /// An item held for review.
    #[must_use]
    pub fn held(title: String, artist: String, reason: String) -> Self {
        Self {
            kind: AttentionKind::Held,
            title,
            artist,
            reason,
        }
    }

    /// An item that failed.
    #[must_use]
    pub fn failed(title: String, artist: String, reason: String) -> Self {
        Self {
            kind: AttentionKind::Failed,
            title,
            artist,
            reason,
        }
    }
}

/// A Spotify list offered for ticking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListChoice {
    /// List name (`Liked Songs` or the playlist name).
    pub name: String,
    /// List id: [`LIKED_SONGS_ID`] or the Spotify playlist id.
    pub id: String,
    /// Number of songs in the list.
    pub count: usize,
}

/// Spotify sign-in progress. A sign-in that fails ends the task with its one `Error` event; there
/// is no separate failed state, so the owner sees one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInState {
    /// No stored sign-in works: the browser is needed. The UI asks "Sync Spotify opens your
    /// browser to sign in" with Continue and Cancel; the task waits for `Go` or `Cancel`.
    NeedsBrowser,
    /// The browser was opened; waiting for the owner to allow access there. `Cancel` ends it and
    /// frees the port.
    WaitingForBrowser,
    /// Signed in; the lists are being fetched (the `Lists` event follows).
    SignedIn,
    /// The macOS Keychain could not read or save the remembered sign-in; the reason in plain
    /// words. Sent at most once per run; the sync goes on.
    KeychainProblem(String),
}

/// An error for the error popup: what went wrong, and what to do about it, in plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInfo {
    /// What went wrong.
    pub message: String,
    /// What the owner should do.
    pub advice: String,
}

/// The plan of a device sync or a Convert, shown in the step form's summary line before the owner
/// starts it: `N files, X GB to copy · M already there · Y GB free on <device>`. When
/// [`PlanSummary::can_start`] is false the summary says why in plain words and Enter is unavailable;
/// the form stays open so the owner can change a row and see a new plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSummary {
    /// Files that will be written.
    pub files: usize,
    /// Bytes that will be written: exact for copies, duration times the preset's nominal bitrate
    /// for conversions (F10); see `estimated`.
    pub bytes: u64,
    /// Whether `bytes` includes an estimate (the plan converts), so the summary says "about".
    pub estimated: bool,
    /// Files left out because a song with that stem is already there in some format.
    pub already: usize,
    /// Free space where the files go; `None` when it is not known (Convert, From device).
    pub free_bytes: Option<u64>,
    /// Why this plan cannot start, in plain words (Convert: the destination is inside the library;
    /// Convert and Sync device: there is nothing to write); `None` when nothing stands in the way. Not enough room is
    /// [`PlanSummary::fits`], not a refusal.
    pub refusal: Option<String>,
}

impl PlanSummary {
    /// Whether the files fit. Unknown free space counts as fitting.
    #[must_use]
    pub fn fits(&self) -> bool {
        self.free_bytes.is_none_or(|free| self.bytes <= free)
    }

    /// Whether Enter may start this plan: nothing refuses it and the files fit.
    #[must_use]
    pub fn can_start(&self) -> bool {
        self.refusal.is_none() && self.fits()
    }
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// Spotify sign-in state changed.
    SignIn(SignInState),
    /// The lists the owner can tick.
    Lists(Vec<ListChoice>),
    /// `N new of M · K waiting in Review` (F4), answering the Sync Spotify form. Songs are counted
    /// once by their Spotify track ID, however many ticked lists name them.
    NewOfTotal {
        /// Songs that the run will look for: not present in the library, not waiting in review,
        /// not marked "never".
        new: usize,
        /// Distinct songs in the ticked lists.
        total: usize,
        /// Listed songs already in the review list.
        waiting: usize,
    },
    /// The run has started: the form is done with, and the running panel shows the task from now
    /// on. Sent once, by the task layer, before the run's first `Counts`; the UI starts the
    /// elapsed-time clock for the time estimate here.
    Started,
    /// What the task is doing now.
    Headline(String),
    /// New counts for the task (a full snapshot).
    Counts(Counts),
    /// An item that needs attention.
    Attention(Attention),
    /// Spotify answered 429; the task ends. `wait` is from `Retry-After`, `None` when absent.
    RateLimited {
        /// How long Spotify asks the client to wait.
        wait: Option<Duration>,
    },
    /// Sync device: the mounted external or removable volumes found (sent on start, and again
    /// when a volume appears or goes, or after `r`). Empty means none.
    Devices(Vec<Device>),
    /// The playlists in the library (file stems), for the What tick list of Convert and Sync
    /// device (To device). Sent when the form opens.
    Playlists(Vec<String>),
    /// Sync device, From device: the device's top-level folders (names not starting with a dot),
    /// answering `Offer::Folders`.
    Folders(Vec<String>),
    /// The review list changed (a song was held, landed or marked "never"); the new number of held
    /// songs, for the Review row. The list itself is read from the runner (`Runner::review`).
    HeldCount(usize),
    /// The plan of a device sync or a Convert is ready.
    Plan(PlanSummary),
    /// The task failed.
    Error(ErrorInfo),
    /// The task finished.
    Finished,
    /// The task was stopped.
    Stopped,
}

/// One message from a task to the UI. Every event carries its task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The task that sent it.
    pub task: TaskId,
    /// What happened.
    pub kind: EventKind,
}

/// Receiving end of the event channel, held by the UI.
pub type EventReceiver = UnboundedReceiver<Event>;

/// Sending end bound to one task: every [`EventSender::send`] is tagged with that task and folded
/// into the task's [`TaskState`], which [`EventSender::snapshot`] reads. Clones share the state.
#[derive(Debug, Clone)]
pub struct EventSender {
    task: TaskId,
    tx: UnboundedSender<Event>,
    seen: Arc<Mutex<TaskState>>,
}

/// A new event channel: the raw sender (wrap it with [`EventSender::new`] per task) and the
/// receiver.
#[must_use]
pub fn channel() -> (UnboundedSender<Event>, EventReceiver) {
    unbounded_channel()
}

impl EventSender {
    /// Bind `tx` to `task`, with an empty [`TaskState`].
    #[must_use]
    pub fn new(task: TaskId, tx: UnboundedSender<Event>) -> Self {
        Self {
            task,
            tx,
            seen: Arc::default(),
        }
    }

    /// The task this sender belongs to.
    #[must_use]
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// What this task has sent so far that the UI keeps: counts, headline, attention items. The
    /// runner reads it to write the last-run line when a task is stopped from outside.
    #[must_use]
    pub fn snapshot(&self) -> TaskState {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Send `kind` tagged with this task. A closed channel means the UI has gone and there is nobody
    /// left to tell, so a failed send is ignored.
    pub fn send(&self, kind: EventKind) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(&kind);
        let _ = self.tx.send(Event {
            task: self.task,
            kind,
        });
    }
}

/// What the UI keeps per task: whether its run started, counts, the current headline and the
/// attention items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskState {
    /// Whether the run started (`Started` was sent): before it, the task is still at its form.
    pub started: bool,
    /// Counts.
    pub counts: Counts,
    /// Current headline.
    pub headline: String,
    /// Held and failed items.
    pub attention: Vec<Attention>,
}

impl TaskState {
    /// Fold one event into the state. Events that carry none of the kept things change nothing.
    pub fn apply(&mut self, kind: &EventKind) {
        match kind {
            EventKind::Started => self.started = true,
            EventKind::Counts(counts) => self.counts = *counts,
            EventKind::Headline(text) => self.headline.clone_from(text),
            EventKind::Attention(item) => self.attention.push(item.clone()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn state_keeps_counts_headline_and_attention() {
        let mut state = TaskState::default();
        let counts = Counts {
            total: Some(5),
            done: 2,
            held: 1,
            failed: 1,
            ..Counts::default()
        };
        state.apply(&EventKind::Counts(counts));
        state.apply(&EventKind::Headline("Downloading".into()));
        state.apply(&EventKind::Attention(Attention::held(
            "T".into(),
            "A".into(),
            "no match".into(),
        )));
        assert!(!state.started);
        state.apply(&EventKind::Started);
        state.apply(&EventKind::Finished);
        assert!(state.started);
        assert_eq!(state.counts, counts);
        assert_eq!(state.counts.need_attention(), 2);
        assert_eq!(state.headline, "Downloading");
        assert_eq!(state.attention.len(), 1);
    }

    #[test]
    fn attention_constructors_set_the_kind() {
        let item = Attention::failed("T".into(), String::new(), "why".into());
        assert_eq!(item.kind, AttentionKind::Failed);
        assert_eq!(
            Attention::held(String::new(), String::new(), String::new()).kind,
            AttentionKind::Held
        );
    }

    #[test]
    fn a_plan_starts_when_it_fits_and_nothing_refuses_it() {
        let plan = |bytes, free_bytes| PlanSummary {
            files: 1,
            bytes,
            estimated: false,
            already: 0,
            free_bytes,
            refusal: None,
        };
        assert!(plan(10, Some(10)).can_start());
        assert!(!plan(11, Some(10)).fits());
        assert!(!plan(11, Some(10)).can_start());
        assert!(plan(u64::MAX, None).fits());
        let refused = PlanSummary {
            refusal: Some("no".into()),
            ..plan(0, None)
        };
        assert!(refused.fits());
        assert!(!refused.can_start());
    }

    #[test]
    fn the_five_functions_in_panel_order() {
        assert_eq!(
            TaskId::ALL,
            [
                TaskId::SyncSpotify,
                TaskId::DownloadLink,
                TaskId::Review,
                TaskId::SyncDevice,
                TaskId::Convert
            ]
        );
        assert_eq!(TaskId::Review.name(), "Review");
    }

    #[test]
    fn the_sender_remembers_what_the_ui_keeps() {
        let (tx, _rx) = channel();
        let sender = EventSender::new(TaskId::Convert, tx);
        let clone = sender.clone();
        clone.send(EventKind::Headline("Converting".into()));
        clone.send(EventKind::Counts(Counts {
            total: Some(3),
            done: 1,
            ..Counts::default()
        }));
        let seen = sender.snapshot();
        assert_eq!(seen.headline, "Converting");
        assert_eq!(seen.counts.done, 1);
    }

    #[test]
    fn events_carry_their_task() -> TestResult {
        let (tx, mut rx) = channel();
        let sender = EventSender::new(TaskId::SyncDevice, tx);
        sender.send(EventKind::Finished);
        let event = rx.try_recv()?;
        assert_eq!(event.task, TaskId::SyncDevice);
        assert_eq!(event.kind, EventKind::Finished);
        Ok(())
    }
}
