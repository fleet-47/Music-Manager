//! The [`Runner`]: starts, feeds and stops one task per function, and keeps the state file.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use sdl_core::paths::remove_tmp_dir_if_empty;
use sdl_core::state::StateError;
use sdl_core::{
    Config, ConvertChoice, Counts, Device, DeviceChoice, ErrorInfo, Event, EventKind, EventSender,
    FormatChoice, LastRun, RunOutcome, State, TaskId,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use super::failure::{failure_event, failure_line};
use super::form::drive;
use super::{Command, TaskError, convert, device, link, now_secs, spotify, task_name};

/// What to do when the state file cannot be written.
pub(super) const STATE_ADVICE: &str = "Check that ~/.config/sdl can be written.";

/// The last-run line of a stopped task.
const STOPPED_LINE: &str = "stopped";

/// Why a task could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The function is already running.
    #[error("{} is already running", task_name(*.0))]
    AlreadyRunning(TaskId),
}

/// How a task body ended without failing.
#[derive(Debug)]
pub(super) enum Flow {
    /// It ran to the end; `line` is the last-run line and `outcome` picks its colour.
    Done { line: String, outcome: RunOutcome },
    /// The owner cancelled at a waiting step, before anything ran.
    Cancelled,
}

impl Flow {
    /// A run that went to the end with `counts`: its outcome is `NeedsAttention` when `counts` has
    /// any unmatched or failed item.
    pub(super) fn done(line: String, counts: &Counts) -> Self {
        Self::Done {
            line,
            outcome: RunOutcome::of_finished(counts),
        }
    }
}

type SharedState = Arc<Mutex<State>>;
type DeviceSlot = Arc<Mutex<Option<Device>>>;

/// The tasks that are running, so the shared `.sdl-tmp` folder is removed only when none is.
#[derive(Debug, Default)]
pub(super) struct Active(Mutex<HashSet<TaskId>>);

impl Active {
    fn enter(&self, task: TaskId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(task);
    }

    /// `task` is no longer running. When no task is left, remove `.sdl-tmp` if it is empty, holding
    /// the lock so no task can start using it meanwhile.
    fn leave(&self, task: TaskId, music_root: &std::path::Path) -> std::io::Result<()> {
        let mut running = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        running.remove(&task);
        if running.is_empty() {
            remove_tmp_dir_if_empty(music_root)
        } else {
            Ok(())
        }
    }
}

/// What a task body needs: its tagged event sender, the config, and the state file.
#[derive(Debug, Clone)]
pub(super) struct TaskContext {
    pub(super) events: EventSender,
    pub(super) config: Arc<Config>,
    state: SharedState,
    state_path: Arc<PathBuf>,
    device: DeviceSlot,
    active: Arc<Active>,
}

impl TaskContext {
    fn task(&self) -> TaskId {
        self.events.task()
    }

