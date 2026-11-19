//! The one boundary between the screen and the task runner: the screen emits [`Command`]s, and
//! [`Control::run`] turns each into a call on [`crate::tasks::Runner`] (start, message, stop).

use sdl_core::state::StateError;
use sdl_core::{Config, ErrorInfo, Event, EventKind, EventSender, ListChoice, State, TaskId};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::FUNCTIONS;
use crate::tasks::{Command as TaskCommand, Runner, StartError};

/// What the screen asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start Sync Spotify: sign in, then the lists.
    StartSpotify,
    /// Start Sync card: mount, then the plan.
    StartCard,
    /// Start Download link with this link.
    StartLink(String),
    /// Spotify: the ticked lists.
    Choose(Vec<ListChoice>),
    /// Spotify or card: the owner pressed Enter on the last step.
    Go(TaskId),
    /// Leave a waiting step; the task reports `Stopped`.
    Cancel(TaskId),
    /// Stop a running task; it reports `Stopped`.
    Stop(TaskId),
    /// Card: clean up and unmount. Reported as `Finished` or `Error` for Sync card.
    FinishCard,
    /// Stop everything; the app is closing.
    Quit,
}

/// Starts, messages and stops tasks.
pub struct Control {
    runner: Runner,
    tx: UnboundedSender<Event>,
}

impl Control {
    /// A control whose tasks send their events on `tx`. The runner loads the state file.
    ///
    /// # Errors
    /// The state file is unreadable or malformed.
    pub fn new(config: Config, tx: UnboundedSender<Event>) -> Result<Self, StateError> {
        Ok(Self {
            runner: Runner::new(config, tx.clone())?,
            tx,
        })
    }

    /// The state the runner keeps: ticked lists and each function's last-run line.
    #[must_use]
    pub fn state(&self) -> State {
        self.runner.state()
    }

    /// Carry out one command. Failures come back as events for the screen.
    pub async fn run(&mut self, command: Command) {
        match command {
            Command::StartSpotify => {
                let started = self.runner.start_spotify();
                self.report(TaskId::SyncSpotify, started);
            }
            Command::StartCard => {
                let started = self.runner.start_card_sync();
                self.report(TaskId::SyncCard, started);
            }
            Command::StartLink(url) => {
                let started = self.runner.start_download_link(url);
                self.report(TaskId::DownloadLink, started);
            }
            Command::Choose(chosen) => self.send(TaskId::SyncSpotify, TaskCommand::Choices(chosen)),
            Command::Go(task) => self.send(task, TaskCommand::Go),
            Command::Cancel(task) => self.send(task, TaskCommand::Cancel),
            Command::Stop(task) => {
                self.runner.stop(task).await;
            }
            Command::FinishCard => {
                // Runs on its own so the screen keeps drawing; the result arrives as an event.
                let unmount = self.runner.unmount_card();
                let events = EventSender::new(TaskId::SyncCard, self.tx.clone());
                tokio::spawn(async move {
                    events.send(match unmount.await {
                        Ok(()) => EventKind::Finished,
                        Err(info) => EventKind::Error(info),
                    });
                });
            }
            Command::Quit => {
                for task in FUNCTIONS {
                    self.runner.stop(task).await;
                }
            }
        }
    }

    fn report(&self, task: TaskId, started: Result<(), StartError>) {
        if let Err(error) = started {
            EventSender::new(task, self.tx.clone()).send(EventKind::Error(ErrorInfo {
                message: error.to_string(),
                advice: "Wait for it to finish, or stop it first.".to_owned(),
            }));
        }
    }

    fn send(&self, task: TaskId, command: TaskCommand) {
        // `false` means the task already ended; its terminal event is on its way.
        let _ = self.runner.send(task, command);
    }
}
