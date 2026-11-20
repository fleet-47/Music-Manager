//! Screen state (F1, F8). State changes only from key events, paste events, task events and ticks;
//! nothing here draws. Every change that needs the outside world is returned as a [`Command`].

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sdl_core::{ErrorInfo, Event, EventKind, RunOutcome, SignInState, State, TaskId, TaskState};
use throbber_widgets_tui::ThrobberState;

use crate::control::Command;
use crate::fmt;
use crate::forms::{Dialog, Reply};
use crate::keys::{self, Hint};
use crate::popups;

/// The functions, in the order of the main panel.
pub const FUNCTIONS: [TaskId; 4] = TaskId::ALL;

const PAGE: usize = 10;

/// Where a function is in its life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing going on.
    #[default]
    Idle,
    /// A step waits for the owner (popup open); no running block yet.
    Preparing,
    /// The owner left a waiting step; the task's `Stopped` has not arrived yet.
    Cancelling,
    /// The task runs: it has a block in the running panel.
    Running,
}

/// What the screen keeps per function.
#[derive(Debug, Clone, Default)]
pub struct TaskView {
    /// Life stage.
    pub phase: Phase,
    /// Counts, headline and attention items.
    pub state: TaskState,
    /// Time since the task started.
    pub elapsed: Duration,
}

/// A centred popup.
#[derive(Debug, Clone)]
pub enum Popup {
    /// Spotify sign-in state.
    SignIn(SignInState),
    /// Convert or Sync device, waiting for what their step form offers (the library's folders, the
    /// devices found).
    Looking(TaskId),
    /// A function's step form (F1).
    Form(Dialog),
    /// "Clean up and eject <device>?"
    EjectConfirm(String),
    /// The device is being cleaned and unmounted.
    Ejecting,
    /// The attention list of a function, with the row under the cursor.
    Attention {
        /// Whose list.
        task: TaskId,
        /// Row under the cursor.
        cursor: usize,
    },
    /// Confirm stopping a running task.
    ConfirmStop(TaskId),
    /// Confirm quitting while a task runs.
    ConfirmQuit,
    /// What went wrong in a function's run, and what to do.
    Error(TaskId, ErrorInfo),
    /// Every key.
    Help {
        /// First visible line.
        scroll: usize,
    },
}

impl Popup {
    /// The function whose steps this popup belongs to; it closes when that function's run ends.
    fn owner(&self) -> Option<TaskId> {
        match self {
            Self::SignIn(_) => Some(TaskId::SyncSpotify),
            Self::EjectConfirm(_) | Self::Ejecting => Some(TaskId::SyncDevice),
            Self::Looking(task) | Self::ConfirmStop(task) => Some(*task),
            Self::Form(dialog) => Some(dialog.task()),
            Self::Attention { .. } | Self::ConfirmQuit | Self::Error(..) | Self::Help { .. } => {
                None
            }
        }
    }
}

/// The screen state.
#[derive(Debug)]
pub struct App {
    /// The chosen function.
    pub selected: TaskId,
    /// Open popups, the last one on top. Only the top one is drawn and takes keys, so a popup
    /// from one function never hides another function's waiting step for good.
    pub popups: Vec<Popup>,
    /// Remembered ticks and last-run lines, as the task runner keeps them.
    pub state: State,
    /// The last link given this session.
    pub link: Option<String>,
    /// The device chosen in the last Sync device run.
    pub device_name: Option<String>,
    /// That device was cleaned up and unmounted, so it is safe to unplug.
    pub device_ejected: bool,
    /// Spinner position.
    pub throbber: ThrobberState,
    /// Seconds since the Unix epoch, as of the last tick.
    pub now: u64,
    viewport: (u16, u16),
    spotify: TaskView,
    download: TaskView,
    convert: TaskView,
    device: TaskView,
    quit: bool,
}

/// `y` or Enter is yes, `n` or Esc is no, anything else is no answer.
fn answer(event: KeyEvent) -> Option<bool> {
    match event.code {
        KeyCode::Char('y') | KeyCode::Enter => Some(true),
        KeyCode::Char('n') | KeyCode::Esc => Some(false),
        _ => None,
    }
}

impl App {
    /// A new screen over the remembered `state`.
    #[must_use]
    pub fn new(state: State, now: u64) -> Self {
        Self {
            selected: TaskId::SyncSpotify,
            popups: Vec::new(),
            state,
            link: None,
            device_name: None,
            device_ejected: false,
            viewport: (80, 24),
            throbber: ThrobberState::default(),
            now,
            spotify: TaskView::default(),
            download: TaskView::default(),
            convert: TaskView::default(),
            device: TaskView::default(),
            quit: false,
        }
    }

