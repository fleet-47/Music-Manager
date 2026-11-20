//! The five functions' dialogs: Sync Spotify, Download link, Review, Sync device and Convert.
//! Each is a form (`sdl_form`) wired to the runner: it says which fields the function asks, what
//! the task's events do to the form (options arrive, a plan is ready, the task is busy), and
//! which `sdl_tasks::Command` the form's reactions send. Nothing here draws; the screen draws
//! [`Dialog::form`] inside a popup.
//!
//! The conversation per function is in the `sdl_tasks::Runner` documentation. Every dialog
//! follows the same rules, so the owner learns one way of working:
//!
//! 1. [`Dialog::open`] starts the task and pre-fills the form from what was remembered. A form
//!    that waits for the task's first offer is busy (`Form::set_busy`, the spinner says what is
//!    happening; a `Headline` that arrives meanwhile replaces the busy text). Download link and
//!    Review are never busy at open.
//! 2. **Continue, then Start.** The primary button reads `Continue` while the summary is missing
//!    or out of date, and `Start` once the summary matches the form. Download link reads `Start`
//!    from the first (it has no summary); Review's list reads `Download N chosen`.
//! 3. **Answers.** `Submit` on `Continue` sends `Answer` and makes the form busy
//!    (`Making the plan`, or the task's headlines). Sync device and Convert also send `Answer`
//!    after each `Reaction::Changed` of a dropdown, radio group, tick list or toggle while nothing
//!    is missing, and once when the form is first ready with nothing missing (the remembered
//!    choices), so the summary follows the owner's choices; a text or folder edit only clears
//!    the summary and sets `Continue` (no plan per keystroke). Sync Spotify sends `Answer` only on
//!    `Continue` (listing songs costs Spotify calls); a change after its summary clears the summary
//!    and sets `Continue` again.
//! 4. **Pending answers.** The dialog counts answers sent and plans received (`NewOfTotal` or
//!    `Plan`); the form stays busy until they match, so an older plan never shows as current and
//!    `Start` never applies to it.
//! 5. **Start.** `Submit` on `Start` sends `Go` (Download link and Review: the `Answer` that starts
//!    the run) and makes the form busy with `Starting`; the dialog closes on `Started`
//!    ([`Step::Close`]), so an error that comes before it is still shown over the form. The
//!    dialog sends `Go` only for a plan that can start: Sync device and Convert when
//!    `PlanSummary::can_start`, otherwise `Form::set_blocked` says why in plain words
//!    (`Not enough room on SNOWSKY: 3.2 GB to copy, 1.1 GB free`, or the plan's refusal).
//! 6. **Leaving.** `Reaction::Cancel` sends `Cancel` and closes, except in Review's song detail,
//!    where it goes back to the list (`Form::set_back`).
//! 7. **Function keys** are what the form ignores (`r`, `o`); [`Dialog::hints`] adds them to the
//!    footer only while they work, so nothing has to be remembered.
//!
//! The app handles the ending events (`Finished`, `Stopped`, `Error`, `RateLimited`) and the
//! sign-in popups itself; a dialog sees every other event of its function. By the time a dialog
//! sees an event, the app already holds the runner's settings read after it (`Msg::Settings`
//! comes first in each batch), so `settings` is current.

pub mod convert;
pub mod device;
pub mod link;
pub mod review;
pub mod spotify;

use crossterm::event::{KeyEvent, MouseEvent};
use sdl_core::{EventKind, TaskId};
use sdl_form::{Focus, Form, Hint, HitMap, Reaction, Target, Value};
use sdl_tasks::Command;

use crate::app::{Cmd, Settings};

/// What the app does after a dialog handled something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The dialog stays open; run these commands.
    Stay(Vec<Cmd>),
    /// The dialog is done (started, cancelled, or its task ended); run these commands and close
    /// it.
    Close(Vec<Cmd>),
}

/// What is particular to each function.
#[derive(Debug)]
enum Extra {
    SyncSpotify(spotify::State),
    DownloadLink,
    Review(Box<review::State>),
    SyncDevice(device::State),
    Convert(convert::State),
}

/// An open function dialog: its task, its current form and what is particular to the function.
#[derive(Debug)]
pub struct Dialog {
    task: TaskId,
    form: Form,
    extra: Extra,
}

