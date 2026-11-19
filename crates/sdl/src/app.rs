//! Screen state (F1, F8). State changes only from key events, paste events, task events and ticks;
//! nothing here draws. Every change that needs the outside world is returned as a [`Command`].

use std::time::Duration;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sdl_core::{
    CardSummary, ErrorInfo, Event, EventKind, ListChoice, SignInState, State, TaskId, TaskState,
};
use throbber_widgets_tui::ThrobberState;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler as _;

use crate::control::Command;
use crate::fmt;
use crate::keys;

/// The functions, in the order of the main panel.
pub const FUNCTIONS: [TaskId; 3] = [TaskId::SyncSpotify, TaskId::DownloadLink, TaskId::SyncCard];

const PAGE: usize = 10;

pub use crate::tasks::task_name;

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

/// Progress of the Spotify list popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStage {
    /// Ticking lists.
    Choosing,
    /// The ticked lists' songs are being listed.
    Listing,
    /// `N new of M`, waiting for Enter.
    Ready {
        /// Songs not yet in the library.
        new: usize,
        /// Songs in the ticked lists.
        total: usize,
    },
}

/// The Spotify list-selection popup.
#[derive(Debug, Clone)]
pub struct ListPopup {
    /// Lists on offer.
    pub choices: Vec<ListChoice>,
    /// Tick per list.
    pub ticked: Vec<bool>,
    /// Row under the cursor.
    pub cursor: usize,
    /// Progress.
    pub stage: ListStage,
}

impl ListPopup {
    /// The ticked lists.
    #[must_use]
    pub fn chosen(&self) -> Vec<ListChoice> {
        self.choices
            .iter()
            .zip(&self.ticked)
            .filter(|(_, on)| **on)
            .map(|(choice, _)| choice.clone())
            .collect()
    }

    /// Songs in the ticked lists, as counted by Spotify.
    #[must_use]
    pub fn ticked_songs(&self) -> usize {
        self.chosen()
            .iter()
            .map(|c| c.count)
            .fold(0, usize::saturating_add)
    }
}

/// A centred popup.
#[derive(Debug, Clone)]
pub enum Popup {
    /// Spotify sign-in state.
    SignIn(SignInState),
    /// Spotify list selection and the `N new of M` confirmation.
    Lists(ListPopup),
    /// The link text field.
    Link(Input),
    /// Card plan; `None` while the card is being mounted.
    CardPlan(Option<CardSummary>),
    /// "Clean up and unmount?"
    UnmountConfirm,
    /// The card is being cleaned and unmounted.
    Unmounting,
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
            Self::SignIn(_) | Self::Lists(_) => Some(TaskId::SyncSpotify),
            Self::CardPlan(_) | Self::UnmountConfirm | Self::Unmounting => Some(TaskId::SyncCard),
            Self::ConfirmStop(task) => Some(*task),
            Self::Link(_)
            | Self::Attention { .. }
            | Self::ConfirmQuit
            | Self::Error(..)
            | Self::Help { .. } => None,
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
    /// The card, as shown in its row.
    pub card_label: String,
    /// The last link given this session.
    pub link: Option<String>,
    /// Spinner position.
    pub throbber: ThrobberState,
    /// Seconds since the Unix epoch, as of the last tick.
    pub now: u64,
    spotify: TaskView,
    download: TaskView,
    card: TaskView,
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
    pub fn new(state: State, card_label: String, now: u64) -> Self {
        Self {
            selected: TaskId::SyncSpotify,
            popups: Vec::new(),
            state,
            card_label,
            link: None,
            throbber: ThrobberState::default(),
            now,
            spotify: TaskView::default(),
            download: TaskView::default(),
            card: TaskView::default(),
            quit: false,
        }
    }

    /// What the screen keeps for `task`.
    #[must_use]
    pub fn view(&self, task: TaskId) -> &TaskView {
        match task {
            TaskId::SyncSpotify => &self.spotify,
            TaskId::DownloadLink => &self.download,
            TaskId::SyncCard => &self.card,
        }
    }

    fn view_mut(&mut self, task: TaskId) -> &mut TaskView {
        match task {
            TaskId::SyncSpotify => &mut self.spotify,
            TaskId::DownloadLink => &mut self.download,
            TaskId::SyncCard => &mut self.card,
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

    /// Take the state the task runner keeps (it writes each run's last-run line before the run's
    /// terminal event).
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

    /// Text was pasted. Only the link field takes it, and pasted control characters, newlines
    /// included, are dropped.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(Popup::Link(input)) = self.popups.last_mut() {
            for c in text.chars().filter(|c| !c.is_control()) {
                input.handle(tui_input::InputRequest::InsertChar(c));
            }
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
            self.quit = true;
            vec![Command::Quit]
        } else {
            self.keep(Popup::ConfirmQuit)
        }
    }

