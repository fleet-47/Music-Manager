//! The screen: the event loop, drawing, theme, effects and mouse routing (F1, F2). It owns the
//! terminal and the [`sdl_tasks::Runner`]; the app state it drives is [`crate::app::App`].
//!
//! The loop draws only after something changed: a key, mouse or paste event, a task event, a
//! resize, or a timer the app asked for ([`crate::app::wake::next_wake`]). All queued task events
//! are applied before one draw. With nothing running and no effect playing, the loop waits on
//! input alone and no timer exists.

pub mod effects;
pub mod keys;
pub mod panels;
pub mod popups;
pub mod theme;

use std::io::stdout;
use std::time::Instant;

use color_eyre::eyre::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as TermEvent, EventStream, KeyEventKind,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::layout::Size;
use sdl_core::{Config, ErrorInfo, EventReceiver, channel};
use sdl_tasks::Runner;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::app::wake::next_wake;
use crate::app::{App, Clock, Cmd, Hits, Msg, Settings};
use effects::Effects;
use theme::Theme;

/// Run the screen until the owner quits.
///
/// # Errors
/// The settings files could not be read, or the terminal could not be set up or drawn to.
pub async fn run(config: Config) -> Result<()> {
    let animations = config.animations;
    let music_root = config.music_root.clone();
    let (tx, events) = channel();
    // The state and review files are read here, so a malformed one is reported before the screen
    // starts.
    let runner = Runner::new(config, tx)?;
    // `try_init` can fail after raw mode is on, so the terminal is restored on every path.
    let result = match ratatui::try_init() {
        Ok(mut terminal) => match execute!(stdout(), EnableBracketedPaste, EnableMouseCapture) {
            Ok(()) => event_loop(&mut terminal, runner, events, animations, music_root).await,
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    };
    // Best effort: the terminal is being given back, and there is no better place to report this.
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn now() -> Clock {
    Clock {
        instant: Instant::now(),
        epoch: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    }
}

fn settings(runner: &Runner, music_root: &std::path::Path) -> Settings {
    Settings {
        state: runner.state(),
        review: runner.review(),
        music_root: music_root.to_path_buf(),
    }
}

/// Wait for `wake`, or forever when no timer is wanted.
async fn timer(wake: Option<std::time::Duration>) {
    match wake {
        Some(wake) => tokio::time::sleep(wake).await,
        None => std::future::pending().await,
    }
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    mut runner: Runner,
    mut events: EventReceiver,
    animations: bool,
    music_root: std::path::PathBuf,
) -> Result<()> {
    let theme = Theme::from_env();
    let effects_on = animations && theme.base().is_some();
    let size = terminal.size()?;
    let mut app = App::new(
        settings(&runner, &music_root),
        effects_on,
        Size::new(size.width, size.height),
        now(),
    );
    let mut effects = Effects::default();
    let mut input = EventStream::new();
    let (eject_tx, mut ejected): (_, UnboundedReceiver<Result<(), ErrorInfo>>) =
        unbounded_channel();
    let mut last_frame = Instant::now();
    let mut dirty = true;

    loop {
        if dirty {
            let frame_start = Instant::now();
            draw(
                terminal,
                &mut app,
                &mut effects,
                theme,
                frame_start.duration_since(last_frame),
            )?;
            last_frame = frame_start;
            dirty = false;
        }
        let msgs = tokio::select! {
            Some(Ok(event)) = input.next() => from_terminal(event).into_iter().collect(),
            Some(first) = events.recv() => {
                // Every queued task event is applied before the one draw. The settings are read
                // after the queue is drained, so they are at least as new as every event in the
                // batch, and go first: each event is then handled against current settings.
                let mut tasks = vec![Msg::Task(first)];
                while let Ok(more) = events.try_recv() {
                    tasks.push(Msg::Task(more));
                }
                let mut batch = vec![Msg::Settings(Box::new(settings(&runner, &music_root)))];
                batch.extend(tasks);
                batch
            }
            Some(result) = ejected.recv() => vec![Msg::Ejected(result)],
            () = timer(next_wake(&app)) => vec![Msg::Wake],
        };
        for msg in msgs {
            dirty = true;
            for cmd in app.update(msg, now()) {
                if matches!(cmd, Cmd::Quit) {
                    return Ok(());
                }
                run_cmd(&mut runner, cmd, &eject_tx).await;
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}

fn from_terminal(event: TermEvent) -> Option<Msg> {
    match event {
        TermEvent::Key(key) if key.kind != KeyEventKind::Release => Some(Msg::Key(key)),
        TermEvent::Mouse(mouse) => Some(Msg::Mouse(mouse)),
        TermEvent::Paste(text) => Some(Msg::Paste(text)),
        TermEvent::Resize(width, height) => Some(Msg::Resize(Size::new(width, height))),
        _ => None,
    }
}

/// Draw one frame: the main screen, the popups, then the effects over them; record what was
/// drawn for the mouse and whether an effect still plays.
fn draw(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    effects: &mut Effects,
    theme: Theme,
    elapsed: std::time::Duration,
) -> Result<()> {
    let mut hits = Hits::default();
    terminal.draw(|frame| {
        panels::draw(frame, app, theme, &mut hits);
        popups::draw(frame, app, theme, &mut hits);
        let area = frame.area();
        effects.process(elapsed, frame.buffer_mut(), area);
    })?;
    app.hits = hits;
    app.animating = app.animations && effects.is_running();
    Ok(())
}

/// Run one command against the runner.
async fn run_cmd(
    runner: &mut Runner,
    cmd: Cmd,
    eject: &tokio::sync::mpsc::UnboundedSender<Result<(), ErrorInfo>>,
) {
    match cmd {
        Cmd::Start(task) => {
            // Starting a function that is already running changes nothing; the app does not ask.
            let _ = runner.start(task);
        }
        Cmd::Send(task, command) => {
            let _ = runner.send(task, command);
        }
        Cmd::Stop(task) => {
            let _ = runner.stop(task).await;
        }
        Cmd::Discard(task) => runner.discard(task).await,
        Cmd::Eject => {
            let future = runner.eject_device();
            let eject = eject.clone();
            tokio::spawn(async move {
                // The receiver is gone only when the app is closing.
                let _ = eject.send(future.await);
            });
        }
        Cmd::Open(url) => {
            // A browser that cannot be opened is not worth an error popup here.
            let _ = open::that_detached(url);
        }
        Cmd::Quit => {}
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use sdl_core::TaskId;

    use super::*;
    use crate::app::{Popup, Settings};
    use crate::dialogs::Dialog;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    fn app() -> App {
        App::new(Settings::default(), false, Size::new(100, 30), now())
    }

    #[test]
    fn the_main_panel_shows_every_function_and_the_footer() -> TestResult {
        let app = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        let mut hits = Hits::default();
        terminal.draw(|frame| panels::draw(frame, &app, Theme::new(false), &mut hits))?;
        let text = screen_text(&terminal);
        for task in TaskId::ALL {
            assert!(text.contains(task.name()), "{}", task.name());
        }
        assert!(text.contains("q quit"));
        assert_eq!(hits.main.len(), 5);
        Ok(())
    }

    #[test]
    fn a_dialog_is_drawn_in_a_popup_and_its_buttons_can_be_clicked() -> TestResult {
        let mut app = app();
        let (dialog, _commands) = Dialog::open(TaskId::DownloadLink, &app.settings);
        app.popups.push(Popup::Dialog(Box::new(dialog)));
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        let mut hits = Hits::default();
        terminal.draw(|frame| {
            panels::draw(frame, &app, Theme::new(false), &mut hits);
            popups::draw(frame, &app, Theme::new(false), &mut hits);
        })?;
        let text = screen_text(&terminal);
        assert!(text.contains("Paste a link"));
        assert!(text.contains("Start"));
        assert!(hits.form.area_of(&sdl_form::Target::Primary).is_some());
        Ok(())
    }
}
