//! The task side of the one step form. Every function is driven the same way: the UI sends
//! [`Command::Answer`] with the form's answers (as often as the owner changes them), then
//! [`Command::Go`], or [`Command::Cancel`]. [`drive`] owns that conversation, so each function
//! only says what its answers mean ([`Step`]).

use std::collections::VecDeque;

use sdl_core::{
    Config, ConvertChoice, Device, DeviceChoice, EventKind, EventSender, Library, LibraryId,
    ListChoice,
};
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
    /// The library the song is held for (`HeldSong::library`); with the key it names one entry.
    pub library: LibraryId,
    /// The held song's key (`WantedSong::key`).
    pub key: String,
    /// The choice.
    pub resolution: Resolution,
}

/// Whose library Sync Spotify and Download link add to: the form's **For me** toggle and, when it
/// is off, its **Library** dropdown (F12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipient {
    /// For me: the owner's own library.
    Me,
    /// An existing other library, by name.
    Existing(String),
    /// **New library…**: a library to make, by the name as typed (made FAT-safe when it is made).
    /// The form has already checked it with `Library::check_new_name`.
    New(String),
}

impl Recipient {
    /// The library this recipient names, for planning: nothing is created, so a new library is
    /// only checked ([`Library::check_new_name`]) and named.
    ///
    /// # Errors
    /// A new library whose name is empty or taken, or `music_root` cannot be read.
    pub(crate) fn library(&self, config: &Config) -> Result<Library, TaskError> {
        match self {
            Self::Me => Ok(Library::mine(config)),
            Self::Existing(name) => Ok(Library::resolve(config, &LibraryId::Other(name.clone()))),
            Self::New(typed) => {
                let name = Library::check_new_name(&Library::list(config)?, typed)?;
                Ok(Library::at(&name, &config.music_root.join(&name), false))
            }
        }
    }

    /// The library to add songs to once the run has started: a new library's folder is made now.
    ///
    /// # Errors
    /// A new library that is refused (the name is empty or taken) or whose folder cannot be made.
    pub(crate) fn make(&self, config: &Config) -> Result<Library, TaskError> {
        match self {
            Self::New(typed) => Ok(Library::create(config, typed)?),
            Self::Me | Self::Existing(_) => self.library(config),
        }
    }
}

/// The form's answers, one shape per function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answers {
    /// Sync Spotify: the ticked lists (remembered). Answered with `NewOfTotal`.
    SyncSpotify {
        /// The ticked lists.
        lists: Vec<ListChoice>,
        /// Whose library the songs go into.
        recipient: Recipient,
    },
    /// Download link: the links. The run starts at once; no `Go` follows.
    DownloadLink {
        /// The pasted links, each once, in the order given.
        urls: Vec<String>,
        /// Whose library the songs go into.
        recipient: Recipient,
    },
    /// Review: a choice per held song the owner decided on ("Download N chosen"). The run starts
    /// at once; no `Go` follows.
    Review {
        /// The decided songs.
        choices: Vec<ReviewChoice>,
    },
    /// Sync device: the device and what was chosen for it, including the library
    /// (`choice.library`); remembered per device, and as the last device. Answered with `Plan`.
    SyncDevice {
        /// The chosen device.
        device: Device,
        /// Library, direction, selection, folders and format.
        choice: DeviceChoice,
    },
    /// Convert: library, what, format, quality, cover and destination (remembered). Answered with
    /// `Plan`.
    Convert(ConvertChoice),
}

