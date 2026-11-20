//! The task side of the one step form. Every function is driven the same way: the UI sends
//! [`Command::Answer`] with the form's answers (as often as the owner changes them), then
//! [`Command::Go`], or [`Command::Cancel`]. [`drive`] owns that conversation, so each function
//! only says what its answers mean ([`Step`]).

use std::collections::VecDeque;

use sdl_core::{ConvertChoice, Device, DeviceChoice, EventKind, EventSender, ListChoice};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::runner::Flow;
use crate::{Command, TaskError};

/// What the owner decided for one held song in Review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Use this candidate (its video ID, from the held song's candidates).
    Candidate(String),
    /// Use the link's own audio. Only for a song that came from a link.
    OwnAudio,
    /// Use the audio of this pasted link, with the wanted song's tags.
    PastedLink(String),
    /// Leave the song in the list for later; nothing changes.
    Later,
    /// Never download this song: it leaves the list and is remembered in the "never" set.
    Never,
}

/// One held song and what was chosen for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewChoice {
    /// The held song's key (`WantedSong::key`).
    pub key: String,
    /// The choice.
    pub resolution: Resolution,
}

/// The form's answers, one shape per function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answers {
    /// Sync Spotify: the ticked lists (remembered). Answered with `NewOfTotal`.
    SyncSpotify {
        /// The ticked lists.
        lists: Vec<ListChoice>,
    },
    /// Download link: the link. The run starts at once; no `Go` follows.
    DownloadLink {
        /// The pasted link.
        url: String,
    },
    /// Review: a choice per held song the owner decided on ("Download N chosen"). The run starts
    /// at once; no `Go` follows.
    Review {
        /// The decided songs.
        choices: Vec<ReviewChoice>,
    },
    /// Sync device: the device and what was chosen for it (remembered per device, and as the last
    /// device). Answered with `Plan`.
    SyncDevice {
        /// The chosen device.
        device: Device,
        /// Direction, selection, folders and format.
        choice: DeviceChoice,
    },
    /// Convert: what, format, quality, cover and destination (remembered). Answered with `Plan`.
    Convert(ConvertChoice),
}

/// A request for something the form offers before it can be answered (Sync device only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Look for devices again (the `r` key). Answered with `Devices`.
    Devices,
    /// The top-level folders of `device` (folders directly in its root), for From device.
    /// Answered with `Folders`.
    Folders(Device),
}

/// How a function's first step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opened {
    /// The first events are sent; the form can be answered.
    Ready,
    /// The function needs the owner's consent before it goes on (Sync Spotify: the browser sign-in,
    /// announced with `SignIn(NeedsBrowser)`). `Go` continues ([`Step::after_consent`]); `Cancel`
    /// ends the task.
    WaitsForConsent,
}

/// What a function did with an answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Answered {
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

/// One function's side of the step form. The futures are `Send` by contract, so each function's
/// body is checked for it where it is written and the runner can spawn any of them.
pub(crate) trait Step: Send {
    /// Send the first events (a headline at once, then what the form first shows).
    fn open(&mut self) -> impl Future<Output = Result<Opened, TaskError>> + Send;

    /// After the owner's consent ([`Opened::WaitsForConsent`]): carry on opening.
    fn after_consent(&mut self) -> impl Future<Output = Result<(), TaskError>> + Send {
        std::future::ready(Ok(()))
    }

    /// Answer an [`Offer`]. `true` when the plan made before is now out of date.
    fn offer(&mut self, _offer: Offer) -> impl Future<Output = Result<bool, TaskError>> + Send {
        std::future::ready(Ok(false))
    }

    /// Something that happens by itself while the form is open (a volume appears or goes). `true`
    /// when the plan made before is now out of date. Waits for ever unless a function has such a
    /// thing; it is dropped whenever a command arrives, so it must be safe to start again.
    fn background(&mut self) -> impl Future<Output = Result<bool, TaskError>> + Send {
        std::future::pending()
    }

    /// Take the form's answers and send the plan.
    fn answer(
        &mut self,
        answers: Answers,
    ) -> impl Future<Output = Result<Answered, TaskError>> + Send;

    /// Carry out the plan.
    fn go(&mut self) -> impl Future<Output = Result<Flow, TaskError>> + Send;
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
/// ends it with [`Flow::Cancelled`] and drops whatever that step was doing. When the run starts,
/// `events` sends [`EventKind::Started`] before [`Step::go`] is called, so the UI leaves the form
/// at once, whatever the run does first.
pub(crate) async fn drive(
    step: &mut impl Step,
    events: &EventSender,
    commands: UnboundedReceiver<Command>,
) -> Result<Flow, TaskError> {
    let mut inbox = Inbox {
        commands,
        waiting: VecDeque::new(),
    };
    let Some(opened) = inbox.unless_cancelled(step.open()).await else {
        return Ok(Flow::Cancelled);
    };
    if opened? == Opened::WaitsForConsent {
        loop {
            match inbox.next().await {
                None | Some(Command::Cancel) => return Ok(Flow::Cancelled),
                Some(Command::Go) => break,
                Some(_) => {}
            }
        }
        let Some(consented) = inbox.unless_cancelled(step.after_consent()).await else {
            return Ok(Flow::Cancelled);
        };
        consented?;
    }
    let mut ready = false;
    loop {
        let command = tokio::select! {
            command = inbox.next() => command,
            stale = step.background() => {
                if stale? {
                    ready = false;
                }
                continue;
            }
        };
        match command {
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
                    Answered::Run => return run(step, events).await,
                }
            }
            Some(Command::Go) => {
                if ready {
                    return run(step, events).await;
                }
            }
        }
    }
}

