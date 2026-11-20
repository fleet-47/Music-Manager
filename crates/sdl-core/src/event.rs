//! Task identity, per-task counts and the events that tasks send to the UI.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::Device;

/// Id of the Liked Songs entry in a list of [`ListChoice`]. It contains a hyphen, which a Spotify
/// playlist id (base 62) never does, so it cannot collide with one.
pub const LIKED_SONGS_ID: &str = "liked-songs";

/// A function of the app, and so the identity of the task that runs it. Every event carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskId {
    /// Sync Spotify (F2).
    SyncSpotify,
    /// Download link (F3).
    DownloadLink,
    /// Convert (F10).
    Convert,
    /// Sync device (F4). Old state files call it `sync_card`.
    #[serde(alias = "sync_card")]
    SyncDevice,
}

impl TaskId {
    /// The four functions, in the order of the main panel.
    pub const ALL: [Self; 4] = [
        Self::SyncSpotify,
        Self::DownloadLink,
        Self::Convert,
        Self::SyncDevice,
    ];
}

/// Per-task counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Items to process.
    pub total: usize,
    /// Items finished, whatever the outcome.
    pub done: usize,
    /// Items added (downloaded, converted, or copied).
    pub added: usize,
    /// Items skipped because they already exist.
    pub skipped: usize,
    /// Songs with no accepted match.
    pub unmatched: usize,
    /// Items that failed.
    pub failed: usize,
}

impl Counts {
    /// Items that need the owner's attention: unmatched plus failed.
    #[must_use]
    pub fn need_attention(&self) -> usize {
        self.unmatched.saturating_add(self.failed)
    }
}

/// Why an item is in the attention list; the UI colours it (unmatched yellow, failed red).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionKind {
    /// No accepted match.
    Unmatched,
    /// The item failed.
    Failed,
}

/// An unmatched or failed item, as listed in the attention list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    /// Unmatched or failed.
    pub kind: AttentionKind,
    /// Song or item title.
    pub title: String,
    /// Artist (empty for a link item that has none).
    pub artist: String,
    /// One-line plain reason.
    pub reason: String,
}

impl Attention {
    /// An item with no accepted match.
    #[must_use]
    pub fn unmatched(title: String, artist: String, reason: String) -> Self {
        Self {
            kind: AttentionKind::Unmatched,
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

/// Spotify sign-in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInState {
    /// The authorize URL is being opened in the browser.
    OpeningBrowser,
    /// Waiting for the browser redirect on the local listener.
    WaitingForBrowser,
    /// Signed in.
    SignedIn,
    /// Sign-in failed, with the reason.
    Failed(String),
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
    /// Bytes that will be written. A file that is converted counts its source size, so this is an
    /// estimate when the plan converts.
    pub bytes: u64,
    /// Files left out because a song with that stem is already there in some format.
    pub already: usize,
    /// Free space where the files go; `None` when it is not known (Convert, From device).
    pub free_bytes: Option<u64>,
    /// Why this plan cannot start, in plain words (Convert: "the destination is inside a folder
    /// being converted"); `None` when nothing stands in the way. Not enough room is
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
    /// `N new of M`: songs not yet in the library, of all songs in the ticked lists.
    NewOfTotal {
        /// Songs not yet in the library.
        new: usize,
        /// Songs in the ticked lists.
        total: usize,
    },
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
    /// Sync device: the mounted external or removable volumes found (sent on start, and again after
    /// a rescan). Empty means none.
    Devices(Vec<Device>),
    /// The top-level folders on the source side, for the Folders tick-list: Convert (the library's,
    /// sent on start) or Sync device (answering a folders request).
    Folders(Vec<String>),
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

/// Sending end bound to one task: every [`EventSender::send`] is tagged with that task.
#[derive(Debug, Clone)]
pub struct EventSender {
    task: TaskId,
    tx: UnboundedSender<Event>,
}

/// A new event channel: the raw sender (wrap it with [`EventSender::new`] per task) and the
/// receiver.
#[must_use]
pub fn channel() -> (UnboundedSender<Event>, EventReceiver) {
    unbounded_channel()
}

impl EventSender {
    /// Bind `tx` to `task`.
    #[must_use]
    pub fn new(task: TaskId, tx: UnboundedSender<Event>) -> Self {
        Self { task, tx }
    }

    /// The task this sender belongs to.
    #[must_use]
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// Send `kind` tagged with this task. A closed channel means the UI has gone and there is nobody
    /// left to tell, so a failed send is ignored.
    pub fn send(&self, kind: EventKind) {
        let _ = self.tx.send(Event {
            task: self.task,
            kind,
        });
    }
}

/// What the UI keeps per task: counts, the current headline and the attention items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskState {
    /// Counts.
    pub counts: Counts,
    /// Current headline.
    pub headline: String,
    /// Unmatched and failed items.
    pub attention: Vec<Attention>,
}

impl TaskState {
    /// Fold one event into the state. Events that carry none of the three kept things change nothing.
    pub fn apply(&mut self, kind: &EventKind) {
        match kind {
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
            total: 5,
            done: 2,
            unmatched: 1,
            failed: 1,
            ..Counts::default()
        };
        state.apply(&EventKind::Counts(counts));
        state.apply(&EventKind::Headline("Downloading".into()));
        state.apply(&EventKind::Attention(Attention::unmatched(
            "T".into(),
            "A".into(),
            "no match".into(),
        )));
        state.apply(&EventKind::Finished);
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
            Attention::unmatched(String::new(), String::new(), String::new()).kind,
            AttentionKind::Unmatched
        );
    }

    #[test]
    fn a_plan_starts_when_it_fits_and_nothing_refuses_it() {
        let plan = |bytes, free_bytes| PlanSummary {
            files: 1,
            bytes,
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
    fn the_four_functions_in_panel_order() {
        assert_eq!(TaskId::ALL.len(), 4);
        assert_eq!(TaskId::ALL.first(), Some(&TaskId::SyncSpotify));
        assert_eq!(TaskId::ALL.last(), Some(&TaskId::SyncDevice));
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