    /// Change the state and save it, holding the lock so concurrent tasks cannot save out of order.
    /// The change stays in memory even when the save fails.
    fn update_state(&self, change: impl FnOnce(&mut State)) -> Result<(), StateError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut state);
        state.save_to(&self.state_path)
    }

    /// Remember the ticked Spotify lists.
    pub(super) fn save_ticked(&self, ids: Vec<String>) -> Result<(), StateError> {
        self.update_state(|state| state.ticked_lists = ids)
    }

    /// Remember the format rows: the format becomes this function's default when `remember` is set.
    pub(super) fn remember_format(&self, choice: FormatChoice) -> Result<(), StateError> {
        let task = self.task();
        self.update_state(|state| {
            if choice.remember {
                state.formats.insert(task, choice.format);
            }
        })
    }

    /// Remember a device's folders, direction and format.
    pub(super) fn remember_device(
        &self,
        device: &Device,
        choice: DeviceChoice,
    ) -> Result<(), StateError> {
        self.update_state(|state| {
            state.devices.insert(device.id().to_owned(), choice);
        })
    }

    /// Remember the Convert choices.
    pub(super) fn remember_convert(&self, choice: ConvertChoice) -> Result<(), StateError> {
        self.update_state(|state| state.convert = choice)
    }

    /// End the run: write the last-run line and outcome (if any), then send the one terminal event.
    /// A failed save is reported after it, so the screen still sees how the run ended.
    fn end(&self, last: Option<(String, RunOutcome)>, event: EventKind) {
        let task = self.task();
        let saved = last.map_or(Ok(()), |(line, outcome)| {
            self.update_state(|state| {
                state.last_run.insert(
                    task,
                    LastRun {
                        line,
                        at: now_secs(),
                        outcome,
                    },
                );
            })
        });
        self.events.send(event);
        if let Err(error) = saved {
            self.events.send(EventKind::Error(state_error(&error)));
        }
    }

    /// Remember the device a sync wrote to, so [`Runner::eject_device`] can find it.
    pub(super) fn set_device(&self, device: Option<Device>) {
        *self.device.lock().unwrap_or_else(PoisonError::into_inner) = device;
    }

    /// Leave the running set and remove `.sdl-tmp` if this was the last task; a failure is told to
    /// the owner after the run's own events.
    fn tidy(&self) {
        if let Err(error) = self.active.leave(self.task(), &self.config.music_root) {
            self.events.send(EventKind::Error(ErrorInfo {
                message: format!("The temporary folder could not be removed: {error}"),
                advice: "Remove .sdl-tmp in the music folder by hand.".to_owned(),
            }));
        }
    }

    /// End the run of a task body that returned `result`. Every temp file is gone by now, because
    /// the body has returned.
    pub(super) fn conclude(&self, result: Result<Flow, TaskError>) {
        self.conclude_events(result);
        self.tidy();
    }

    fn conclude_events(&self, result: Result<Flow, TaskError>) {
        match result {
            Ok(Flow::Done { line, outcome }) => {
                self.end(Some((line, outcome)), EventKind::Finished);
            }
            Ok(Flow::Cancelled) => self.end(None, EventKind::Stopped),
            Err(error) => {
                let event = failure_event(&error);
                let outcome = match event {
                    EventKind::RateLimited { .. } => RunOutcome::NeedsAttention,
                    _ => RunOutcome::Failed,
                };
                self.end(Some((failure_line(&event), outcome)), event);
            }
        }
    }
}

pub(super) fn state_error(error: &StateError) -> ErrorInfo {
    ErrorInfo {
        message: error.to_string(),
        advice: STATE_ADVICE.to_owned(),
    }
}

/// A task that is running.
#[derive(Debug)]
struct Running {
    handle: JoinHandle<()>,
    commands: UnboundedSender<Command>,
    context: TaskContext,
}

/// Starts, feeds and stops the tasks; one per function.
#[derive(Debug)]
pub struct Runner {
    config: Arc<Config>,
    state: SharedState,
    state_path: Arc<PathBuf>,
    events: UnboundedSender<Event>,
    device: DeviceSlot,
    active: Arc<Active>,
    running: HashMap<TaskId, Running>,
}

impl Runner {
    /// A runner that sends on `events` (the sender from [`sdl_core::channel`]) and keeps
    /// `~/.config/sdl/state.toml`, loaded now.
    ///
    /// # Errors
    /// The state file is unreadable or malformed.
    pub fn new(config: Config, events: UnboundedSender<Event>) -> Result<Self, StateError> {
        let path = State::path()?;
        let state = State::load()?;
        Ok(Self::with_state(config, state, path, events))
    }