    /// What the screen keeps for `task`.
    #[must_use]
    pub fn view(&self, task: TaskId) -> &TaskView {
        match task {
            TaskId::SyncSpotify => &self.spotify,
            TaskId::DownloadLink => &self.download,
            TaskId::Convert => &self.convert,
            TaskId::SyncDevice => &self.device,
        }
    }

    fn view_mut(&mut self, task: TaskId) -> &mut TaskView {
        match task {
            TaskId::SyncSpotify => &mut self.spotify,
            TaskId::DownloadLink => &mut self.download,
            TaskId::Convert => &mut self.convert,
            TaskId::SyncDevice => &mut self.device,
        }
    }

    /// Functions that run now, in panel order. The running panel shows while this is not empty.
    #[must_use]
    pub fn running(&self) -> Vec<TaskId> {
        FUNCTIONS
            .into_iter()
            .filter(|t| self.view(*t).phase == Phase::Running)
            .collect()
    }

    /// True once the owner has quit.
    #[must_use]
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Take the state the task runner keeps (it saves the owner's answers before replying to them,
    /// and each run's last-run line before the run's terminal event).
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Time passes: the spinner turns while any function is busy, and running tasks age.
    pub fn tick(&mut self, dt: Duration, now: u64) {
        self.now = now;
        if FUNCTIONS
            .iter()
            .any(|task| self.view(*task).phase != Phase::Idle)
        {
            self.throbber.calc_next();
        }
        for task in self.running() {
            let view = self.view_mut(task);
            view.elapsed = view.elapsed.saturating_add(dt);
        }
    }

    /// The popup on top, the one shown.
    #[must_use]
    pub fn popup(&self) -> Option<&Popup> {
        self.popups.last()
    }

    fn keep(&mut self, popup: Popup) -> Vec<Command> {
        self.popups.push(popup);
        Vec::new()
    }

    fn find_popup(&mut self, wanted: impl Fn(&Popup) -> bool) -> Option<&mut Popup> {
        self.popups.iter_mut().rev().find(|popup| wanted(popup))
    }

    // ---- keys ----

    /// A key was pressed.
    pub fn handle_key(&mut self, event: KeyEvent) -> Vec<Command> {
        if event.kind != KeyEventKind::Press {
            return Vec::new();
        }
        match self.popups.pop() {
            None => self.key_main(event),
            Some(popup) => self.key_popup(popup, event),
        }
    }

    /// Text was pasted. Only a step form takes it (its focused text row); pasted control
    /// characters, newlines included, are dropped, so a paste never submits.
    pub fn handle_paste(&mut self, text: &str) -> Vec<Command> {
        match self.popups.pop() {
            Some(Popup::Form(mut dialog)) => {
                let reply = dialog.paste(text);
                self.form_reply(dialog, reply)
            }
            Some(other) => {
                self.popups.push(other);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn key_main(&mut self, event: KeyEvent) -> Vec<Command> {
        if event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL) {
            return self.request_quit();
        }
        match event.code {
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Enter => self.begin(self.selected),
            KeyCode::Char('s') => {
                if self.view(self.selected).phase == Phase::Running {
                    self.popups.push(Popup::ConfirmStop(self.selected));
                }
                Vec::new()
            }
            KeyCode::Char('a') => {
                if !self.view(self.selected).state.attention.is_empty() {
                    self.popups.push(Popup::Attention {
                        task: self.selected,
                        cursor: 0,
                    });
                }
                Vec::new()
            }
            KeyCode::Char('?') => self.keep(Popup::Help { scroll: 0 }),
            _ => Vec::new(),
        }
    }

    fn move_selection(&mut self, down: bool) -> Vec<Command> {
        let at = FUNCTIONS
            .iter()
            .position(|t| *t == self.selected)
            .unwrap_or(0);
        let to = if down {
            at.saturating_add(1).min(FUNCTIONS.len().saturating_sub(1))
        } else {
            at.saturating_sub(1)
        };
        self.selected = FUNCTIONS.get(to).copied().unwrap_or(self.selected);
        Vec::new()
    }

    fn request_quit(&mut self) -> Vec<Command> {
        if self.running().is_empty() {
            self.quit()
        } else {
            self.keep(Popup::ConfirmQuit)
        }
    }

    /// Close the app: running tasks are stopped (their last-run line says so), then the rest, which
    /// only wait at their form, end without touching their last-run line.
    fn quit(&mut self) -> Vec<Command> {
        self.quit = true;
        let mut commands: Vec<Command> = self.running().into_iter().map(Command::Stop).collect();
        commands.push(Command::Quit);
        commands
    }

    /// Enter on a function: open its first step. The last run's counts and attention list stay
    /// until the new run starts, so cancelling the form loses nothing.
    fn begin(&mut self, task: TaskId) -> Vec<Command> {
        if self.view(task).phase != Phase::Idle {
            return Vec::new();
        }
        self.view_mut(task).phase = Phase::Preparing;
        let first = match task {
            TaskId::SyncSpotify => Popup::SignIn(SignInState::OpeningBrowser),
            TaskId::DownloadLink => Popup::Form(Dialog::link(&self.state)),
            TaskId::Convert | TaskId::SyncDevice => Popup::Looking(task),
        };
        self.popups.push(first);
        vec![Command::Start(task)]
    }

    fn reset(&mut self, task: TaskId, phase: Phase) {
        *self.view_mut(task) = TaskView {
            phase,
            ..TaskView::default()
        };
    }

    /// Give up on a function's waiting steps (its popup is already closed).
    fn cancel(&mut self, task: TaskId) -> Vec<Command> {
        self.view_mut(task).phase = Phase::Cancelling;
        vec![Command::Cancel(task)]
    }

    fn key_popup(&mut self, popup: Popup, event: KeyEvent) -> Vec<Command> {
        match popup {
            Popup::SignIn(shown) => {
                if event.code == KeyCode::Esc {
                    self.cancel(TaskId::SyncSpotify)
                } else {
                    self.keep(Popup::SignIn(shown))
                }
            }
            Popup::Looking(task) => {
                if event.code == KeyCode::Esc {
                    self.cancel(task)
                } else {
                    self.keep(Popup::Looking(task))
                }
            }
            Popup::Form(mut dialog) => {
                let reply = dialog.key(event);
                self.form_reply(dialog, reply)
            }
            Popup::EjectConfirm(name) => match answer(event) {
                Some(true) => {
                    self.view_mut(TaskId::SyncDevice).phase = Phase::Preparing;
                    self.keep(Popup::Ejecting);
                    vec![Command::Eject]
                }
                Some(false) => Vec::new(),
                None => self.keep(Popup::EjectConfirm(name)),
            },
            Popup::Ejecting => self.keep(Popup::Ejecting),
            Popup::Attention { task, cursor } => {
                let len = self.view(task).state.attention.len();
                self.popups
                    .extend(scrolled(event, cursor, len, |cursor| Popup::Attention {
                        task,
                        cursor,
                    }));
                Vec::new()
            }
            Popup::ConfirmStop(task) => match answer(event) {
                Some(true) if self.view(task).phase == Phase::Running => {
                    vec![Command::Stop(task)]
                }
                Some(_) => Vec::new(),
                None => self.keep(Popup::ConfirmStop(task)),
            },
            Popup::ConfirmQuit => match answer(event) {
                Some(true) => self.quit(),
                Some(false) => Vec::new(),
                None => self.keep(Popup::ConfirmQuit),
            },
            Popup::Error(task, info) => {
                // Every key the help names for closing a popup closes it.
                if matches!(
                    event.code,
                    KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')
                ) {
                    Vec::new()
                } else {
                    self.keep(Popup::Error(task, info))
                }
            }
            Popup::Help { scroll } => {
                // Scrolling stops at the last full page, so Up always moves at once.
                let last = popups::help_max_scroll(self.viewport.1);
                self.popups
                    .extend(scrolled(event, scroll, last.saturating_add(1), |scroll| {
                        Popup::Help { scroll }
                    }));
                Vec::new()
            }
        }
    }

    /// What a step form's reply means for the screen.
    fn form_reply(&mut self, dialog: Dialog, reply: Reply) -> Vec<Command> {
        match reply {
            Reply::Stay(commands) => {
                self.popups.push(Popup::Form(dialog));
                commands
            }
            Reply::Start(commands) => {
                let task = dialog.task();
                match (task, dialog.chosen_label()) {
                    (TaskId::DownloadLink, Some(link)) => self.link = Some(link),
                    (TaskId::SyncDevice, Some(name)) => {
                        self.device_name = Some(name);
                        self.device_ejected = false;
                    }
                    _ => {}
                }
                self.reset(task, Phase::Running);
                commands
            }
            Reply::Cancel => self.cancel(dialog.task()),
        }
    }

    /// The step form of `task`, wherever it is in the stack.
    fn dialog_of(&mut self, task: TaskId) -> Option<&mut Dialog> {
        self.popups.iter_mut().rev().find_map(|popup| match popup {
            Popup::Form(dialog) if dialog.task() == task => Some(dialog),
            _ => None,
        })
    }

    // ---- task events ----

    /// A task reported. Some reports ask for a command in return (the folders of a device).
    pub fn apply_event(&mut self, event: Event) -> Vec<Command> {
        let Event { task, kind } = event;
        let phase = self.view(task).phase;
        // After the owner left a waiting step, only the run's end still matters.
        if phase == Phase::Cancelling && !ends_run(&kind) {
            return Vec::new();
        }
        match kind {
            EventKind::SignIn(sign_in) => {
                if let Some(Popup::SignIn(shown)) =
                    self.find_popup(|p| matches!(p, Popup::SignIn(_)))
                {
                    *shown = sign_in;
                }
            }
            EventKind::Lists(choices) => {
                let form = Popup::Form(Dialog::spotify(choices, &self.state));
                match self.find_popup(|p| matches!(p, Popup::SignIn(_))) {
                    Some(shown) => *shown = form,
                    None if phase == Phase::Preparing => self.popups.push(form),
                    None => {}
                }
            }
            EventKind::Devices(found) => return self.devices_found(found, phase),
            EventKind::Folders(folders) if task == TaskId::Convert => {
                let form = Popup::Form(Dialog::convert(&folders, &self.state));
                match self.find_popup(|p| matches!(p, Popup::Looking(TaskId::Convert))) {
                    Some(shown) => *shown = form,
                    None if phase == Phase::Preparing => self.popups.push(form),
                    None => {}
                }
            }
            EventKind::Folders(_) | EventKind::NewOfTotal { .. } | EventKind::Plan(_) => {
                if let Some(dialog) = self.dialog_of(task) {
                    return dialog.apply(&kind);
                }
            }
            EventKind::Headline(_) | EventKind::Counts(_) | EventKind::Attention(_) => {
                // A form waiting on its task shows what the task is doing (listing pages, say).
                if let Some(dialog) = self.dialog_of(task) {
                    dialog.apply(&kind);
                }
                self.view_mut(task).state.apply(&kind);
            }
            EventKind::Finished => {
                let ejected = task == TaskId::SyncDevice
                    && self.popups.iter().any(|p| matches!(p, Popup::Ejecting));
                self.ended(task);
                if ejected {
                    self.device_ejected = true;
                } else if task == TaskId::SyncDevice && phase == Phase::Running {
                    let name = self
                        .device_name
                        .clone()
                        .unwrap_or_else(|| "the device".to_owned());
                    self.popups.push(Popup::EjectConfirm(name));
                }
            }
            EventKind::Stopped => self.ended(task),
            // An error is always shown, also one that arrives after its run ended.
            EventKind::Error(info) => {
                self.ended(task);
                self.popups.push(Popup::Error(task, info));
            }
            EventKind::RateLimited { wait } => {
                self.ended(task);
                self.popups.push(Popup::Error(task, rate_limit_info(wait)));
            }
        }
        Vec::new()
    }

    /// Devices were listed: the first time they open the form, later they refresh it.
    fn devices_found(&mut self, found: Vec<sdl_core::Device>, phase: Phase) -> Vec<Command> {
        if let Some(dialog) = self.dialog_of(TaskId::SyncDevice) {
            return dialog.apply(&EventKind::Devices(found));
        }
        let (dialog, commands) = Dialog::device(found, &self.state);
        let form = Popup::Form(dialog);
        match self.find_popup(|p| matches!(p, Popup::Looking(TaskId::SyncDevice))) {
            Some(shown) => *shown = form,
            None if phase == Phase::Preparing => self.popups.push(form),
            None => return Vec::new(),
        }
        commands
    }

    /// A run ended: its popups close. Its last-run line is already in the runner's state.
    fn ended(&mut self, task: TaskId) {
        self.view_mut(task).phase = Phase::Idle;
        self.popups.retain(|popup| popup.owner() != Some(task));
    }

    // ---- what the screen shows ----

    /// The selection shown in a function's row.
    #[must_use]
    pub fn selection_text(&self, task: TaskId) -> String {
        match task {
            TaskId::SyncSpotify => spotify_selection(&self.state.ticked_lists),
            TaskId::DownloadLink => self
                .link
                .clone()
                .unwrap_or_else(|| "no link yet".to_owned()),
            TaskId::Convert => format!(
                "{} to {}",
                self.state.convert.format.label(),
                self.state.convert.destination.display()
            ),
            TaskId::SyncDevice => match &self.device_name {
                Some(name) if self.device_ejected => format!("{name} · ejected, safe to unplug"),
                Some(name) => name.clone(),
                None => "no device chosen yet".to_owned(),
            },
        }
    }

    /// Keys valid right now, for the footer and the popup hint line.
    #[must_use]
    pub fn hints(&self) -> Vec<Hint> {
        match self.popup() {
            None => {
                let mut hints = vec![keys::CHOOSE.hint()];
                let view = self.view(self.selected);
                if view.phase == Phase::Idle {
                    hints.push(keys::RUN.hint());
                }
                if view.phase == Phase::Running {
                    hints.push(keys::STOP.hint());
                }
                if !view.state.attention.is_empty() {
                    hints.push(keys::ATTENTION.hint());
                }
                hints.extend([keys::HELP_KEY.hint(), keys::QUIT.hint()]);
                hints
            }
            Some(Popup::SignIn(SignInState::Failed(_)) | Popup::Error(..)) => {
                vec![keys::CLOSE.hint()]
            }
            Some(Popup::SignIn(_) | Popup::Looking(_)) => vec![keys::CANCEL.hint()],
            Some(Popup::Form(dialog)) => dialog.hints(),
            Some(Popup::EjectConfirm(_) | Popup::ConfirmStop(_) | Popup::ConfirmQuit) => {
                vec![keys::YES.hint(), keys::NO.hint()]
            }
            Some(Popup::Ejecting) => Vec::new(),
            Some(Popup::Attention { .. } | Popup::Help { .. }) => {
                vec![keys::SCROLL.hint(), keys::CLOSE.hint()]
            }
        }
    }

    /// The terminal's size, so scrolling can stop where the screen shows the end.
    pub fn set_viewport(&mut self, width: u16, height: u16) {
        self.viewport = (width, height);
        let last = popups::help_max_scroll(height);
        for popup in &mut self.popups {
            if let Popup::Help { scroll } = popup {
                *scroll = (*scroll).min(last);
            }
        }
    }
}

/// Scroll keys for a list of `len` rows; `None` closes the popup.
fn scrolled(
    event: KeyEvent,
    at: usize,
    len: usize,
    rebuild: impl FnOnce(usize) -> Popup,
) -> Option<Popup> {
    let last = len.saturating_sub(1);
    let to = match event.code {
        KeyCode::Esc | KeyCode::Char('q') => return None,
        KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => at.saturating_add(1),
        KeyCode::PageUp => at.saturating_sub(PAGE),
        KeyCode::PageDown => at.saturating_add(PAGE),
        KeyCode::Home => 0,
        KeyCode::End => last,
        _ => at,
    };
    Some(rebuild(to.min(last)))
}

fn spotify_selection(ticked: &[String]) -> String {
    let liked = ticked
        .iter()
        .any(|id| id == sdl_core::event::LIKED_SONGS_ID);
    let playlists = ticked.len().saturating_sub(usize::from(liked));
    let noun = if playlists == 1 {
        "playlist"
    } else {
        "playlists"
    };
    match (liked, playlists) {
        (false, 0) => "no lists chosen yet".to_owned(),
        (true, 0) => "Liked Songs".to_owned(),
        (false, n) => format!("{n} {noun}"),
        (true, n) => format!("Liked Songs and {n} {noun}"),
    }
}

/// Whether an event ends a run.
fn ends_run(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Finished
            | EventKind::Stopped
            | EventKind::Error(_)
            | EventKind::RateLimited { .. }
    )
}

fn rate_limit_info(wait: Option<Duration>) -> ErrorInfo {
    let wait = wait.map_or_else(
        || "Wait unknown.".to_owned(),
        |d| format!("Spotify asks you to wait {}.", fmt::duration_text(d)),
    );
    ErrorInfo {
        message: format!("Spotify says too many requests were made, so the sync stopped. {wait}"),
        advice: "Run Sync Spotify again after the wait.".to_owned(),
    }
}

/// How a last-run line is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Finished cleanly.
    Done,
    /// Finished with items that need attention, or stopped by a rate limit.
    Attention,
    /// Stopped by the owner.
    Stopped,
    /// Failed.
    Failed,
}

/// The tone of a last-run line, from the outcome saved with it.
#[must_use]
pub fn tone(outcome: RunOutcome) -> Tone {
    match outcome {
        RunOutcome::Done => Tone::Done,
        RunOutcome::NeedsAttention => Tone::Attention,
        RunOutcome::Stopped => Tone::Stopped,
        RunOutcome::Failed => Tone::Failed,
    }
}

#[cfg(test)]
mod tests;
