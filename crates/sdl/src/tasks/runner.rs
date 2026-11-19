//! The [`Runner`]: starts, feeds and stops one task per function, and keeps the state file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use sdl_core::paths::remove_tmp_dir_if_empty;
use sdl_core::state::StateError;
use sdl_core::{Config, ErrorInfo, Event, EventKind, EventSender, LastRun, State, TaskId};
use sdl_library::Card;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use super::failure::{failure_event, failure_line};
use super::{Command, TaskError, card, link, now_secs, spotify, task_name};

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
    /// It ran to the end; `line` is the last-run line.
    Done { line: String },
    /// The owner cancelled at a waiting step, before anything ran.
    Cancelled,
}

type SharedState = Arc<Mutex<State>>;
type CardSlot = Arc<Mutex<Option<Card>>>;

/// What a task body needs: its tagged event sender, the config, and the state file.
#[derive(Debug, Clone)]
pub(super) struct TaskContext {
    pub(super) events: EventSender,
    pub(super) config: Arc<Config>,
    state: SharedState,
    state_path: Arc<PathBuf>,
    card: CardSlot,
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

    /// End the run: write the last-run line (if any), then send the one terminal event. A failed
    /// save is reported after it, so the screen still sees how the run ended.
    fn end(&self, line: Option<String>, event: EventKind) {
        let task = self.task();
        let saved = line.map_or(Ok(()), |line| {
            self.update_state(|state| {
                state.last_run.insert(
                    task,
                    LastRun {
                        line,
                        at: now_secs(),
                    },
                );
            })
        });
        self.events.send(event);
        if let Err(error) = saved {
            self.events.send(EventKind::Error(state_error(&error)));
        }
    }

    /// Remember the mounted card so [`Runner::unmount_card`] can find it; `None` once unmounted.
    pub(super) fn set_card(&self, card: Option<Card>) {
        *self.card.lock().unwrap_or_else(PoisonError::into_inner) = card;
    }

    /// End the run of a task body that returned `result`.
    pub(super) fn conclude(&self, result: Result<Flow, TaskError>) {
        match result {
            Ok(Flow::Done { line }) => self.end(Some(line), EventKind::Finished),
            Ok(Flow::Cancelled) => self.end(None, EventKind::Stopped),
            Err(error) => {
                let event = failure_event(&error);
                self.end(Some(failure_line(&event)), event);
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
    card: CardSlot,
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
            card: Arc::default(),
            running: HashMap::new(),
        }
    }

    /// A copy of the state: the ticked lists and each function's last-run line.
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

    /// Start Sync Spotify.
    ///
    /// # Errors
    /// [`StartError::AlreadyRunning`].
    pub fn start_spotify(&mut self) -> Result<(), StartError> {
        self.spawn(TaskId::SyncSpotify, |context, commands| async move {
            let result = spotify::run(&context, commands).await;
            context.conclude(result);
        })
    }

    /// Start Download link for `url`.
    ///
    /// # Errors
    /// [`StartError::AlreadyRunning`].
    pub fn start_download_link(&mut self, url: String) -> Result<(), StartError> {
        self.spawn(TaskId::DownloadLink, |context, _commands| async move {
            let result = link::run(&context, &url).await;
            context.conclude(result);
        })
    }

    /// Start Sync card.
    ///
    /// # Errors
    /// [`StartError::AlreadyRunning`].
    pub fn start_card_sync(&mut self) -> Result<(), StartError> {
        self.spawn(TaskId::SyncCard, |context, commands| async move {
            let result = card::run(&context, commands).await;
            context.conclude(result);
        })
    }

    /// Stop `task`: abort it (child processes die through `kill_on_drop`, temp files are removed
    /// as the task is dropped), remove `.sdl-tmp` if empty, write the last-run line `stopped` and
    /// send `Stopped`. `false` when the task was not running.
    pub async fn stop(&mut self, task: TaskId) -> bool {
        let Some(running) = self.running.remove(&task) else {
            return false;
        };
        if running.handle.is_finished() {
            return false;
        }
        running.handle.abort();
        let _ = running.handle.await; // Err(cancelled) is the expected result of an abort.
        let context = running.context;
        context.end(Some(STOPPED_LINE.to_owned()), EventKind::Stopped);
        if let Err(error) = remove_tmp_dir_if_empty(&context.config.music_root) {
            context.events.send(EventKind::Error(ErrorInfo {
                message: format!("The temporary folder could not be removed: {error}"),
                advice: "Remove .sdl-tmp in the music folder by hand.".to_owned(),
            }));
        }
        true
    }

    /// After a card sync and the owner's confirmation: remove the dot-folders from the card and
    /// unmount it. The returned future does not borrow the runner, so it can run on its own.
    ///
    /// # Errors
    /// No card is mounted by this run, or cleanup or unmount failed.
    pub fn unmount_card(&self) -> impl Future<Output = Result<(), ErrorInfo>> + Send + 'static {
        let slot = Arc::clone(&self.card);
        let uuid = self.config.card_uuid.clone();
        async move {
            let card = slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or_else(|| ErrorInfo {
                    message: "No card was mounted by sdl.".to_owned(),
                    advice: "Run Sync card first.".to_owned(),
                })?;
            sdl_library::finish_card(&card, &uuid)
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
            card: Arc::clone(&self.card),
        };
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
            card_uuid: "uuid".into(),
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
        runner.spawn(TaskId::SyncCard, idle)?;
        assert!(runner.is_running(TaskId::SyncSpotify));
        assert!(runner.is_running(TaskId::SyncCard));
        assert!(!runner.is_running(TaskId::DownloadLink));
        Ok(())
    }

    #[tokio::test]
    async fn stop_cleans_temp_files_and_records_stopped() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        let root = runner.config.music_root.clone();
        runner.spawn(TaskId::SyncSpotify, |_context, _commands| {
            let temp = super::super::progress::download_dir(&root);
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
    async fn conclude_writes_the_last_run_before_the_terminal_event() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            context.conclude(Ok(Flow::Done {
                line: "2 added, 0 skipped".into(),
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
    async fn cancel_at_a_waiting_step_ends_with_stopped_and_no_last_run() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::SyncCard, |context, mut commands| async move {
            let go = super::super::progress::wait_for_go(&mut commands).await;
            context.conclude(Ok(if go {
                Flow::Done {
                    line: String::new(),
                }
            } else {
                Flow::Cancelled
            }));
        })?;
        assert!(runner.send(TaskId::SyncCard, Command::Cancel));
        let event = rx.recv().await.ok_or("no event")?;
        assert_eq!(event.kind, EventKind::Stopped);
        assert!(runner.state().last_run.is_empty());
        Ok(())
    }
}