    /// Enter on a function: open its first step.
    fn begin(&mut self, task: TaskId) -> Vec<Command> {
        if self.view(task).phase != Phase::Idle {
            return Vec::new();
        }
        match task {
            TaskId::SyncSpotify => {
                self.reset(task, Phase::Preparing);
                self.popups.push(Popup::SignIn(SignInState::OpeningBrowser));
                vec![Command::StartSpotify]
            }
            TaskId::DownloadLink => self.keep(Popup::Link(Input::default())),
            TaskId::SyncCard => {
                self.reset(task, Phase::Preparing);
                self.popups.push(Popup::CardPlan(None));
                vec![Command::StartCard]
            }
        }
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
            Popup::Lists(list) => self.key_lists(list, event),
            Popup::Link(input) => self.key_link(input, event),
            Popup::CardPlan(plan) => self.key_card_plan(plan, event),
            Popup::UnmountConfirm => match answer(event) {
                Some(true) => {
                    self.view_mut(TaskId::SyncCard).phase = Phase::Preparing;
                    self.keep(Popup::Unmounting);
                    vec![Command::FinishCard]
                }
                Some(false) => Vec::new(),
                None => self.keep(Popup::UnmountConfirm),
            },
            Popup::Unmounting => self.keep(Popup::Unmounting),
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
                Some(true) => {
                    self.quit = true;
                    vec![Command::Quit]
                }
                Some(false) => Vec::new(),
                None => self.keep(Popup::ConfirmQuit),
            },
            Popup::Error(task, info) => {
                if matches!(event.code, KeyCode::Enter | KeyCode::Esc) {
                    Vec::new()
                } else {
                    self.keep(Popup::Error(task, info))
                }
            }
            Popup::Help { scroll } => {
                self.popups
                    .extend(scrolled(event, scroll, keys::help_line_count(), |scroll| {
                        Popup::Help { scroll }
                    }));
                Vec::new()
            }
        }
    }

    fn key_card_plan(&mut self, plan: Option<CardSummary>, event: KeyEvent) -> Vec<Command> {
        match (event.code, plan) {
            (KeyCode::Esc, _) => self.cancel(TaskId::SyncCard),
            (KeyCode::Enter, Some(_)) => {
                self.reset(TaskId::SyncCard, Phase::Running);
                vec![Command::Go(TaskId::SyncCard)]
            }
            (_, plan) => self.keep(Popup::CardPlan(plan)),
        }
    }

    fn key_lists(&mut self, mut list: ListPopup, event: KeyEvent) -> Vec<Command> {
        if event.code == KeyCode::Esc {
            return self.cancel(TaskId::SyncSpotify);
        }
        let mut commands = Vec::new();
        match (list.stage, event.code) {
            (ListStage::Choosing, KeyCode::Up | KeyCode::Char('k')) => {
                list.cursor = list.cursor.saturating_sub(1);
            }
            (ListStage::Choosing, KeyCode::Down | KeyCode::Char('j')) => {
                list.cursor = list
                    .cursor
                    .saturating_add(1)
                    .min(list.choices.len().saturating_sub(1));
            }
            (ListStage::Choosing, KeyCode::Char(' ')) => {
                if let Some(on) = list.ticked.get_mut(list.cursor) {
                    *on = !*on;
                }
            }
            (ListStage::Choosing, KeyCode::Enter) => {
                let chosen = list.chosen();
                if !chosen.is_empty() {
                    list.stage = ListStage::Listing;
                    commands.push(Command::Choose(chosen));
                }
            }
            (ListStage::Ready { .. }, KeyCode::Enter) => {
                self.reset(TaskId::SyncSpotify, Phase::Running);
                return vec![Command::Go(TaskId::SyncSpotify)];
            }
            _ => {}
        }
        self.popups.push(Popup::Lists(list));
        commands
    }

    fn key_link(&mut self, mut input: Input, event: KeyEvent) -> Vec<Command> {
        match event.code {
            KeyCode::Esc => Vec::new(),
            KeyCode::Enter => {
                let url = input.value().trim().to_owned();
                if url.is_empty() {
                    return self.keep(Popup::Link(input));
                }
                self.link = Some(url.clone());
                self.reset(TaskId::DownloadLink, Phase::Running);
                vec![Command::StartLink(url)]
            }
            _ => {
                input.handle_event(&TermEvent::Key(event));
                self.keep(Popup::Link(input))
            }
        }
    }

    // ---- task events ----

    /// A task reported.
    pub fn apply_event(&mut self, event: Event) {
        let Event { task, kind } = event;
        let phase = self.view(task).phase;
        // After the owner left a waiting step, only the run's end still matters.
        if phase == Phase::Cancelling && !ends_run(&kind) {
            return;
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
                let list = Popup::Lists(self.list_popup(choices));
                match self.find_popup(|p| matches!(p, Popup::SignIn(_))) {
                    Some(shown) => *shown = list,
                    None if phase == Phase::Preparing => self.popups.push(list),
                    None => {}
                }
            }
            EventKind::NewOfTotal { new, total } => {
                if let Some(Popup::Lists(list)) = self.find_popup(|p| matches!(p, Popup::Lists(_)))
                {
                    list.stage = ListStage::Ready { new, total };
                }
            }
            EventKind::CardSummary(summary) => {
                if let Some(plan) = self.find_popup(|p| matches!(p, Popup::CardPlan(None))) {
                    *plan = Popup::CardPlan(Some(summary));
                }
            }
            EventKind::Headline(_) | EventKind::Counts(_) | EventKind::Attention(_) => {
                self.view_mut(task).state.apply(&kind);
            }
            EventKind::Finished => {
                self.ended(task);
                if task == TaskId::SyncCard && phase == Phase::Running {
                    self.popups.push(Popup::UnmountConfirm);
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
    }

    fn list_popup(&self, choices: Vec<ListChoice>) -> ListPopup {
        let ticked = choices
            .iter()
            .map(|c| self.state.ticked_lists.contains(&c.id))
            .collect();
        ListPopup {
            choices,
            ticked,
            cursor: 0,
            stage: ListStage::Choosing,
        }
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
            TaskId::SyncCard => self.card_label.clone(),
        }
    }

    /// Keys valid right now, for the footer and the popup hint line.
    #[must_use]
    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.popup() {
            None => {
                let mut hints = vec![("↑↓", "choose")];
                let view = self.view(self.selected);
                if view.phase == Phase::Idle {
                    hints.push(("Enter", "run"));
                }
                if view.phase == Phase::Running {
                    hints.push(("s", "stop"));
                }
                if !view.state.attention.is_empty() {
                    hints.push(("a", "attention"));
                }
                hints.extend([("?", "help"), ("q", "quit")]);
                hints
            }
            Some(Popup::SignIn(SignInState::Failed(_)) | Popup::Error(..)) => {
                vec![("Esc", "close")]
            }
            Some(Popup::SignIn(_)) => vec![("Esc", "cancel")],
            Some(Popup::CardPlan(None)) => vec![("Esc", "unmount and cancel")],
            Some(Popup::Lists(list)) => match list.stage {
                ListStage::Choosing => {
                    let mut hints = vec![("↑↓", "move"), ("Space", "tick")];
                    if list.ticked.contains(&true) {
                        hints.push(("Enter", "next"));
                    }
                    hints.push(("Esc", "cancel"));
                    hints
                }
                ListStage::Listing => vec![("Esc", "cancel")],
                ListStage::Ready { .. } => vec![("Enter", "start"), ("Esc", "cancel")],
            },
            Some(Popup::Link(input)) if input.value().trim().is_empty() => {
                vec![("Esc", "cancel")]
            }
            Some(Popup::Link(_)) => vec![("Enter", "start"), ("Esc", "cancel")],
            Some(Popup::CardPlan(Some(_))) => {
                vec![("Enter", "start"), ("Esc", "unmount and cancel")]
            }
            Some(Popup::UnmountConfirm | Popup::ConfirmStop(_) | Popup::ConfirmQuit) => {
                vec![("y", "yes"), ("n", "no")]
            }
            Some(Popup::Unmounting) => Vec::new(),
            Some(Popup::Attention { .. } | Popup::Help { .. }) => {
                vec![("↑↓", "scroll"), ("Esc", "close")]
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

/// Whether an event ends a run: its last-run line is then in the runner's state.
#[must_use]
pub fn ends_run(kind: &EventKind) -> bool {
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

/// The tone of a last-run line written by this module (or loaded from the state file).
#[must_use]
pub fn tone(line: &str) -> Tone {
    if line.starts_with("failed") {
        Tone::Failed
    } else if line.starts_with("stopped: Spotify") || line.contains("need attention") {
        Tone::Attention
    } else if line.starts_with("stopped") {
        Tone::Stopped
    } else {
        Tone::Done
    }
}

#[cfg(test)]
mod tests;
