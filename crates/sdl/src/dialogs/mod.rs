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
//!    Review are never busy at open. The shared state of rules 2 to 5 is in `conversation.rs`.
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

mod choices;
mod conversation;
mod convert;
mod device;
mod link;
mod review;
mod spotify;
#[cfg(test)]
mod tests;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use sdl_core::{EventKind, TaskId};
use sdl_form::{Focus, Form, Hint, HitMap, Reaction, Target, Value};
use sdl_tasks::{Answers, Command};

use self::conversation::Conversation;
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
    Convert,
}

/// An open function dialog: its task, its current form and what is particular to the function.
#[derive(Debug)]
pub struct Dialog {
    task: TaskId,
    form: Form,
    conv: Conversation,
    extra: Extra,
}

impl Dialog {
    /// Open the function's dialog from the remembered choices: the form pre-filled, busy until the
    /// task's first offer arrives. The commands start the task.
    #[must_use]
    pub fn open(task: TaskId, settings: &Settings) -> (Self, Vec<Cmd>) {
        let (form, conv, extra) = match task {
            TaskId::SyncSpotify => {
                let (form, state, conv) = spotify::open(settings);
                (form, conv, Extra::SyncSpotify(state))
            }
            TaskId::DownloadLink => (link::open(), Conversation::direct(), Extra::DownloadLink),
            TaskId::Review => {
                let (form, state) = review::open(settings);
                (form, Conversation::direct(), Extra::Review(Box::new(state)))
            }
            TaskId::SyncDevice => {
                let (form, state, conv) = device::open(settings);
                (form, conv, Extra::SyncDevice(state))
            }
            TaskId::Convert => {
                let (form, conv) = convert::open(settings);
                (form, conv, Extra::Convert)
            }
        };
        let mut dialog = Self {
            task,
            form,
            conv,
            extra,
        };
        dialog.conv.refresh(&mut dialog.form);
        (dialog, vec![Cmd::Start(task)])
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
        let own = match &self.extra {
            Extra::SyncDevice(_) if self.looks_again_works() => Some(Hint {
                key: "r",
                what: "look again",
            }),
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

    /// `r` works unless the focus is in a tick list (where it is typed) or the folder browser.
    fn looks_again_works(&self) -> bool {
        self.form.explorer().is_none()
            && !matches!(
                self.form.focus(),
                Focus::Field(device::WHAT | device::FOLDERS)
            )
    }

    /// A task event for this dialog's function: options arrive, a plan is ready, the headline
    /// changes. The app handles the ending events (`Finished`, `Stopped`, `Error`,
    /// `RateLimited`) and `SignIn(NeedsBrowser)` itself; the dialog sees the rest. `Started`
    /// closes the dialog.
    pub fn on_event(&mut self, kind: &EventKind, settings: &Settings) -> Step {
        if matches!(kind, EventKind::Started) {
            return Step::Close(Vec::new());
        }
        let mut commands = Vec::new();
        match &mut self.extra {
            Extra::SyncSpotify(state) => {
                spotify::on_event(&mut self.form, state, &mut self.conv, kind);
            }
            Extra::DownloadLink => {
                if let EventKind::Headline(text) = kind {
                    self.conv.headline(text);
                }
            }
            Extra::Review(state) => {
                if let EventKind::Headline(text) = kind {
                    self.conv.headline(text);
                }
                review::on_event(&mut self.form, state, kind, settings);
            }
            Extra::SyncDevice(state) => {
                commands = device::on_event(&mut self.form, state, &mut self.conv, kind, settings);
            }
            Extra::Convert => convert::on_event(&mut self.form, &mut self.conv, kind),
        }
        if matches!(
            kind,
            EventKind::Devices(_) | EventKind::Playlists(_) | EventKind::Folders(_)
        ) {
            commands.extend(self.replan());
        }
        self.conv.refresh(&mut self.form);
        Step::Stay(commands)
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
        let commands = match reaction {
            Reaction::Cancel => {
                if let Extra::Review(state) = &mut self.extra
                    && review::back(&mut self.form, state)
                {
                    Vec::new()
                } else {
                    return Step::Close(vec![self.send(Command::Cancel)]);
                }
            }
            Reaction::Submit => self.submit(settings),
            Reaction::Changed(field) => self.changed(field, settings),
            Reaction::Activated { id, .. } => {
                if let Extra::Review(state) = &mut self.extra {
                    review::open_detail(&mut self.form, state, id, settings);
                }
                Vec::new()
            }
            Reaction::Ignored => self.own_key(key, settings),
            Reaction::Handled => Vec::new(),
        };
        self.conv.refresh(&mut self.form);
        Step::Stay(commands)
    }

    fn send(&self, command: Command) -> Cmd {
        Cmd::Send(self.task, command)
    }

    /// Enter on the primary button (or in a field): `Done` in Review's detail; otherwise `Start`
    /// goes (`Go`) once a plan is shown, and anything else sends the answer.
    fn submit(&mut self, settings: &Settings) -> Vec<Cmd> {
        if let Extra::Review(state) = &mut self.extra
            && state.list.is_some()
        {
            review::done(&mut self.form, state, settings);
            return Vec::new();
        }
        if self.conv.is_planned() {
            self.conv.start();
            return vec![self.send(Command::Go)];
        }
        let Some(answers) = self.answers() else {
            return Vec::new();
        };
        self.conv.ask();
        vec![self.send(Command::Answer(answers))]
    }

    /// The form's answers, as the task layer takes them.
    fn answers(&self) -> Option<Answers> {
        match &self.extra {
            Extra::SyncSpotify(state) => spotify::answers(&self.form, state),
            Extra::DownloadLink => Some(link::answers(&self.form)),
            Extra::Review(state) => Some(review::answers(state)),
            Extra::SyncDevice(state) => device::answers(&self.form, state),
            Extra::Convert => Some(convert::answers(&self.form)),
        }
    }

    /// A field changed: nothing shown is current, and a new plan is asked for when the field is
    /// one that decides it (rule 3).
    fn changed(&mut self, field: &str, settings: &Settings) -> Vec<Cmd> {
        let mut commands = Vec::new();
        let mut replans = false;
        match &mut self.extra {
            Extra::SyncSpotify(_) => self.conv.changed(),
            Extra::DownloadLink => {}
            Extra::Review(state) => {
                if state.list.is_some() {
                    review::changed(&mut self.form, field);
                }
            }
            Extra::SyncDevice(state) => {
                self.conv.changed();
                self.form.set_blocked(None);
                commands = device::changed(&mut self.form, state, &mut self.conv, field, settings);
                replans = true;
            }
            Extra::Convert => {
                self.conv.changed();
                self.form.set_blocked(None);
                replans = convert::changed(&mut self.form, field);
            }
        }
        if replans {
            commands.extend(self.replan());
        }
        commands
    }

    /// Ask for a new plan when the form changed, nothing is awaited and nothing is missing.
    fn replan(&mut self) -> Vec<Cmd> {
        if !self.conv.is_stale() || self.conv.is_waiting() || !self.form.missing().is_empty() {
            return Vec::new();
        }
        let Some(answers) = self.answers() else {
            return Vec::new();
        };
        self.conv.ask();
        vec![self.send(Command::Answer(answers))]
    }

    /// A key the form ignored: `r` looks for devices again; `o` opens the chosen candidate.
    fn own_key(&mut self, key: Option<KeyEvent>, settings: &Settings) -> Vec<Cmd> {
        let plain = |key: KeyEvent, c: char| {
            key.code == KeyCode::Char(c)
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        };
        let Some(key) = key else {
            return Vec::new();
        };
        let works = self.looks_again_works();
        match &mut self.extra {
            Extra::SyncDevice(state) if plain(key, 'r') && works => {
                device::look_again(state, &mut self.conv)
            }
            Extra::Review(state) if plain(key, 'o') => {
                review::chosen_candidate(&self.form, state, settings)
                    .map(|candidate| vec![Cmd::Open(candidate.url())])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}