    fn with_state(
        config: Config,
        state: State,
        state_path: PathBuf,
        events: UnboundedSender<Event>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            state: Arc::new(Mutex::new(state)),
            state_path: Arc::new(state_path),
            events,
            device: Arc::default(),
            active: Arc::default(),
            running: HashMap::new(),
        }
    }

    /// A copy of the state: everything remembered, including each function's last-run line.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether `task` is running.
    #[must_use]
    pub fn is_running(&self, task: TaskId) -> bool {
        self.running
            .get(&task)
            .is_some_and(|running| !running.handle.is_finished())
    }

    /// Send a [`Command`] to a running task. `false` when the task is not running.
    #[must_use = "false means the task is not running"]
    pub fn send(&self, task: TaskId, command: Command) -> bool {
        self.running
            .get(&task)
            .is_some_and(|running| running.commands.send(command).is_ok())
    }

    /// Start `task`. It sends its first event at once and then waits for the form's
    /// [`Command`]s.
    ///
    /// # Errors
    /// [`StartError::AlreadyRunning`].
    pub fn start(&mut self, task: TaskId) -> Result<(), StartError> {
        self.spawn(task, move |context, commands| async move {
            let result = match task {
                TaskId::SyncSpotify => drive(&mut spotify::Spotify::new(&context), commands).await,
                TaskId::DownloadLink => drive(&mut link::Link::new(&context), commands).await,
                TaskId::Convert => drive(&mut convert::Convert::new(&context), commands).await,
                TaskId::SyncDevice => drive(&mut device::DeviceSync::new(&context), commands).await,
            };
            context.conclude(result);
        })
    }

    /// Stop `task`: abort it (child processes die through `kill_on_drop`, temp files are removed
    /// as the task is dropped), write the last-run line `stopped`, send `Stopped`, and remove
    /// `.sdl-tmp` if empty and no other task runs. `false` when the task was not running.
    pub async fn stop(&mut self, task: TaskId) -> bool {
        let Some(context) = self.abort(task).await else {
            return false;
        };
        context.end(
            Some((STOPPED_LINE.to_owned(), RunOutcome::Stopped)),
            EventKind::Stopped,
        );
        context.tidy();
        true
    }

    /// End `task` as the app closes, without a last-run line or an event: used for a task whose
    /// run the owner never started, so its last-run line keeps the previous run.
    pub async fn discard(&mut self, task: TaskId) {
        if let Some(context) = self.abort(task).await {
            context.tidy();
        }
    }

    /// Abort `task` and wait until it is gone; its context, or `None` when it was not running.
    async fn abort(&mut self, task: TaskId) -> Option<TaskContext> {
        let running = self.running.remove(&task)?;
        if running.handle.is_finished() {
            return None;
        }
        running.handle.abort();
        let _ = running.handle.await; // Err(cancelled) is the expected result of an abort.
        Some(running.context)
    }

    /// After a device sync and the owner's confirmation: remove the dot-folders from the device and
    /// unmount it. The returned future does not borrow the runner, so it can run on its own.
    ///
    /// # Errors
    /// No device was synced by this run, or cleanup or unmount failed.
    pub fn eject_device(&self) -> impl Future<Output = Result<(), ErrorInfo>> + Send + 'static {
        let slot = Arc::clone(&self.device);
        async move {
            let device = slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or_else(|| ErrorInfo {
                    message: "No device was synced by sdl.".to_owned(),
                    advice: "Run Sync device first.".to_owned(),
                })?;
            sdl_library::devices::eject(&device)
                .await
                .map_err(|error| super::failure::error_info(&TaskError::from(error)))?;
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
            Ok(())
        }
    }

    /// Spawn `body` as the task for `task`, unless that function is already running.
    fn spawn<F, Fut>(&mut self, task: TaskId, body: F) -> Result<(), StartError>
    where
        F: FnOnce(TaskContext, UnboundedReceiver<Command>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.is_running(task) {
            return Err(StartError::AlreadyRunning(task));
        }
        let context = TaskContext {
            events: EventSender::new(task, self.events.clone()),
            config: Arc::clone(&self.config),
            state: Arc::clone(&self.state),
            state_path: Arc::clone(&self.state_path),
            device: Arc::clone(&self.device),
            active: Arc::clone(&self.active),
        };
        context.active.enter(task);
        let (commands, receiver) = unbounded_channel();
        let handle = tokio::spawn(body(context.clone(), receiver));
        self.running.insert(
            task,
            Running {
                handle,
                commands,
                context,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl_core::channel;
    use sdl_core::paths::tmp_dir;
    use std::num::NonZeroUsize;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn runner(dir: &std::path::Path) -> (Runner, sdl_core::EventReceiver) {
        let config = Config {
            client_id: "id".into(),
            music_root: dir.join("music"),
            download_jobs: NonZeroUsize::MIN,
        };
        let (tx, rx) = channel();
        let runner = Runner::with_state(config, State::default(), dir.join("state.toml"), tx);
        (runner, rx)
    }

    #[tokio::test]
    async fn the_same_function_cannot_run_twice_but_others_can() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, _rx) = runner(dir.path());
        let idle = |_context, _commands| std::future::pending::<()>();
        runner.spawn(TaskId::SyncSpotify, idle)?;
        assert!(matches!(
            runner.spawn(TaskId::SyncSpotify, idle),
            Err(StartError::AlreadyRunning(TaskId::SyncSpotify))
        ));
        runner.spawn(TaskId::SyncDevice, idle)?;
        assert!(runner.is_running(TaskId::SyncSpotify));
        assert!(runner.is_running(TaskId::SyncDevice));
        assert!(!runner.is_running(TaskId::DownloadLink));
        Ok(())
    }

    #[tokio::test]
    async fn stop_cleans_temp_files_and_records_stopped() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        let root = runner.config.music_root.clone();
        runner.spawn(TaskId::SyncSpotify, |_context, _commands| {
            let temp = super::super::progress::download_dir(&root, sdl_core::Format::Opus);
            async move {
                let _held = temp;
                std::future::pending::<()>().await;
            }
        })?;
        tokio::task::yield_now().await;
        assert!(tmp_dir(&runner.config.music_root).exists());
        assert!(runner.stop(TaskId::SyncSpotify).await);
        assert!(!tmp_dir(&runner.config.music_root).exists());
        assert!(!runner.is_running(TaskId::SyncSpotify));
        let event = rx.try_recv()?;
        assert_eq!(event.task, TaskId::SyncSpotify);
        assert_eq!(event.kind, EventKind::Stopped);
        let line = runner
            .state()
            .last_run
            .get(&TaskId::SyncSpotify)
            .map(|last| last.line.clone());
        assert_eq!(line.as_deref(), Some("stopped"));
        let saved = State::parse(
            &std::fs::read_to_string(dir.path().join("state.toml"))?,
            &dir.path().join("state.toml"),
        )?;
        assert_eq!(saved, runner.state());
        assert!(!runner.stop(TaskId::SyncSpotify).await);
        Ok(())
    }

    #[tokio::test]
    async fn discard_keeps_the_last_run_and_sends_nothing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        std::fs::create_dir_all(tmp_dir(&runner.config.music_root))?;
        runner.spawn(TaskId::SyncDevice, |_context, _commands| {
            std::future::pending::<()>()
        })?;
        runner.discard(TaskId::SyncDevice).await;
        assert!(!runner.is_running(TaskId::SyncDevice));
        assert!(rx.try_recv().is_err());
        assert_eq!(runner.state().last_run, std::collections::BTreeMap::new());
        assert!(!tmp_dir(&runner.config.music_root).exists());
        Ok(())
    }

    #[tokio::test]
    async fn conclude_writes_the_last_run_before_the_terminal_event() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            context.conclude(Ok(Flow::Done {
                line: "2 added, 0 skipped".into(),
                outcome: RunOutcome::Done,
            }));
        })?;
        let event = rx.recv().await.ok_or("no event")?;
        assert_eq!(
            (event.task, event.kind),
            (TaskId::DownloadLink, EventKind::Finished)
        );
        let line = runner
            .state()
            .last_run
            .get(&TaskId::DownloadLink)
            .map(|last| last.line.clone());
        assert_eq!(line.as_deref(), Some("2 added, 0 skipped"));
        Ok(())
    }

    #[tokio::test]
    async fn the_format_is_saved_only_when_remember_is_ticked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, _rx) = runner(dir.path());
        let chosen = |remember| FormatChoice {
            format: sdl_core::Format::M4a,
            remember,
        };
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            let _ = context.remember_format(chosen(false));
        })?;
        tokio::task::yield_now().await;
        assert_eq!(runner.state().formats, std::collections::BTreeMap::new());
        runner.spawn(TaskId::SyncSpotify, |context, _commands| async move {
            let _ = context.remember_format(chosen(true));
        })?;
        tokio::task::yield_now().await;
        assert_eq!(
            runner.state().formats.get(&TaskId::SyncSpotify),
            Some(&sdl_core::Format::M4a)
        );
        Ok(())
    }

    #[tokio::test]
    async fn outcomes_follow_how_the_run_ended() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            let counts = Counts {
                unmatched: 1,
                ..Counts::default()
            };
            context.conclude(Ok(Flow::done("1 need attention".into(), &counts)));
        })?;
        rx.recv().await.ok_or("no event")?;
        runner.spawn(TaskId::Convert, |context, _commands| async move {
            context.conclude(Err(TaskError::Library(
                sdl_library::LibraryError::FfmpegNotFound,
            )));
        })?;
        rx.recv().await.ok_or("no event")?;
        let state = runner.state();
        let outcome = |task| state.last_run.get(&task).map(|run| run.outcome);
        assert_eq!(
            outcome(TaskId::DownloadLink),
            Some(RunOutcome::NeedsAttention)
        );
        assert_eq!(outcome(TaskId::Convert), Some(RunOutcome::Failed));
        Ok(())
    }

    #[tokio::test]
    async fn cancel_at_a_waiting_step_ends_with_stopped_and_no_last_run() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::SyncDevice, |context, commands| async move {
            let result = drive(&mut Waiting, commands).await;
            context.conclude(result);
        })?;
        assert!(runner.send(TaskId::SyncDevice, Command::Cancel));
        let event = rx.recv().await.ok_or("no event")?;
        assert_eq!(event.kind, EventKind::Stopped);
        assert_eq!(runner.state().last_run, std::collections::BTreeMap::new());
        Ok(())
    }

    /// A step that only waits to be told to go.
    struct Waiting;

    impl super::super::form::Step for Waiting {
        fn open(&mut self) -> impl Future<Output = Result<(), TaskError>> {
            std::future::ready(Ok(()))
        }

        fn answer(
            &mut self,
            _answers: super::super::form::Answers,
        ) -> impl Future<Output = Result<super::super::form::Answered, TaskError>> {
            std::future::ready(Ok(super::super::form::Answered::Ignored))
        }

        fn go(&mut self) -> impl Future<Output = Result<Flow, TaskError>> {
            std::future::ready(Ok(Flow::Cancelled))
        }
    }

    #[test]
    fn the_tmp_folder_goes_only_when_the_last_task_leaves() -> TestResult {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(tmp_dir(root.path()))?;
        let active = Active::default();
        active.enter(TaskId::SyncSpotify);
        active.enter(TaskId::SyncDevice);
        active.leave(TaskId::SyncSpotify, root.path())?;
        assert!(tmp_dir(root.path()).exists());
        active.leave(TaskId::SyncDevice, root.path())?;
        assert!(!tmp_dir(root.path()).exists());
        Ok(())
    }

    #[tokio::test]
    async fn a_finished_task_leaves_tmp_alone_while_another_runs() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        let root = runner.config.music_root.clone();
        std::fs::create_dir_all(tmp_dir(&root))?;
        runner.spawn(TaskId::SyncDevice, |_context, _commands| {
            std::future::pending::<()>()
        })?;
        runner.spawn(TaskId::Convert, |context, _commands| async move {
            context.conclude(Ok(Flow::Cancelled));
        })?;
        rx.recv().await.ok_or("no event")?;
        assert!(tmp_dir(&root).exists());
        assert!(runner.stop(TaskId::SyncDevice).await);
        assert!(!tmp_dir(&root).exists());
        Ok(())
    }
}