/// A request for something the form offers before it can be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Look at the libraries on disk again (Sync Spotify, Download link, Sync device, Convert).
    /// Answered with `Libraries`.
    Libraries,
    /// The playlists of this library, when the Library field changes (Sync device and Convert).
    /// Answered with `Playlists`. Never makes the plan out of date by itself: the form answers
    /// again with the new choice.
    Playlists(LibraryId),
    /// Sync device: look for devices again (the `r` key). Answered with `Devices`.
    Devices,
    /// Sync device: the top-level folders of `device` (folders directly in its root), for From
    /// device.
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

    /// Whether a `Cancel` is already queued behind the command just read (the owner stopped the
    /// task as it was starting).
    fn cancel_queued(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            self.waiting.push_back(command);
        }
        self.waiting.contains(&Command::Cancel)
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
        // The inbox first: a `Go` already sent is read before something that happened by itself
        // can make the plan stale and the `Go` ignored.
        let command = tokio::select! {
            biased;
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
                    Answered::Run => return run(step, events, &mut inbox).await,
                }
            }
            Some(Command::Go) => {
                if ready {
                    return run(step, events, &mut inbox).await;
                }
            }
        }
    }
}

/// Announce the run, then carry it out, unless the owner has stopped it. Nothing reads commands
/// once the run has started, so a `Cancel` is looked for twice: before the announcement (so a run
/// stopped in time never shows as started), and after it. [`crate::Runner::stop`] sends `Cancel`
/// and then reads whether the run has started; a stop that read "not started" just before the
/// announcement sent its `Cancel` before this second look, so it is never lost.
async fn run(
    step: &mut impl Step,
    events: &EventSender,
    inbox: &mut Inbox,
) -> Result<Flow, TaskError> {
    if inbox.cancel_queued() {
        return Ok(Flow::Cancelled);
    }
    events.send(EventKind::Started);
    if inbox.cancel_queued() {
        return Ok(Flow::Cancelled);
    }
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
        /// Something happens by itself all the time that makes the plan stale (a device goes).
        Stale,
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

        async fn background(&mut self) -> Result<bool, TaskError> {
            if self.quirk == Quirk::Stale {
                Ok(true)
            } else {
                pending().await
            }
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

    #[test]
    fn a_new_library_is_made_only_when_the_run_starts() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = Config {
            client_id: "id".into(),
            music_root: dir.path().to_path_buf(),
            my_library: "Me".into(),
            download_jobs: std::num::NonZeroUsize::MIN,
            animations: true,
        };
        let new = Recipient::New("S:am".into());
        let planned = new.library(&config)?;
        assert_eq!((planned.name(), planned.is_own()), ("Sam", false));
        assert!(!planned.root().exists(), "planning makes nothing");
        let made = new.make(&config)?;
        assert_eq!(made.root(), planned.root());
        assert!(made.root().is_dir());
        assert!(new.library(&config).is_err() && new.make(&config).is_err());
        assert!(Recipient::New("  ..".into()).library(&config).is_err());
        assert!(Recipient::Me.make(&config)?.is_own());
        let existing = Recipient::Existing("Sam".into()).make(&config)?;
        assert_eq!(existing.root(), made.root());
        Ok(())
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
            disk_image: false,
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
        let other = Command::Answer(Answers::DownloadLink {
            urls: vec!["u".into()],
            recipient: Recipient::Me,
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
    async fn a_cancel_sent_with_go_wins_over_it() -> TestResult {
        let mut step = Fake {
            can_start: true,
            ..Fake::default()
        };
        let (cancelled, started) =
            drive_with(&mut step, vec![convert(), Command::Go, Command::Cancel]).await?;
        assert!(cancelled && !started, "nothing ran");
        assert_eq!(step.log, ["open", "answer"]);
        let mut direct = Fake {
            run_at_once: true,
            ..Fake::default()
        };
        assert!(run(&mut direct, vec![convert(), Command::Cancel]).await?);
        assert_eq!(direct.log, ["open", "answer"]);
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
    async fn a_go_already_sent_is_read_before_anything_that_happens_by_itself() -> TestResult {
        // Without the inbox first, each round picks either side at random; 30 rounds make a
        // lost `Go` all but certain.
        for _ in 0..30 {
            let mut step = Fake {
                can_start: true,
                quirk: Quirk::Stale,
                ..Fake::default()
            };
            let (cancelled, started) = drive_with(&mut step, vec![convert(), Command::Go]).await?;
            assert!(!cancelled && started, "the Go was lost");
        }
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
