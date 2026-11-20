//! The task side of the one step form. Every function is driven the same way: the UI sends
//! [`Command::Answer`] with the form's answers (as often as the owner changes them), then
//! [`Command::Go`], or [`Command::Cancel`]. [`drive`] owns that conversation, so each function
//! only says what its answers mean ([`Step`]).

use std::collections::VecDeque;

use sdl_core::{ConvertChoice, Device, DeviceChoice, Direction, FormatChoice, ListChoice};
use tokio::sync::mpsc::UnboundedReceiver;

use super::runner::Flow;
use super::{Command, TaskError};

/// The form's answers, one shape per function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answers {
    /// Sync Spotify: the ticked lists and the format rows. Answered with `NewOfTotal`.
    Spotify {
        /// The ticked lists.
        lists: Vec<ListChoice>,
        /// Format and Remember this format.
        format: FormatChoice,
    },
    /// Download link: the link and the format rows. The run starts at once; no `Go` follows.
    Link {
        /// The pasted link.
        url: String,
        /// Format and Remember this format.
        format: FormatChoice,
    },
    /// Convert: folders, format and destination. Answered with `Plan`.
    Convert(ConvertChoice),
    /// Sync device: the device and its folders, direction and format. Answered with `Plan`.
    Device {
        /// The chosen device.
        device: Device,
        /// Folders, direction and format.
        choice: DeviceChoice,
    },
}

/// A request for something the form offers before it can be answered (Sync device only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Look for devices again (the `r` key). Answered with `Devices`.
    Devices,
    /// The top-level folders on the source side of `device` for `direction`. Answered with
    /// `Folders`.
    Folders {
        /// The chosen device.
        device: Device,
        /// The chosen direction.
        direction: Direction,
    },
}

/// What a function did with an answer.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Answered {
    /// The answer is not this function's; nothing changed.
    Ignored,
    /// The plan is shown. `Go` starts the run when `can_start`.
    Waiting {
        /// Whether the run may start.
        can_start: bool,
    },
    /// Nothing more to ask: start the run now.
    Run,
}

/// One function's side of the step form.
pub(super) trait Step {
    /// Send the first events (a headline at once, then what the form first shows).
    async fn open(&mut self) -> Result<(), TaskError>;

    /// Answer an [`Offer`]. `true` when it applied; any plan made before is then out of date.
    async fn offer(&mut self, _offer: Offer) -> Result<bool, TaskError> {
        Ok(false)
    }

    /// Take the form's answers and send the plan.
    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError>;

    /// Carry out the plan.
    async fn go(&mut self) -> Result<Flow, TaskError>;
}

/// The UI's commands, read one at a time. A command that arrives while a step is working is kept
/// for after it, never lost; only [`Command::Cancel`] (or the UI going) reaches the step at once.
struct Inbox {
    commands: UnboundedReceiver<Command>,
    waiting: VecDeque<Command>,
}

impl Inbox {
    /// The next command; `None` when the UI is gone.
    async fn next(&mut self) -> Option<Command> {
        match self.waiting.pop_front() {
            Some(command) => Some(command),
            None => self.commands.recv().await,
        }
    }

    /// Run `work` unless the owner cancels (or the UI goes) first: then `None`, and `work` is
    /// dropped, which frees what it holds (the sign-in listener's port, for one). A finished
    /// `work` wins over a cancel that arrives with it; the cancel is read next.
    async fn unless_cancelled<T>(&mut self, work: impl Future<Output = T>) -> Option<T> {
        let waiting = &mut self.waiting;
        let commands = &mut self.commands;
        let cancelled = async {
            while let Some(command) = commands.recv().await {
                if command == Command::Cancel {
                    return;
                }
                waiting.push_back(command);
            }
        };
        tokio::select! {
            biased;
            out = work => Some(out),
            () = cancelled => None,
        }
    }
}

