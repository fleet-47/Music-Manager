//! Tasks and state (F8), plus the orchestration of the four functions: Sync Spotify (F2), Download
//! link (F3), Convert (F10) and Sync device (F4).
//!
//! # Contract with the UI
//!
//! The UI owns one [`Runner`], built with the sending half of [`sdl_core::channel`], and reads the
//! receiving half. Per function:
//!
//! * `Runner::start` starts the task, or fails with [`StartError::AlreadyRunning`] (one task per
//!   function; different functions run side by side).
//! * Every [`sdl_core::Event`] carries its [`sdl_core::TaskId`].
//! * The UI answers a waiting step with [`Runner::send`] and a [`Command`].
//! * [`Runner::stop`] ends a running task from outside (the `s` key, after its confirmation).
//! * [`Runner::discard`] ends a task whose run never started, as the app closes, leaving its
//!   last-run line as it was.
//!
//! Each run ends with exactly one terminal event: `Finished`, `Stopped`, `Error` or `RateLimited`.
//! Before `Finished`, `Error`, `RateLimited` and the `Stopped` that follows [`Runner::stop`], the
//! function's last-run line (with its [`sdl_core::RunOutcome`]) has already been written to the
//! state file and to [`Runner::state`], so the UI reads it from there when the terminal event
//! arrives. The `Stopped` that answers [`Command::Cancel`] writes no last-run line, because nothing
//! ran.
//!
//! ## One conversation for every function
//! `Runner::start(task)` starts the task and its first event (a headline) arrives at once. The
//! UI then drives the one step form with three kinds of message, the same for all four functions
//! ([`Command`]):
//!
//! * `Command::Answer(Answers)`: the form's answers. The task replies with the plan and waits;
//!   send it again whenever the owner changes a row.
//! * `Command::Go`: start. Ignored until the last answer left a plan that can start.
//! * `Command::Cancel`: leave the form; the task ends with `Stopped` and no last-run line.
//!
//! (Sync device alone also takes `Command::Offer(Offer)`: rescan, or the folders of a device and
//! direction. Any offer makes the last plan stale, so answer again before `Go`.) A message that
//! does not fit the step is ignored; one sent while a step is working is kept until it is done,
//! except `Cancel`, which ends the step at once.
//!
//! Per function, the events after `start`:
//!
//! * **Sync Spotify**: `Headline`, `SignIn(..)`, `Lists(..)`. `Answer(Spotify { lists, format })`
//!   (ticked ids are saved; the format when `remember` is set): `Headline`, `NewOfTotal`. `Go`:
//!   `Headline`s, `Counts` (a full snapshot after every song), `Attention` (one per unmatched or
//!   failed song), `Finished`.
//! * **Download link**: `Headline`. `Answer(Link { url, format })` starts the run at once, with
//!   no `Go`: `Headline`s, `Counts`, `Attention`, `Finished`.
//! * **Convert**: `Headline`, `Folders(..)`. `Answer(Convert(choice))` (saved): `Headline`, `Plan`.
//!   A destination inside the folders being read comes back as a plan with a `refusal`, not an
//!   error, so the form stays open. `Go`: `Headline`, `Counts`, `Attention`, `Finished`.
//! * **Sync device**: `Headline`, `Devices(..)` (empty means none found). `Offer::Devices`:
//!   `Devices`. `Offer::Folders { device, direction }`: `Folders`. `Answer(Device { device,
//!   choice })` (saved per device): `Headline`, `Plan`. `Go`: `Headline`, `Counts`, `Attention`,
//!   `Finished`. Then the UI asks "Clean up and eject <device>?" and on yes awaits
//!   [`Runner::eject_device`].
//!
//! A run counts only the work it does: songs already in the library, in any format, are settled
//! before it starts and appear only as "already there" in the last-run line, so the bar and the time
//! estimate are not thrown off by items that were never going to be touched.
//!
//! `.sdl-tmp` is removed only when the last running task ends.

mod convert;
mod device;
mod failure;
mod form;
mod link;
mod progress;
mod runner;
mod spotify;

use std::time::{SystemTime, UNIX_EPOCH};

use sdl_core::TaskId;

pub use form::{Answers, Offer};
pub use progress::estimate_remaining;
pub use runner::{Runner, StartError};

/// Display name of a function.
#[must_use]
pub fn task_name(task: TaskId) -> &'static str {
    match task {
        TaskId::SyncSpotify => "Sync Spotify",
        TaskId::DownloadLink => "Download link",
        TaskId::Convert => "Convert",
        TaskId::SyncDevice => "Sync device",
    }
}

/// Seconds since the Unix epoch; 0 if the clock is before it.
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A message from the UI to a running task; the same three kinds drive every function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The form's answers; the task replies with its plan.
    Answer(Answers),
    /// Sync device: something the form needs before it can be answered.
    Offer(Offer),
    /// Start, after an answer whose plan can start.
    Go,
    /// The owner left the form before starting: the task ends with `Stopped`.
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
