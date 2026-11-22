//! The popups the app can have open, as data. The screen draws them; `App::update` opens and
//! closes them. At most one is on top; the others wait under it (a confirmation over a form).

use sdl_core::{ErrorInfo, TaskId};

use crate::dialogs::Dialog;

/// A yes-or-no question (F1 Confirmations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// `s` on a running function: "Stop <name>?". Yes stops it.
    Stop(TaskId),
    /// `q` while a function runs: "Quit and stop <names>?". Yes stops them and quits.
    Quit,
    /// After a device sync: "Clean up and eject <device>?", saying that clean-up also empties the
    /// device's Trash. Yes asks the runner to eject.
    Eject {
        /// The device's name.
        device: String,
    },
    /// Sync Spotify needs the browser: "Sync Spotify opens your browser to sign in", with
    /// Continue (sends `Go`) and Cancel (sends `Cancel`).
    Browser,
}

impl Confirm {
    /// The answer that has the focus when the question opens: the safe "no" where yes does damage
    /// that cannot be undone (stopping a run, quitting while one runs, clean-up that empties the
    /// device's Trash), and "yes" to opening the browser, which the owner asked for by choosing
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