/// Run `step` against the UI's `commands`. A command that does not fit is ignored; `Go` is ignored
/// until the last answer left a plan that can start. Cancelling during any step (or the UI going)
/// ends it with [`Flow::Cancelled`] and drops whatever that step was doing.
pub(super) async fn drive(
    step: &mut impl Step,
    commands: UnboundedReceiver<Command>,
) -> Result<Flow, TaskError> {
    let mut inbox = Inbox {
        commands,
        waiting: VecDeque::new(),
    };
    let Some(opened) = inbox.unless_cancelled(step.open()).await else {
        return Ok(Flow::Cancelled);
    };
    opened?;
    let mut ready = false;
    loop {
        match inbox.next().await {
            None | Some(Command::Cancel) => return Ok(Flow::Cancelled),
            Some(Command::Offer(offer)) => {
                let Some(applied) = inbox.unless_cancelled(step.offer(offer)).await else {
                    return Ok(Flow::Cancelled);
                };
                if applied? {
                    ready = false;
                }
            }
            Some(Command::Answer(answers)) => {
                let Some(answered) = inbox.unless_cancelled(step.answer(answers)).await else {
                    return Ok(Flow::Cancelled);
                };
                match answered? {
                    Answered::Ignored => {}
                    Answered::Waiting { can_start } => ready = can_start,
                    Answered::Run => return step.go().await,
                }
            }
            Some(Command::Go) => {
                if ready {
                    return step.go().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use sdl_core::RunOutcome;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A step that records what it was asked.
    #[derive(Default)]
    struct Fake {
        log: Vec<&'static str>,
        can_start: bool,
        run_at_once: bool,
        hang_on_answer: bool,
    }

    impl Step for Fake {
        fn open(&mut self) -> impl Future<Output = Result<(), TaskError>> {
            self.log.push("open");
            std::future::ready(Ok(()))
        }

        fn offer(&mut self, _offer: Offer) -> impl Future<Output = Result<bool, TaskError>> {
            self.log.push("offer");
            std::future::ready(Ok(true))
        }

        async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
            if self.hang_on_answer {
                pending::<()>().await;
            }
            self.log.push("answer");
            Ok(match answers {
                Answers::Convert(_) if self.run_at_once => Answered::Run,
                Answers::Convert(_) => Answered::Waiting {
                    can_start: self.can_start,
                },
                _ => Answered::Ignored,
            })
        }

        fn go(&mut self) -> impl Future<Output = Result<Flow, TaskError>> {
            self.log.push("go");
            std::future::ready(Ok(Flow::Done {
                line: String::new(),
                outcome: RunOutcome::Done,
            }))
        }
    }

    fn convert() -> Command {
        Command::Answer(Answers::Convert(ConvertChoice::default()))
    }

    fn device() -> Device {
        Device {
            name: "D".into(),
            mount_point: "/Volumes/D".into(),
            volume_uuid: None,
            free_bytes: 0,
        }
    }

    /// Send `commands`, drive `step` to the end, and say whether it was cancelled.
    async fn run(step: &mut Fake, commands: Vec<Command>) -> Result<bool, TaskError> {
        let (tx, rx) = unbounded_channel();
        for command in commands {
            assert!(tx.send(command).is_ok());
        }
        drop(tx);
        Ok(matches!(drive(step, rx).await?, Flow::Cancelled))
    }

    #[tokio::test]
    async fn go_waits_for_an_answer_that_can_start() -> TestResult {
        let mut step = Fake {
            can_start: true,
            ..Fake::default()
        };
        let cancelled = run(&mut step, vec![Command::Go, convert(), Command::Go]).await?;
        assert!(!cancelled);
        assert_eq!(step.log, ["open", "answer", "go"]);
        Ok(())
    }

    #[tokio::test]
    async fn go_is_ignored_when_the_plan_cannot_start() -> TestResult {
        let mut step = Fake::default();
        let cancelled = run(&mut step, vec![convert(), Command::Go]).await?;
        assert!(cancelled, "the closed channel ends it, nothing ran");
        assert_eq!(step.log, ["open", "answer"]);
        Ok(())
    }

    #[tokio::test]
    async fn an_offer_makes_the_last_plan_stale() -> TestResult {
        let mut step = Fake {
            can_start: true,
            ..Fake::default()
        };
        let offer = Command::Offer(Offer::Devices);
        let folders = Command::Offer(Offer::Folders {
            device: device(),
            direction: Direction::ToDevice,
        });
        let cancelled = run(
            &mut step,
            vec![convert(), offer, Command::Go, folders, Command::Go],
        )
        .await?;
        assert!(cancelled);
        assert_eq!(step.log, ["open", "answer", "offer", "offer"]);
        Ok(())
    }

    #[tokio::test]
    async fn an_answer_for_another_function_changes_nothing() -> TestResult {
        let mut step = Fake {
            can_start: true,
            ..Fake::default()
        };
        let other = Command::Answer(Answers::Link {
            url: "u".into(),
            format: FormatChoice::default(),
        });
        let cancelled = run(&mut step, vec![convert(), other, Command::Go]).await?;
        assert!(!cancelled);
        assert_eq!(step.log, ["open", "answer", "answer", "go"]);
        Ok(())
    }

    #[tokio::test]
    async fn run_starts_without_go() -> TestResult {
        let mut step = Fake {
            run_at_once: true,
            ..Fake::default()
        };
        assert!(!run(&mut step, vec![convert()]).await?);
        assert_eq!(step.log, ["open", "answer", "go"]);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_ends_the_task_even_while_a_step_is_working() -> TestResult {
        let mut step = Fake {
            hang_on_answer: true,
            ..Fake::default()
        };
        let cancelled = run(&mut step, vec![convert(), Command::Cancel]).await?;
        assert!(cancelled);
        assert_eq!(step.log, ["open"]);
        Ok(())
    }
}
