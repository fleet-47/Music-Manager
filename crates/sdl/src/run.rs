//! The event loop: terminal events, task events and ticks go into [`App`]; the screen is drawn
//! after each; commands go to [`Control`].

use std::io::stdout;
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, EventStream, KeyEventKind,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use sdl_core::{Config, EventReceiver, channel};

use crate::app::App;
use crate::control::{Command, Control};
use crate::tasks::now_secs;
use crate::theme::Theme;
use crate::ui;

const TICK: Duration = Duration::from_millis(100);

/// Run the screen until the owner quits.
///
/// # Errors
/// The terminal could not be set up or drawn to.
pub async fn run(config: Config) -> Result<()> {
    // The state file is read here, so a malformed one is reported before the screen starts.
    let (tx, events) = channel();
    let mut control = Control::new(config, tx)?;
    let app = App::new(control.state(), now_secs());
    // `try_init` can fail after raw mode is on, so the terminal is restored on every path.
    let result = match ratatui::try_init() {
        Ok(mut terminal) => match execute!(stdout(), EnableBracketedPaste) {
            Ok(()) => event_loop(&mut terminal, app, &mut control, events).await,
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    };
    // Best effort: the terminal is being given back, and there is no better place to report this.
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    mut app: App,
    control: &mut Control,
    mut events: EventReceiver,
) -> Result<()> {
    let theme = Theme::from_env();
    let mut keys = EventStream::new();
    let mut ticks = tokio::time::interval(TICK);
    let mut last_tick = Instant::now();

    while !app.should_quit() {
        let size = terminal.size()?;
        app.set_viewport(size.width, size.height);
        terminal.draw(|frame| ui::render(frame, &app, theme))?;
        let commands = tokio::select! {
            Some(Ok(event)) = keys.next() => on_terminal_event(&mut app, event),
            Some(event) = events.recv() => {
                let commands = app.apply_event(event);
                // A task saves the owner's answers and its last-run line before it reports, so
                // the main panel shows the choices a run uses from its first event on.
                app.set_state(control.state());
                commands
            }
            _ = ticks.tick() => {
                let now = Instant::now();
                app.tick(now.duration_since(last_tick), now_secs());
                last_tick = now;
                Vec::new()
            }
        };
        for command in commands {
            control.run(command).await;
        }
    }
    Ok(())
}

fn on_terminal_event(app: &mut App, event: TermEvent) -> Vec<Command> {
    match event {
        TermEvent::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
        TermEvent::Paste(text) => app.handle_paste(&text),
        _ => Vec::new(),
    }
}
