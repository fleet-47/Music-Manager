//! `sdl` entry point: load the config, run the TUI, restore the terminal on every exit path.

use color_eyre::eyre::Result;
use crossterm::event::{Event as TermEvent, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use sdl::app::{Action, App};
use sdl::{pipeline, screens};
use sdl_core::{Config, Event};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

fn main() -> Result<()> {
    // Errors are shown as the message and its causes only: no source location or backtrace
    // instructions, which mean nothing to the owner.
    color_eyre::config::HookBuilder::default()
        .display_location_section(false)
        .display_env_section(false)
        .install()?;
    // A missing or malformed config is reported before the terminal changes mode.
    let config = Config::load()?;
    let runtime = Runtime::new()?;
    let mut terminal = ratatui::init();
    let result = runtime.block_on(run(&mut terminal, config));
    ratatui::restore();
    // Blocking work cannot be cancelled (the sign-in listener waits for a browser redirect with no
    // timeout), so quitting must not wait for it.
    runtime.shutdown_background();
    result
}

async fn run(terminal: &mut DefaultTerminal, config: Config) -> Result<()> {
    let mut keys = EventStream::new();
    let mut app = App::new(config);
    let mut worker: Option<JoinHandle<()>> = None;
    // Each run gets a fresh channel, so events from a stopped worker never reach the next run.
    let (_, mut events_rx) = mpsc::unbounded_channel::<Event>();

    let result = loop {
        if app.quit {
            break Ok(());
        }
        if let Err(error) = terminal.draw(|frame| screens::render(&mut app, frame)) {
            break Err(error.into());
        }
        let action = tokio::select! {
            key = keys.next() => match key {
                Some(Ok(TermEvent::Key(key))) => app.on_key(key),
                Some(Ok(_)) => Action::None,
                Some(Err(error)) => break Err(error.into()),
                None => break Ok(()),
            },
            Some(event) = events_rx.recv() => {
                app.on_worker_event(event);
                while let Ok(event) = events_rx.try_recv() {
                    app.on_worker_event(event);
                }
                Action::None
            }
        };
        if action == Action::None {
            continue;
        }
        if let Some(handle) = worker.take() {
            handle.abort();
        }
        let (events_tx, fresh_rx) = mpsc::unbounded_channel();
        events_rx = fresh_rx;
        worker = pipeline::spawn(action, app.config.clone(), events_tx);
    };
    if let Some(handle) = worker {
        handle.abort();
    }
    result
}