/// Announce the run, then carry it out.
async fn run(step: &mut impl Step, events: &EventSender) -> Result<Flow, TaskError> {
    events.send(EventKind::Started);
    step.go().await
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use sdl_core::{RunOutcome, TaskId};
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A step that records what it was asked.
    #[derive(Default)]
    struct Fake {
        log: Vec<&'static str>,
        can_start: bool,
        run_at_once: bool,
        quirk: Quirk,
    }

    #[derive(Default, PartialEq, Eq)]
    enum Quirk {
        #[default]
        None,
        Hang,
        Consent,
    }

    impl Step for Fake {
        fn open(&mut self) -> impl Future<Output = Result<Opened, TaskError>> {
            self.log.push("open");
            std::future::ready(Ok(if self.quirk == Quirk::Consent {
                Opened::WaitsForConsent
            } else {
                Opened::Ready
            }))
        }

        fn after_consent(&mut self) -> impl Future<Output = Result<(), TaskError>> {
            self.log.push("consent");
            std::future::ready(Ok(()))
        }

        fn offer(&mut self, _offer: Offer) -> impl Future<Output = Result<bool, TaskError>> {
            self.log.push("offer");
            std::future::ready(Ok(true))
        }

        async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
            if self.quirk == Quirk::Hang {
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

    /// Send `commands`, drive `step` to the end, and say whether it was cancelled and whether the
    /// run was announced.
    async fn drive_with(
        step: &mut Fake,
        commands: Vec<Command>,
    ) -> Result<(bool, bool), TaskError> {
        let (tx, rx) = unbounded_channel();
        for command in commands {
            assert!(tx.send(command).is_ok());
        }
        drop(tx);
        let (events_tx, _events_rx) = sdl_core::channel();
        let events = EventSender::new(TaskId::Convert, events_tx);
        let cancelled = matches!(drive(step, &events, rx).await?, Flow::Cancelled);
        Ok((cancelled, events.snapshot().started))
    }

    /// As [`drive_with`], saying only whether it was cancelled.
    async fn run(step: &mut Fake, commands: Vec<Command>) -> Result<bool, TaskError> {
        Ok(drive_with(step, commands).await?.0)
    }

    #[tokio::test]
    async fn go_waits_for_an_answer_that_can_start() -> TestResult {
        let mut step = Fake {
            can_start: true,
            ..Fake::default()
        };
        let (cancelled, started) =
            drive_with(&mut step, vec![Command::Go, convert(), Command::Go]).await?;
        assert!(!cancelled && started, "the run is announced");
        assert_eq!(step.log, ["open", "answer", "go"]);
        Ok(())
    }

    #[tokio::test]
    async fn go_is_ignored_when_the_plan_cannot_start() -> TestResult {
        let mut step = Fake::default();
        let (cancelled, started) = drive_with(&mut step, vec![convert(), Command::Go]).await?;
        assert!(
            cancelled && !started,
            "the closed channel ends it, nothing ran"
        );
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
        let folders = Command::Offer(Offer::Folders(device()));
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
        let other = Command::Answer(Answers::DownloadLink { url: "u".into() });
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
    async fn consent_waits_for_go_and_ignores_answers_until_then() -> TestResult {
        let mut step = Fake {
            quirk: Quirk::Consent,
            can_start: true,
            ..Fake::default()
        };
        let cancelled = run(
            &mut step,
            vec![convert(), Command::Go, convert(), Command::Go],
        )
        .await?;
        assert!(!cancelled);
        assert_eq!(step.log, ["open", "consent", "answer", "go"]);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_at_the_consent_step_ends_the_task() -> TestResult {
        let mut step = Fake {
            quirk: Quirk::Consent,
            ..Fake::default()
        };
        assert!(run(&mut step, vec![Command::Cancel]).await?);
        assert_eq!(step.log, ["open"]);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_ends_the_task_even_while_a_step_is_working() -> TestResult {
        let mut step = Fake {
            quirk: Quirk::Hang,
            ..Fake::default()
        };
        let cancelled = run(&mut step, vec![convert(), Command::Cancel]).await?;
        assert!(cancelled);
        assert_eq!(step.log, ["open"]);
        Ok(())
    }
}
