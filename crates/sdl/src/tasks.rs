//! Tasks and state (F8), plus the orchestration of Sync Spotify (F2), Download link (F3) and Sync
//! card (F4).
//!
//! # Contract with the UI
//!
//! The UI owns one [`Runner`], built with the sending half of [`sdl_core::channel`], and reads the
//! receiving half. Per function:
//!
//! * `Runner::start_*` starts the task, or fails with [`StartError::AlreadyRunning`] (one task per
//!   function; different functions run side by side).
//! * Every [`sdl_core::Event`] carries its [`sdl_core::TaskId`].
//! * The UI answers a waiting step with [`Runner::send`] and a [`Command`].
//! * [`Runner::stop`] ends a running task from outside (the `s` key, after its confirmation).
//!
//! Each run ends with exactly one terminal event: `Finished`, `Stopped`, `Error` or `RateLimited`.
//! Before `Finished`, `Error`, `RateLimited` and the `Stopped` that follows [`Runner::stop`], the
//! function's last-run line has already been written to the state file and to [`Runner::state`], so
//! the UI reads it from there when the terminal event arrives. The `Stopped` that answers
//! [`Command::Cancel`] writes no last-run line, because nothing ran.
//!
//! ## Sync Spotify
//! 1. Start. Events: `Headline`, `SignIn(..)`, then `Lists(..)`.
//! 2. UI sends [`Command::Choices`] with the ticked lists (the ids are saved to the state file).
//!    Events: `Headline`, then `NewOfTotal { new, total }`.
//! 3. UI sends [`Command::Go`]. Events: `Counts` (a full snapshot after every song), `Attention`
//!    (one per unmatched or failed song), `Headline` (phases only), then `Finished`.
//!
//! ## Download link
//! Start with the URL. Events: `Headline`, `Counts`, `Attention`, then `Finished`.
//!
//! ## Sync card
//! 1. Start. Events: `Headline`, then `CardSummary`.
//! 2. UI sends [`Command::Go`]. Events: `Headline`, `Counts`, then `Finished`. [`Command::Cancel`]
//!    here cleans up and unmounts the card, then ends with `Stopped`.
//! 3. After `Finished` the UI asks "Clean up and unmount?" and on yes awaits
//!    [`Runner::unmount_card`].
//!
//! [`Command::Cancel`] at any waiting step, or a closed command channel, ends the task with
//! `Stopped`. A command that does not fit the step is ignored.

mod card;
mod failure;
mod link;
mod progress;
mod runner;
mod spotify;

use std::time::{SystemTime, UNIX_EPOCH};

use sdl_core::{ListChoice, TaskId};

pub use progress::estimate_remaining;
pub use runner::{Runner, StartError};

/// Display name of a function.
#[must_use]
pub fn task_name(task: TaskId) -> &'static str {
    match task {
        TaskId::SyncSpotify => "Sync Spotify",
        TaskId::DownloadLink => "Download link",
        TaskId::SyncCard => "Sync card",
    }
}

/// Seconds since the Unix epoch; 0 if the clock is before it.
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A message from the UI to a running task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Sync Spotify, answering `Lists`: the lists the owner ticked.
    Choices(Vec<ListChoice>),
    /// Sync Spotify, answering `NewOfTotal`; Sync card, answering `CardSummary`: start.
    Go,
    /// The owner left the popup before starting: the task ends with `Stopped`.
    Cancel,
}

/// Errors that end a task. Turned into an [`sdl_core::ErrorInfo`] (or `RateLimited`) for the popup.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// A Spotify failure.
    #[error(transparent)]
    Spotify(#[from] sdl_spotify::SpotifyError),
    /// A `YouTube` failure.
    #[error(transparent)]
    Youtube(#[from] sdl_youtube::YoutubeError),
    /// A library (lookup, tag, card) failure.
    #[error(transparent)]
    Library(#[from] sdl_library::LibraryError),
    /// A file system failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The state file could not be written.
    #[error(transparent)]
    State(#[from] sdl_core::state::StateError),
}
