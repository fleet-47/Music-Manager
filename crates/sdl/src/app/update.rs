//! `App::update`: the one place a [`Msg`] changes the app and asks for [`Cmd`]s.
//!
//! First pass, enough for the main screen: selecting, opening a function's dialog, closing
//! popups, folding task events, errors, quit. The screen engineer completes it:
//!
//! * **Keys** (every key is also on the help popup; keep `screen::keys` the one table): Up, Down,
//!   `j`, `k` select; Enter runs the selected function (opens its dialog) unless it is running;
//!   `s` asks `Confirm::Stop` for a running function; `a` opens the attention list of the
//!   selected function (the current run's items while it runs, otherwise the saved last run's);
//!   `?` opens help; `q` and Ctrl-C quit: with a function running, `Confirm::Quit` first, then
//!   `Stop` for every running one, `Discard` for every one still at its form, then `Quit`. Esc
//!   closes the top popup (a dialog sends `Cancel` through its own reaction; a `Waiting::Browser`
//!   sends `Cancel`).
//! * **Task events** (`Msg::Task`): fold into the view, route to the function's dialog, flash the
//!   row when a run ended ([`App::ended`]). `SignIn(NeedsBrowser)` pushes `Confirm::Browser` (Yes
//!   sends `Go`, No sends `Cancel` and closes the dialog); `SignIn(WaitingForBrowser)` pushes
//!   `Waiting::Browser`; `SignIn(SignedIn)` pops it; `SignIn(KeychainProblem)` shows its reason
//!   once as an error popup that does not end the task. `Error` closes the function's dialog and
//!   pushes `Popup::Error`; `RateLimited` does the same with the wait from `Retry-After` ("Spotify
//!   asks you to wait about 5 minutes", or "wait unknown"). `Finished` of Sync device pushes
//!   `Confirm::Eject` naming the device (its clean-up also empties the device's Trash); Yes
//!   sends `Cmd::Eject` and shows `Waiting::Eject` until `Msg::Ejected`.
//! * **Mouse**: tested against [`App::hits`], topmost first: a popup's own targets (`Hits::popup`
//!   or, for a dialog, `Dialog::mouse` with `Hits::form`), else the main screen's (`Hits::main`:
//!   a click selects a row, a second click runs it); the wheel scrolls a list under it. A click
//!   outside a popup does nothing. Every mouse action has a key.
//! * **Wake**: advance the spinner of the top dialog's form ([`sdl_form::Form::tick`]).
//! * **Plain popups** (`screen::popups`): `y`/Enter answers Yes and `n` No on a confirmation; Esc
//!   and `q` close the rest. `Waiting::Browser`'s Cancel (button or Esc) sends `Cancel` and closes
//!   the Sync Spotify dialog under it. Help and the attention list scroll with Up, Down, Page Up,
//!   Page Down, Home, End and the wheel, and have a `Close` button like every popup except
//!   `Waiting::Eject`, so a mouse-only walk can always leave. Attention reasons wrap to the popup
//!   width (`Paragraph` with `Wrap`; the scroll length is the wrapped line count).
//! * **Effects**: a popup opening fades in, the running panel slides in when the first task
//!   starts, and each row in [`App::take_ended`] flashes once; none when `App::animations` is off.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sdl_core::{EventKind, TaskId};

use super::{App, Clock, Cmd, Confirm, Msg, Popup};
use crate::dialogs::{Dialog, Step};

impl App {
    /// Apply one message at time `clock`; the commands are for the loop to run, in order.
    pub fn update(&mut self, msg: Msg, clock: Clock) -> Vec<Cmd> {
        self.clock = clock;
        match msg {
            Msg::Resize(size) => {
                self.size = size;
                Vec::new()
            }
            Msg::Settings(settings) => {
                self.settings = *settings;
                Vec::new()
            }
            Msg::Task(event) => self.on_task(event.task, &event.kind),
            Msg::Key(key) => self.on_key(key),
            Msg::Paste(text) => self.on_paste(&text),
            Msg::Mouse(_) => Vec::new(),
            Msg::Ejected(result) => {
                self.popups
                    .retain(|popup| !matches!(popup, Popup::Waiting(_)));
                if let Err(info) = result {
                    self.popups.push(Popup::Error(info));
                }
                Vec::new()
            }
            Msg::Wake => {
                self.spin = self.spin.wrapping_add(1);
                if let Some(Popup::Dialog(dialog)) = self.popups.last_mut() {
                    dialog.form_mut().tick();
                }
                Vec::new()
            }
        }
    }

    fn on_task(&mut self, task: TaskId, kind: &EventKind) -> Vec<Cmd> {
        let now = self.clock.instant;
        let ended = self
            .view_mut(task)
            .is_some_and(|view| view.apply(kind, now));
        if ended {
            self.ended.push(task);
        }
        if let EventKind::Error(info) = kind {
            self.close_dialog(task);
            self.popups.push(Popup::Error(info.clone()));
            return Vec::new();
        }
        let settings = &self.settings;
        let mut commands = Vec::new();
        let mut done = false;
        for popup in &mut self.popups {
            if let Popup::Dialog(dialog) = popup
                && dialog.task() == task
            {
                match dialog.on_event(kind, settings) {
                    Step::Stay(more) => commands.extend(more),
                    Step::Close(more) => {
                        commands.extend(more);
                        done = true;
                    }
                }
            }
        }
        if done {
            self.close_dialog(task);
        }
        commands
    }

