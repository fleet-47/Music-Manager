//! The runner and the five functions' tasks (F11): Sync Spotify (F4), Download link (F5), Review
//! (F8), Sync device (F10) and Convert (F9). The only writer of `state.toml` and `review.toml`.
//!
//! The interface layer drives tasks only through [`Runner`]: it starts a function with
//! [`Runner::start`], answers what the function asks with [`Runner::send`] and a [`Command`]
//! carrying [`Answers`] (or an [`Offer`]), and reads what comes back as [`sdl_core::Event`]s on the
//! channel it gave the runner (offers: `Lists`, `Libraries`, `Devices`, `Playlists`, `Folders`; the plan:
//! `NewOfTotal`, `Plan`; the run: `Started`, then `Headline`, `Counts`, `Attention`, `HeldCount`;
//! the end: `Finished`, `Stopped`, `Error`, `RateLimited`). The full conversation per function is
//! in the [`Runner`] documentation. The UI folds every event into an [`sdl_core::TaskState`] per
//! task, the same fold the runner keeps.
//!
//! Songs enter the library through one module, `tasks::intake` (F3); Sync Spotify, Download link
//! and Review differ only in which songs they take and by which route.

mod conversation;
mod failure;
mod progress;
mod runner;
mod tasks;
pub mod tools;

use std::time::{SystemTime, UNIX_EPOCH};

pub use conversation::{Answers, Offer, Recipient, Resolution, ReviewChoice};
pub use progress::estimate_remaining;
pub(crate) use runner::{Flow, TaskContext};
pub use runner::{Runner, StartError};

/// Seconds since the Unix epoch; 0 if the clock is before it.
#[must_use]
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A message from the UI to a running task; the same four kinds drive every function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The form's answers; the task replies with its plan (or, for Download link and Review,
    /// starts at once).
    Answer(Answers),
    /// Something the form needs before it can be answered.
    Offer(Offer),
    /// Start, after an answer whose plan can start; or, for Sync Spotify at
    /// `SignIn(NeedsBrowser)`, "Continue" to the browser sign-in.
    Go,
    /// The owner left the form before starting: the task ends with `Stopped`.
    Cancel,
}

/// Errors that end a task. Turned into an [`sdl_core::ErrorInfo`] (or `RateLimited`) for the popup.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TaskError {
    /// A Spotify failure.
    #[error(transparent)]
    Spotify(#[from] sdl_spotify::SpotifyError),
    /// A `YouTube` failure.
    #[error(transparent)]
    Youtube(#[from] sdl_youtube::YoutubeError),
    /// A library (lookup, tag, playlist, converter) failure.
    #[error(transparent)]
    Library(#[from] sdl_library::LibraryError),
    /// A device failure.
    #[error(transparent)]
    Devices(#[from] sdl_devices::DevicesError),
    /// A file system failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A settings file could not be written.
    #[error(transparent)]
    State(#[from] sdl_core::StoreError),
    /// A new library was refused or its folder could not be made.
    #[error(transparent)]
    NewLibrary(#[from] sdl_core::NewLibraryError),
}
