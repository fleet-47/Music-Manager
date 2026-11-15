//! Progress events sent from worker tasks to the UI.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::{Stage, Status, Track};

/// Channel end that workers use to report to the UI.
pub type EventSender = UnboundedSender<Event>;

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

/// One progress message from a worker to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Spotify sign-in state changed.
    SignIn(SignInState),
    /// Spotify returned its songs; each becomes a row in the table.
    Listed(Vec<Track>),
    /// A song moved to a new stage or status. `key` is [`Track::key`] for a Spotify song, or the item
    /// title for a link download (link items are not listed first, so their first event adds the row).
    Song {
        /// Row key.
        key: String,
        /// Current stage.
        stage: Stage,
        /// Current status.
        status: Status,
    },
    /// Overall progress: `done` of `total` items.
    Progress {
        /// Items finished.
        done: usize,
        /// Items in total.
        total: usize,
    },
    /// What is happening right now, for the activity line.
    Activity(String),
    /// Spotify answered HTTP 429; the run ends. `wait` is the `Retry-After` value, `None` when the
    /// header is missing or unreadable.
    RateLimited {
        /// How long Spotify asks the client to wait.
        wait: Option<Duration>,
    },
    /// Card copy finished; waiting for the user to confirm dot-folder cleanup and unmount.
    CardSynced {
        /// Where the card is mounted.
        mount: PathBuf,
    },
    /// The run ended with an error.
    Error(String),
    /// The run finished.
    Finished,
}

/// Send `event` to the UI. A closed channel means the UI has gone and there is nobody left to tell,
/// so a failed send is ignored.
pub fn emit(events: &EventSender, event: Event) {
    let _ = events.send(event);
}
