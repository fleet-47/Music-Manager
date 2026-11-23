//! The popups the app can have open, as data. The screen draws them; `App::update` opens and
//! closes them. At most one is on top; the others wait under it (a confirmation over a form).

use std::time::{Duration, Instant};

use sdl_core::{ErrorInfo, TaskId};

use crate::dialogs::Dialog;

/// A yes-or-no question (F1 Confirmations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// `s` on a running function: "Stop <name>?". Yes stops it.
    Stop(TaskId),
    /// `q` while a function runs: "Quit and stop <names>?". Yes stops them and quits.
    Quit,
    /// After a completed device sync: "Eject <device>?". Yes asks the runner to eject, and so
    /// does the countdown reaching `deadline` (F10).
    Eject {
        /// The device's name.
        device: String,
        /// When the countdown reaches zero.
        deadline: Instant,
    },
    /// Sync Spotify needs the browser: "Sync Spotify opens your browser to sign in", with
    /// Continue (sends `Go`) and Cancel (sends `Cancel`).
    Browser,
}

/// How long the eject question waits before it ejects by itself (F10).
pub const EJECT_COUNTDOWN: Duration = Duration::from_secs(30);

impl Confirm {
    /// The eject question for `device`, asked at `now`.
    #[must_use]
    pub fn eject(device: String, now: Instant) -> Self {
        Self::Eject {
            device,
            deadline: now.checked_add(EJECT_COUNTDOWN).unwrap_or(now),
        }
    }

    /// Whole seconds the countdown of the eject question has left at `now`, rounded up; `None`
    /// for any other question.
    #[must_use]
    pub fn seconds_left(&self, now: Instant) -> Option<u64> {
        let Self::Eject { deadline, .. } = self else {
            return None;
        };
        let left = deadline.saturating_duration_since(now);
        Some(
            left.as_secs()
                .saturating_add(u64::from(left.subsec_nanos() > 0)),
        )
    }

    /// How long until the countdown's number changes, or `None` for any other question.
    #[must_use]
    pub fn next_second(&self, now: Instant) -> Option<Duration> {
        let Self::Eject { deadline, .. } = self else {
            return None;
        };
        let left = deadline.saturating_duration_since(now);
        Some(match left.subsec_nanos() {
            0 if left.is_zero() => Duration::ZERO,
            0 => Duration::from_secs(1),
            nanos => Duration::new(0, nanos),
        })
    }

    /// Whether the countdown has reached zero at `now`.
    #[must_use]
    pub fn expired(&self, now: Instant) -> bool {
        self.seconds_left(now) == Some(0)
    }

    /// The answer that has the focus when the question opens: the safe "no" where yes does damage
    /// that cannot be undone (stopping a run, quitting while one runs, ejecting), and "yes" to opening the browser, which the owner asked for by choosing
    /// Sync Spotify. Enter presses the focused button, so it does what is shown.
    #[must_use]
    pub fn enter_answers_yes(&self) -> bool {
        matches!(self, Self::Browser)
    }
}

/// A popup that only waits, with a spinner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waiting {
    /// "Waiting for you to allow access in the browser", with Cancel (sends `Cancel`).
    Browser,
    /// "Ejecting <device>", until the runner's eject future ends. Esc (or Hide) only hides it:
    /// the eject goes on, and a failure still shows when it comes.
    Eject(String),
}

/// One open popup.
#[derive(Debug)]
pub enum Popup {
    /// A function's form (and, for Review, its list and its detail).
    Dialog(Box<Dialog>),
    /// The held and failed items of a function's last or current run; `scroll` is the first line
    /// shown.
    Attention {
        /// Whose items.
        task: TaskId,
        /// First line shown.
        scroll: usize,
    },
    /// A yes-or-no question, and whether its Yes button has the focus (Left, Right and Tab move
    /// it; Enter presses the focused one).
    Confirm(Confirm, bool),
    /// What went wrong and what to do.
    Error(ErrorInfo),
    /// The eject succeeded: the named device is safe to unplug. Shown even when the waiting
    /// popup was hidden, so the owner is always told.
    Ejected(String),
    /// Every key and mouse action; `scroll` is the first line shown.
    Help {
        /// First line shown.
        scroll: usize,
    },
    /// Waiting for something outside the form.
    Waiting(Waiting),
}

impl Popup {
    /// The question `confirm`, with the focus on the answer it starts on.
    #[must_use]
    pub fn ask(confirm: Confirm) -> Self {
        let yes = confirm.enter_answers_yes();
        Self::Confirm(confirm, yes)
    }

    /// Whether the popup shows a spinner (so the spinner timer runs).
    #[must_use]
    pub fn shows_spinner(&self) -> bool {
        match self {
            Self::Dialog(dialog) => dialog.form().is_animating(),
            Self::Waiting(_) => true,
            Self::Attention { .. }
            | Self::Confirm(..)
            | Self::Error(_)
            | Self::Ejected(_)
            | Self::Help { .. } => false,
        }
    }
}
