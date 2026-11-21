//! What goes into `App::update` and what comes out of it. Nothing here touches the terminal or
//! the runner, so the whole app can be driven in a test.

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Size;
use sdl_core::{ErrorInfo, Event, TaskId};
use sdl_tasks::Command;

use super::Settings;

/// An input to the app.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A key press (the loop drops releases).
    Key(KeyEvent),
    /// A mouse event; tested against the rectangles the last frame drew (`App::hits`).
    Mouse(MouseEvent),
    /// Pasted text (bracketed paste).
    Paste(String),
    /// The terminal changed size.
    Resize(Size),
    /// A task event. The loop applies every queued one before it draws.
    Task(Event),
    /// The runner's state and review list, read after the queued task events were taken and sent
    /// before them (a task saves its choices and its last-run line before it reports), so every
    /// event of the batch is handled against settings at least as new as itself.
    Settings(Box<Settings>),
    /// The eject the app asked for ([`Cmd::Eject`]) ended.
    Ejected(Result<(), ErrorInfo>),
    /// The install the app asked for ([`Cmd::InstallTools`]) ended: the tools still missing, and
    /// what went wrong when it failed.
    Tools {
        /// Tools still not on `PATH`.
        missing: Vec<String>,
        /// Why the install failed, in plain words.
        error: Option<String>,
    },
    /// A timer the app asked for ([`crate::app::wake::next_wake`]) fired: spinners advance and the
    /// elapsed time is read again. Carries no state of its own.
    Wake,
}

/// Something the app asks the event loop to do. The loop runs them in order, against the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// `Runner::start` for the function.
    Start(TaskId),
    /// `Runner::send`. Made with [`Cmd::send`]: a command carries a whole form's answers.
    Send(TaskId, Box<Command>),
    /// `Runner::stop` (after the owner confirmed).
    Stop(TaskId),
    /// `Runner::discard`: end a function that never started its run, as the app closes.
    Discard(TaskId),
    /// `Runner::eject_device`; its result comes back as [`Msg::Ejected`].
    Eject,
    /// Install these tools with mise; the result comes back as [`Msg::Tools`].
    InstallTools(Vec<String>),
    /// Open a link in the browser (`open`): review's `o`.
    Open(String),
    /// Leave the loop. Always the last command.
    Quit,
}

impl Cmd {
    /// `Runner::send` of `command` to the function.
    #[must_use]
    pub fn send(task: TaskId, command: Command) -> Self {
        Self::Send(task, Box::new(command))
    }
}
