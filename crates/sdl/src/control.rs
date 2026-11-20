//! The one boundary between the screen and the task runner: the screen emits [`Command`]s, and
//! [`Control::run`] turns each into a call on [`crate::tasks::Runner`] (start, message, stop).

use sdl_core::state::StateError;
use sdl_core::{Config, ErrorInfo, Event, EventKind, EventSender, State, TaskId};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::FUNCTIONS;
use crate::tasks::{Answers, Command as TaskCommand, Offer, Runner, StartError};

/// What the screen asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start a function's task; it opens its step and waits for the form's answers.
    Start(TaskId),
    /// The step form's answers; the task replies with its plan (Download link starts at once).
    Answer(Answers),
    /// Sync device: something the form needs (devices again, a device's folders).
    Offer(Offer),
    /// The owner pressed Enter on the last step.
    Go(TaskId),
    /// Leave a waiting step; the task reports `Stopped`.
    Cancel(TaskId),
    /// Stop a running task; it reports `Stopped`.
    Stop(TaskId),
    /// Device: clean up and unmount. Reported as `Finished` or `Error` for Sync device.
    Eject,
    /// The app is closing: end every task still there (sent after a `Stop` for each running one,
    /// so only tasks waiting at their form are left; their last-run line stays as it was).
    Quit,
}

/// The function a form's answers are for.
fn task_of(answers: &Answers) -> TaskId {
    match answers {
        Answers::Spotify { .. } => TaskId::SyncSpotify,
        Answers::Link { .. } => TaskId::DownloadLink,
        Answers::Convert(_) => TaskId::Convert,
        Answers::Device { .. } => TaskId::SyncDevice,
    }
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

    /// The state the runner keeps: the remembered choices and each function's last-run line.
    #[must_use]
    pub fn state(&self) -> State {
        self.runner.state()
    }

    /// Carry out one command. Failures come back as events for the screen.
    pub async fn run(&mut self, command: Command) {
        match command {
            Command::Start(task) => {
                let started = self.runner.start(task);
                self.report(task, started);
            }
            Command::Answer(answers) => self.send(task_of(&answers), TaskCommand::Answer(answers)),
            Command::Offer(offer) => self.send(TaskId::SyncDevice, TaskCommand::Offer(offer)),
            Command::Go(task) => self.send(task, TaskCommand::Go),
            Command::Cancel(task) => self.send(task, TaskCommand::Cancel),
            Command::Stop(task) => {
                self.runner.stop(task).await;
            }
            Command::Eject => {
                // Runs on its own so the screen keeps drawing; the result arrives as an event.
                let unmount = self.runner.eject_device();
                let events = EventSender::new(TaskId::SyncDevice, self.tx.clone());
                tokio::spawn(async move {
                    events.send(match unmount.await {
                        Ok(()) => EventKind::Finished,
                        Err(info) => EventKind::Error(info),
                    });
                });
            }
            Command::Quit => {
                for task in FUNCTIONS {
                    self.runner.discard(task).await;
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
