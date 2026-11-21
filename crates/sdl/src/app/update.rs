//! `App::update`: the one place a [`Msg`] changes the app and asks for [`Cmd`]s.
//!
//! * **Keys** (the table is `screen::keys`, which the footer and the help read): Up, Down, `j`,
//!   `k` choose a function; Enter runs the chosen one (opens its dialog) unless it is open; `s`
//!   asks to stop a running one; `a` opens the attention list; `?` opens help; `q` and Ctrl-C
//!   quit, asking first while a function runs. A popup on top takes every key.
//! * **Task events** fold into the function's view and go to its dialog. The ending events
//!   (`Finished`, `Stopped`, `Error`, `RateLimited`) close the dialog and are the app's own: an
//!   error or a rate limit becomes an error popup, a finished device sync asks to eject, and
//!   Spotify's sign-in events become the browser question and the waiting popup.
//! * **Mouse** is tested against the rectangles the last frame drew ([`App::hits`]); see
//!   `update/mouse.rs`. **Plain popups** (questions, errors, help, the attention list) are in
//!   `update/popups.rs`.

mod mouse;
mod popups;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use sdl_core::{ErrorInfo, EventKind, SignInState, TaskId};
use sdl_tasks::Command;

use super::{App, Clock, Cmd, Confirm, FunctionView, Msg, Phase, Popup, Waiting};
use crate::dialogs::{Dialog, Step};
use crate::fmt::duration_text;

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
            Msg::Mouse(event) => self.on_mouse(event),
            Msg::Ejected(result) => {
                self.popups
                    .retain(|popup| !matches!(popup, Popup::Waiting(Waiting::Eject(_))));
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
        if self
            .view_mut(task)
            .is_some_and(|view| view.apply(kind, now))
        {
            self.ended.push(task);
        }
        match kind {
            EventKind::Error(info) => {
                self.finish(task);
                self.popups.push(Popup::Error(info.clone()));
                return Vec::new();
            }
            EventKind::RateLimited { wait } => {
                self.finish(task);
                self.popups.push(Popup::Error(rate_limit(*wait)));
                return Vec::new();
            }
            EventKind::Finished => {
                self.finish(task);
                if task == TaskId::SyncDevice {
                    let device = self.synced_device();
                    self.popups.push(Popup::Confirm(Confirm::Eject { device }));
                }
                return Vec::new();
            }
            EventKind::Stopped => {
                self.finish(task);
                return Vec::new();
            }
            EventKind::Devices(devices) => self.devices.clone_from(devices),
            EventKind::SignIn(state) => {
                let app_only = self.on_sign_in(state);
                if app_only {
                    return Vec::new();
                }
            }
            _ => {}
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

    /// A sign-in event; `true` when it was the app's alone (the dialog does not see it).
    fn on_sign_in(&mut self, state: &SignInState) -> bool {
        match state {
            SignInState::NeedsBrowser => {
                self.drop_browser_popups();
                self.popups.push(Popup::Confirm(Confirm::Browser));
                true
            }
            SignInState::WaitingForBrowser => {
                self.drop_browser_popups();
                self.popups.push(Popup::Waiting(Waiting::Browser));
                true
            }
            SignInState::SignedIn => {
                self.drop_browser_popups();
                false
            }
            SignInState::KeychainProblem(reason) => {
                self.popups.push(Popup::Error(ErrorInfo {
                    message: reason.clone(),
                    advice: "The sync goes on. You may be asked to sign in again next time."
                        .to_owned(),
                }));
                true
            }
        }
    }

    fn drop_browser_popups(&mut self) {
        self.popups.retain(|popup| {
            !matches!(
                popup,
                Popup::Confirm(Confirm::Browser) | Popup::Waiting(Waiting::Browser)
            )
        });
    }

    /// The function's run is over: its dialog and its sign-in popups have nothing left to say.
    fn finish(&mut self, task: TaskId) {
        self.close_dialog(task);
        if task == TaskId::SyncSpotify {
            self.drop_browser_popups();
        }
    }

    fn close_dialog(&mut self, task: TaskId) {
        self.popups.retain(|popup| match popup {
            Popup::Dialog(dialog) => dialog.task() != task,
            _ => true,
        });
    }

    /// What a dialog's answer means for the app: stay, or close it, with its commands.
    fn apply_step(&mut self, task: TaskId, step: Step) -> Vec<Cmd> {
        match step {
            Step::Stay(commands) => commands,
            Step::Close(commands) => {
                self.close_dialog(task);
                commands
            }
        }
    }

    /// The name of the device the last Sync device run wrote to, for the eject question.
    fn synced_device(&self) -> String {
        self.settings
            .state
            .last_device
            .as_deref()
            .and_then(|id| self.devices.iter().find(|device| device.id() == id))
            .map_or_else(|| "the device".to_owned(), |device| device.name.clone())
    }

    fn on_paste(&mut self, text: &str) -> Vec<Cmd> {
        let settings = &self.settings;
        let step = match self.popups.last_mut() {
            Some(Popup::Dialog(dialog)) => Some((dialog.task(), dialog.paste(text, settings))),
            _ => None,
        };
        step.map_or_else(Vec::new, |(task, step)| self.apply_step(task, step))
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
        let step = match self.popups.last_mut() {
            Some(Popup::Dialog(dialog)) => Some((dialog.task(), dialog.key(key, &self.settings))),
            Some(_) => return self.on_popup_key(key),
            None => return self.on_main_key(key),
        };
        step.map_or_else(Vec::new, |(task, step)| self.apply_step(task, step))
    }

    fn on_main_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        match key.code {
            KeyCode::Char('q') => self.quit(),
            KeyCode::Char('?') => {
                self.popups.push(Popup::Help { scroll: 0 });
                Vec::new()
            }
            KeyCode::Char('a') => {
                self.popups.push(Popup::Attention {
                    task: self.selected,
                    scroll: 0,
                });
                Vec::new()
            }
            KeyCode::Char('s') => {
                let task = self.selected;
                if self.view(task).is_some_and(FunctionView::is_running) {
                    self.popups.push(Popup::Confirm(Confirm::Stop(task)));
                }
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => self.select_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.select_by(1),
            KeyCode::Enter => self.run_selected(),
            _ => Vec::new(),
        }
    }

    fn select_by(&mut self, step: isize) -> Vec<Cmd> {
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
        if self.view(task).is_none_or(FunctionView::is_open) {
            return Vec::new();
        }
        if let Some(view) = self.view_mut(task) {
            view.begin();
        }
        let (dialog, commands) = Dialog::open(task, &self.settings);
        self.popups.push(Popup::Dialog(Box::new(dialog)));
        commands
    }

    /// Ask the owner to quit while a function runs; otherwise quit now.
    fn quit(&mut self) -> Vec<Cmd> {
        if self.shows_running_panel() {
            if !matches!(self.top(), Some(Popup::Confirm(Confirm::Quit))) {
                self.popups.push(Popup::Confirm(Confirm::Quit));
            }
            return Vec::new();
        }
        self.quit_now()
    }

    /// Stop what runs, end what only waits at its form, and leave the loop.
    fn quit_now(&mut self) -> Vec<Cmd> {
        self.quit = true;
        let mut commands: Vec<Cmd> = self
            .views
            .iter()
            .filter_map(|view| match view.phase {
                Phase::Running => Some(Cmd::Stop(view.task)),
                Phase::AtForm => Some(Cmd::Discard(view.task)),
                Phase::Idle => None,
            })
            .collect();
        commands.push(Cmd::Quit);
        commands
    }

    /// The owner left the sign-in: the task is told, and the form that waited for it goes.
    fn cancel_sign_in(&mut self) -> Vec<Cmd> {
        self.close_dialog(TaskId::SyncSpotify);
        vec![Cmd::Send(TaskId::SyncSpotify, Command::Cancel)]
    }
}

/// What the error popup says for a Spotify 429 (F4): the wait from `Retry-After`, or that it is
/// not known.
fn rate_limit(wait: Option<Duration>) -> ErrorInfo {
    let advice = match wait {
        Some(wait) => format!(
            "Spotify asks you to wait {}. Then run Sync Spotify again; what is finished stays.",
            duration_text(wait)
        ),
        None => "Spotify did not say how long to wait. Try again later; what is finished stays."
            .to_owned(),
    };
    ErrorInfo {
        message: "Spotify is limiting requests, so the sync stopped.".to_owned(),
        advice,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crossterm::event::KeyEventState;
    use ratatui::layout::Size;
    use sdl_core::{Device, Event};

    use super::*;
    use crate::app::Settings;

    pub(super) fn clock() -> Clock {
        Clock {
            instant: Instant::now(),
            epoch: 0,
        }
    }

    pub(super) fn app() -> App {
        App::new(Settings::default(), true, Size::new(100, 30), clock())
    }

    pub(super) fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    pub(super) fn task(app: &mut App, task: TaskId, kind: EventKind) -> Vec<Cmd> {
        app.update(Msg::Task(Event { task, kind }), clock())
    }

    /// A function that runs, with no dialog.
    pub(super) fn running(app: &mut App, id: TaskId) {
        if let Some(view) = app.view_mut(id) {
            view.begin();
        }
        let _ = task(app, id, EventKind::Started);
    }

    #[test]
    fn q_with_nothing_running_quits_at_once() {
        let mut app = app();
        let commands = app.update(key(KeyCode::Char('q')), clock());
        assert_eq!(commands, vec![Cmd::Quit]);
        assert!(app.quit);
    }

    #[test]
    fn q_while_a_function_runs_asks_first_and_yes_stops_it_and_quits() {
        let mut app = app();
        running(&mut app, TaskId::Convert);
        assert_eq!(app.update(key(KeyCode::Char('q')), clock()), Vec::new());
        assert!(matches!(app.top(), Some(Popup::Confirm(Confirm::Quit))));
        assert!(!app.quit);
        // Asking again does not stack a second question.
        let _ = app.update(key(KeyCode::Char('q')), clock());
        assert_eq!(app.popups.len(), 1);
        let commands = app.update(key(KeyCode::Char('y')), clock());
        assert_eq!(commands, vec![Cmd::Stop(TaskId::Convert), Cmd::Quit]);
        assert!(app.quit);
    }

    #[test]
    fn no_to_quitting_keeps_going() {
        let mut app = app();
        running(&mut app, TaskId::Convert);
        let _ = app.update(key(KeyCode::Char('q')), clock());
        assert_eq!(app.update(key(KeyCode::Char('n')), clock()), Vec::new());
        assert!(app.popups.is_empty() && !app.quit);
    }

    #[test]
    fn quitting_discards_a_function_still_at_its_form() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Down), clock());
        let _ = app.update(key(KeyCode::Enter), clock());
        let _ = app.update(key(KeyCode::Char('c')), clock());
        // A dialog takes `c`; Ctrl-C is the way out.
        let control_c = Msg::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        let commands = app.update(control_c, clock());
        assert_eq!(
            commands,
            vec![Cmd::Discard(TaskId::DownloadLink), Cmd::Quit]
        );
    }

    #[test]
    fn enter_opens_the_chosen_functions_dialog_and_starts_its_task() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Down), clock());
        assert_eq!(app.selected, TaskId::DownloadLink);
        let commands = app.update(key(KeyCode::Enter), clock());
        assert_eq!(commands, vec![Cmd::Start(TaskId::DownloadLink)]);
        assert!(matches!(app.top(), Some(Popup::Dialog(_))));
        assert_eq!(app.popups.len(), 1);
    }

    #[test]
    fn s_asks_to_stop_only_a_running_function() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Char('s')), clock());
        assert!(app.popups.is_empty());
        running(&mut app, TaskId::SyncSpotify);
        let _ = app.update(key(KeyCode::Char('s')), clock());
        assert!(matches!(
            app.top(),
            Some(Popup::Confirm(Confirm::Stop(TaskId::SyncSpotify)))
        ));
        assert_eq!(
            app.update(key(KeyCode::Char('y')), clock()),
            vec![Cmd::Stop(TaskId::SyncSpotify)]
        );
        assert!(app.popups.is_empty());
    }

    #[test]
    fn a_and_question_mark_open_their_popups_and_esc_closes_them() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Char('a')), clock());
        assert!(matches!(
            app.top(),
            Some(Popup::Attention {
                task: TaskId::SyncSpotify,
                ..
            })
        ));
        let _ = app.update(key(KeyCode::Esc), clock());
        let _ = app.update(key(KeyCode::Char('?')), clock());
        assert!(matches!(app.top(), Some(Popup::Help { .. })));
        let _ = app.update(key(KeyCode::Esc), clock());
        assert!(app.popups.is_empty());
    }

    #[test]
    fn an_error_closes_the_dialog_and_says_what_to_do() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Enter), clock());
        let error = EventKind::Error(ErrorInfo {
            message: "It broke".into(),
            advice: "Try again".into(),
        });
        let _ = task(&mut app, TaskId::SyncSpotify, error);
        assert!(matches!(app.top(), Some(Popup::Error(_))));
        assert_eq!(app.popups.len(), 1);
    }

    fn advice(app: &App) -> Result<String, &'static str> {
        match app.top() {
            Some(Popup::Error(info)) => Ok(info.advice.clone()),
            _ => Err("no error popup"),
        }
    }

    #[test]
    fn a_rate_limit_says_the_wait_or_that_it_is_unknown() -> Result<(), &'static str> {
        let mut app = app();
        running(&mut app, TaskId::SyncSpotify);
        let wait = Some(Duration::from_mins(5));
        let _ = task(
            &mut app,
            TaskId::SyncSpotify,
            EventKind::RateLimited { wait },
        );
        assert!(advice(&app)?.contains("about 5 min"));
        let _ = app.update(key(KeyCode::Esc), clock());
        running(&mut app, TaskId::SyncSpotify);
        let none = EventKind::RateLimited { wait: None };
        let _ = task(&mut app, TaskId::SyncSpotify, none);
        assert!(advice(&app)?.contains("did not say"));
        Ok(())
    }

    #[test]
    fn the_browser_question_sends_go_or_cancels_and_the_waiting_popup_follows() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Enter), clock());
        let sign_in = |state| EventKind::SignIn(state);
        let _ = task(
            &mut app,
            TaskId::SyncSpotify,
            sign_in(SignInState::NeedsBrowser),
        );
        assert!(matches!(app.top(), Some(Popup::Confirm(Confirm::Browser))));
        let go = app.update(key(KeyCode::Enter), clock());
        assert_eq!(go, vec![Cmd::Send(TaskId::SyncSpotify, Command::Go)]);
        let _ = task(
            &mut app,
            TaskId::SyncSpotify,
            sign_in(SignInState::WaitingForBrowser),
        );
        assert!(matches!(app.top(), Some(Popup::Waiting(Waiting::Browser))));
        let cancel = app.update(key(KeyCode::Esc), clock());
        assert_eq!(
            cancel,
            vec![Cmd::Send(TaskId::SyncSpotify, Command::Cancel)]
        );
        // The form that waited for the sign-in is gone with it.
        assert!(app.popups.is_empty());
    }

    #[test]
    fn declining_the_browser_cancels_the_task_and_closes_the_form() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Enter), clock());
        let _ = task(
            &mut app,
            TaskId::SyncSpotify,
            EventKind::SignIn(SignInState::NeedsBrowser),
        );
        let cancel = app.update(key(KeyCode::Char('n')), clock());
        assert_eq!(
            cancel,
            vec![Cmd::Send(TaskId::SyncSpotify, Command::Cancel)]
        );
        assert!(app.popups.is_empty());
    }

    #[test]
    fn a_keychain_problem_is_shown_once_and_does_not_end_the_task() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Enter), clock());
        let _ = task(
            &mut app,
            TaskId::SyncSpotify,
            EventKind::SignIn(SignInState::KeychainProblem(
                "The Keychain is locked".into(),
            )),
        );
        assert!(matches!(app.top(), Some(Popup::Error(_))));
        assert_eq!(app.popups.len(), 2);
        assert!(
            app.view(TaskId::SyncSpotify)
                .is_some_and(FunctionView::is_open)
        );
    }

    #[test]
    fn a_finished_device_sync_asks_to_eject_and_yes_waits_for_the_result() {
        let mut app = app();
        let device = Device {
            name: "SNOWSKY".into(),
            mount_point: "/Volumes/SNOWSKY".into(),
            volume_uuid: Some("AAAA".into()),
            free_bytes: 0,
            disk_image: false,
        };
        let _ = task(
            &mut app,
            TaskId::SyncDevice,
            EventKind::Devices(vec![device]),
        );
        app.settings.state.last_device = Some("AAAA".into());
        running(&mut app, TaskId::SyncDevice);
        let _ = task(&mut app, TaskId::SyncDevice, EventKind::Finished);
        assert_eq!(
            app.top().map(|popup| matches!(popup, Popup::Confirm(Confirm::Eject { device }) if device == "SNOWSKY")),
            Some(true)
        );
        assert_eq!(
            app.update(key(KeyCode::Char('y')), clock()),
            vec![Cmd::Eject]
        );
        assert!(matches!(app.top(), Some(Popup::Waiting(Waiting::Eject(_)))));
        // Other keys do nothing; Esc hides the popup and the eject goes on.
        assert_eq!(app.update(key(KeyCode::Char('q')), clock()), Vec::new());
        assert!(matches!(app.top(), Some(Popup::Waiting(Waiting::Eject(_)))));
        let _ = app.update(Msg::Ejected(Ok(())), clock());
        assert!(app.popups.is_empty());
        app.popups.push(Popup::Waiting(Waiting::Eject("X".into())));
        assert_eq!(app.update(key(KeyCode::Esc), clock()), Vec::new());
        assert!(app.popups.is_empty(), "Esc hides it");
        let failed = ErrorInfo {
            message: "m".into(),
            advice: "a".into(),
        };
        let _ = app.update(Msg::Ejected(Err(failed)), clock());
        assert!(
            matches!(app.top(), Some(Popup::Error(_))),
            "a hidden eject still says it failed"
        );
    }

    #[test]
    fn a_failed_eject_says_what_to_do() {
        let mut app = app();
        app.popups.push(Popup::Waiting(Waiting::Eject("X".into())));
        let info = ErrorInfo {
            message: "m".into(),
            advice: "a".into(),
        };
        let _ = app.update(Msg::Ejected(Err(info.clone())), clock());
        assert_eq!(app.popups.len(), 1);
        assert!(matches!(app.top(), Some(Popup::Error(shown)) if *shown == info));
    }

    #[test]
    fn a_run_that_ends_flashes_its_row_once() {
        let mut app = app();
        running(&mut app, TaskId::Convert);
        let _ = task(&mut app, TaskId::Convert, EventKind::Finished);
        assert_eq!(app.take_ended(), vec![TaskId::Convert]);
        assert_eq!(app.take_ended(), Vec::<TaskId>::new());
    }
}