impl Dialog {
    /// Open the function's dialog from the remembered choices: the form pre-filled, busy until the
    /// task's first offer arrives. The commands start the task.
    #[must_use]
    pub fn open(task: TaskId, settings: &Settings) -> (Self, Vec<Cmd>) {
        let (form, extra) = match task {
            TaskId::SyncSpotify => {
                let (form, state) = spotify::open(settings);
                (form, Extra::SyncSpotify(state))
            }
            TaskId::DownloadLink => (link::open(), Extra::DownloadLink),
            TaskId::Review => {
                let (form, state) = review::open(settings);
                (form, Extra::Review(Box::new(state)))
            }
            TaskId::SyncDevice => {
                let (form, state) = device::open(settings);
                (form, Extra::SyncDevice(state))
            }
            TaskId::Convert => {
                let (form, state) = convert::open(settings);
                (form, Extra::Convert(state))
            }
        };
        (Self { task, form, extra }, vec![Cmd::Start(task)])
    }

    /// The function.
    #[must_use]
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// The form to draw now (Review: its list, or the detail of one song).
    #[must_use]
    pub fn form(&self) -> &Form {
        &self.form
    }

    /// The form, to advance its spinner ([`Form::tick`]).
    pub fn form_mut(&mut self) -> &mut Form {
        &mut self.form
    }

    /// The footer's hints: the form's, plus the function's own keys while they work (`r` on
    /// Sync device when the focus is not in a tick list; `o` in Review's song detail while a
    /// candidate is chosen).
    #[must_use]
    pub fn hints(&self) -> Vec<Hint> {
        let mut hints = self.form.hints();
        let browsing = self.form.explorer().is_some();
        let own = match &self.extra {
            Extra::SyncDevice(_)
                if !browsing
                    && !matches!(
                        self.form.focus(),
                        Focus::Field(device::WHAT | device::FOLDERS)
                    ) =>
            {
                Some(Hint {
                    key: "r",
                    what: "look again",
                })
            }
            Extra::Review(state)
                if state.list.is_some()
                    && matches!(
                        self.form.value(review::CHOICE),
                        Some(Value::Choice(Some(id))) if review::is_candidate(&id)
                    ) =>
            {
                Some(Hint {
                    key: "o",
                    what: "open in browser",
                })
            }
            _ => None,
        };
        if let Some(own) = own {
            // Before `Tab` and `Esc`, which every form ends with.
            hints.insert(hints.len().saturating_sub(2), own);
        }
        hints
    }

    /// A task event for this dialog's function: options arrive, a plan is ready, the headline
    /// changes. The app handles the ending events (`Finished`, `Stopped`, `Error`,
    /// `RateLimited`) and `SignIn(NeedsBrowser)` itself; the dialog sees the rest.
    pub fn on_event(&mut self, kind: &EventKind, settings: &Settings) -> Step {
        let _ = settings;
        match (&mut self.extra, kind) {
            (Extra::SyncSpotify(state), _) => spotify::on_event(&mut self.form, state, kind),
            (Extra::SyncDevice(state), _) => device::on_event(&mut self.form, state, kind),
            (Extra::Convert(state), _) => convert::on_event(&mut self.form, state, kind),
            (Extra::Review(state), _) => review::on_event(&mut self.form, state, kind),
            (Extra::DownloadLink, _) => Step::Stay(Vec::new()),
        }
    }

    /// A key. The form gets it first; what it ignores is the function's own key (`r` looks for
    /// devices again; `o` opens the highlighted candidate in the browser).
    pub fn key(&mut self, key: KeyEvent, settings: &Settings) -> Step {
        let reaction = self.form.key(key);
        self.react(&reaction, Some(key), settings)
    }

    /// Pasted text.
    pub fn paste(&mut self, text: &str, settings: &Settings) -> Step {
        let reaction = self.form.paste(text);
        self.react(&reaction, None, settings)
    }

    /// A mouse event, with the rectangles the form drew last.
    pub fn mouse(&mut self, event: MouseEvent, hits: &HitMap<Target>, settings: &Settings) -> Step {
        let reaction = self.form.mouse(event, hits);
        self.react(&reaction, None, settings)
    }

    /// What a reaction of the form means for this function.
    fn react(&mut self, reaction: &Reaction, key: Option<KeyEvent>, settings: &Settings) -> Step {
        let _ = (key, settings);
        match reaction {
            Reaction::Cancel => Step::Close(vec![Cmd::Send(self.task, Command::Cancel)]),
            _ => Step::Stay(Vec::new()),
        }
    }
}