    fn close_dialog(&mut self, task: TaskId) {
        self.popups.retain(|popup| match popup {
            Popup::Dialog(dialog) => dialog.task() != task,
            _ => true,
        });
    }

    fn on_paste(&mut self, text: &str) -> Vec<Cmd> {
        let settings = &self.settings;
        match self.popups.last_mut() {
            Some(Popup::Dialog(dialog)) => match dialog.paste(text, settings) {
                Step::Stay(commands) | Step::Close(commands) => commands,
            },
            _ => Vec::new(),
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        let control_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if control_c {
            return self.quit();
        }
        match self.popups.last_mut() {
            Some(Popup::Dialog(dialog)) => {
                let task = dialog.task();
                let step = dialog.key(key, &self.settings);
                match step {
                    Step::Stay(commands) => commands,
                    Step::Close(commands) => {
                        self.close_dialog(task);
                        commands
                    }
                }
            }
            Some(_) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                    self.popups.pop();
                }
                Vec::new()
            }
            None => self.on_main_key(key),
        }
    }

    fn on_main_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        match key.code {
            KeyCode::Char('q') => self.quit(),
            KeyCode::Char('?') => {
                self.popups.push(Popup::Help { scroll: 0 });
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => self.select(-1),
            KeyCode::Down | KeyCode::Char('j') => self.select(1),
            KeyCode::Enter => self.run_selected(),
            _ => Vec::new(),
        }
    }

    fn select(&mut self, step: isize) -> Vec<Cmd> {
        let at = TaskId::ALL
            .iter()
            .position(|task| *task == self.selected)
            .unwrap_or(0);
        let last = TaskId::ALL.len().saturating_sub(1);
        let to = at.saturating_add_signed(step).min(last);
        if let Some(task) = TaskId::ALL.get(to) {
            self.selected = *task;
        }
        Vec::new()
    }

    fn run_selected(&mut self) -> Vec<Cmd> {
        let task = self.selected;
        if self.view(task).is_none_or(super::FunctionView::is_open) {
            return Vec::new();
        }
        if let Some(view) = self.view_mut(task) {
            view.begin();
        }
        let (dialog, commands) = Dialog::open(task, &self.settings);
        self.popups.push(Popup::Dialog(Box::new(dialog)));
        commands
    }

    /// Quit now when nothing runs; otherwise ask first.
    fn quit(&mut self) -> Vec<Cmd> {
        if self.shows_running_panel() {
            if !matches!(self.top(), Some(Popup::Confirm(Confirm::Quit))) {
                self.popups.push(Popup::Confirm(Confirm::Quit));
            }
            return Vec::new();
        }
        self.quit = true;
        let mut commands: Vec<Cmd> = TaskId::ALL
            .into_iter()
            .filter(|task| self.view(*task).is_some_and(super::FunctionView::is_open))
            .map(Cmd::Discard)
            .collect();
        commands.push(Cmd::Quit);
        commands
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crossterm::event::KeyEventState;
    use ratatui::layout::Size;
    use sdl_core::Event;

    use super::*;
    use crate::app::Settings;

    fn clock() -> Clock {
        Clock {
            instant: Instant::now(),
            epoch: 0,
        }
    }

    fn app() -> App {
        App::new(Settings::default(), true, Size::new(100, 30), clock())
    }

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn q_with_nothing_running_quits_at_once() {
        let mut app = app();
        let commands = app.update(key(KeyCode::Char('q')), clock());
        assert_eq!(commands, vec![Cmd::Quit]);
        assert!(app.quit);
    }

    #[test]
    fn q_while_a_function_runs_asks_first() {
        let mut app = app();
        let now = clock();
        let started = Event {
            task: TaskId::Convert,
            kind: EventKind::Started,
        };
        if let Some(view) = app.view_mut(TaskId::Convert) {
            view.begin();
        }
        let _ = app.update(Msg::Task(started), now);
        assert_eq!(app.update(key(KeyCode::Char('q')), now), Vec::<Cmd>::new());
        assert!(matches!(app.top(), Some(Popup::Confirm(Confirm::Quit))));
        assert!(!app.quit);
    }

    #[test]
    fn enter_opens_the_selected_functions_dialog_and_starts_its_task() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Down), clock());
        assert_eq!(app.selected, TaskId::DownloadLink);
        let commands = app.update(key(KeyCode::Enter), clock());
        assert_eq!(commands, vec![Cmd::Start(TaskId::DownloadLink)]);
        assert!(matches!(app.top(), Some(Popup::Dialog(_))));
        // Opening it again does nothing: the same function is not started twice.
        assert_eq!(app.update(key(KeyCode::Enter), clock()), Vec::<Cmd>::new());
    }

    #[test]
    fn an_error_closes_the_dialog_and_says_what_to_do() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Enter), clock());
        let event = Event {
            task: TaskId::SyncSpotify,
            kind: EventKind::Error(sdl_core::ErrorInfo {
                message: "It broke".into(),
                advice: "Try again".into(),
            }),
        };
        let _ = app.update(Msg::Task(event), clock());
        assert!(matches!(app.top(), Some(Popup::Error(_))));
        assert_eq!(app.popups.len(), 1);
    }
}
