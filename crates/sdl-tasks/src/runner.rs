//! The [`Runner`]: starts, feeds and stops one task per function, keeps the enrichment worker
//! (F6) alive, and is the only writer of `pending_enrichment.toml`,
//! `state.toml` and `review.toml` (F11).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use sdl_core::StoreError;
use sdl_core::place::remove_tmp_dir_if_empty;
use sdl_core::run::CancellationToken;
use sdl_core::{
    Config, ConvertChoice, Device, DeviceChoice, EnrichmentCounts, ErrorInfo, Event, EventKind,
    EventSender, HeldSong, LastRun, Library, LibraryId, Never, PendingEnrichment, ReviewList,
    RunOutcome, State, TaskId, WantedSong,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::task::AbortOnDropHandle;

use crate::conversation::drive;
use crate::enrich::{Queue, work};
use crate::failure::{failure_event, failure_line};
use crate::progress::stopped_line;
use crate::tasks::{convert, device, link, review, spotify};
use crate::{Command, TaskError, now_secs};

/// What to do when a settings file cannot be written.
pub(crate) const STATE_ADVICE: &str = "Check that ~/.config/sdl can be written.";

/// Why a task could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The function is already running.
    #[error("{} is already running", .0.name())]
    AlreadyRunning(TaskId),
}

/// How a task body ended without failing.
#[derive(Debug)]
pub(crate) enum Flow {
    /// It ran to the end; `line` is the last-run line and `outcome` picks its colour.
    Done {
        /// The last-run line.
        line: String,
        /// How it ended.
        outcome: RunOutcome,
    },
    /// A cooperative stop (Convert, Sync device): the file in hand was finished, then the run
    /// ended. `line` says "stopped" with the count finished.
    Stopped {
        /// The last-run line.
        line: String,
    },
    /// The owner cancelled at a waiting step, before anything ran.
    Cancelled,
}

impl Flow {
    /// A run that went to the end with `counts`: its outcome is `NeedsAttention` when `counts` has
    /// any held or failed item.
    #[must_use]
    pub fn done(line: String, counts: &sdl_core::Counts) -> Self {
        Self::Done {
            line,
            outcome: RunOutcome::of_finished(counts),
        }
    }
}

type SharedState = Arc<Mutex<State>>;
type SharedReview = Arc<Mutex<ReviewList>>;
/// The device last synced, for [`Runner::eject_device`].
type DeviceSlot = Arc<Mutex<Option<Device>>>;

/// The tasks that are running, so the shared `.sdl-tmp` folder is removed only when none is.
#[derive(Debug, Default)]
pub(crate) struct Active(Mutex<HashSet<TaskId>>);

impl Active {
    fn enter(&self, task: TaskId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(task);
    }

    /// `task` is no longer running. When no task is left, remove `.sdl-tmp` if it is empty, holding
    /// the lock so no task can start using it meanwhile.
    fn leave(&self, task: TaskId, music_root: &Path) -> std::io::Result<()> {
        let mut running = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        running.remove(&task);
        if running.is_empty() {
            remove_tmp_dir_if_empty(music_root)
        } else {
            Ok(())
        }
    }
}

/// What a task body needs: its tagged event sender, the config, a stop token, and the only ways to
/// change `state.toml` and `review.toml`. Task bodies are written against this; the UI never sees
/// it.
#[derive(Debug, Clone)]
pub(crate) struct TaskContext {
    /// Sends events tagged with this task; also remembers counts, headline and attention items.
    pub events: EventSender,
    /// The config.
    pub config: Arc<Config>,
    /// Cancelled by [`Runner::stop`]; Convert and Sync device check it between files.
    pub stop: CancellationToken,
    state: SharedState,
    state_path: Arc<PathBuf>,
    review: SharedReview,
    review_path: Arc<PathBuf>,
    queue: Queue,
    device: DeviceSlot,
    /// Something to add to the run's last-run line, whatever way the run ended.
    line_note: Arc<Mutex<Option<String>>>,
    active: Arc<Active>,
}

impl TaskContext {
    fn task(&self) -> TaskId {
        self.events.task()
    }

    /// A copy of the current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A copy of the current review list.
    #[must_use]
    pub fn review(&self) -> ReviewList {
        self.review
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the song with `key` is held for `library` (read without copying the list).
    #[must_use]
    pub fn is_held(&self, library: &LibraryId, key: &str) -> bool {
        self.review
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(library, key)
    }

    /// Whether the song with `key` was marked "never download" in `library`.
    #[must_use]
    pub fn is_never(&self, library: &LibraryId, key: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_never(library, key)
    }

    /// Change the state and save it, holding the lock so concurrent tasks cannot save out of
    /// order. The change stays in memory even when the save fails.
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn update_state(&self, change: impl FnOnce(&mut State)) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut state);
        state.save_to(&self.state_path)
    }

    /// Change the review list, save it, and tell the UI the new number of held songs
    /// ([`EventKind::HeldCount`]) and, when `Review` is open, nothing else: the Review task
    /// sends its own list.
    ///
    /// # Errors
    /// The review file could not be written; the change stays in memory.
    pub fn update_review(&self, change: impl FnOnce(&mut ReviewList)) -> Result<(), StoreError> {
        let (saved, count) = {
            let mut review = self.review.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut review);
            (review.save_to(&self.review_path), review.len())
        };
        self.events.send(EventKind::HeldCount(count));
        saved
    }

    /// Hold a song for review (one call per held song of a sync or a link).
    ///
    /// # Errors
    /// As [`TaskContext::update_review`].
    pub fn hold(&self, song: HeldSong) -> Result<(), StoreError> {
        self.update_review(|review| review.hold(song))
    }

    /// Queue a song that has just landed in `library` for enrichment (F6): the background worker
    /// adds its genre and totals later.
    ///
    /// # Errors
    /// The queue file could not be written; the song is queued in memory all the same.
    pub fn queue_enrichment(&self, library: LibraryId, song: WantedSong) -> Result<(), StoreError> {
        self.queue.add(library, song)
    }

    /// Remember the ticked Spotify lists.
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn remember_ticked(&self, ids: Vec<String>) -> Result<(), StoreError> {
        self.update_state(|state| state.ticked_lists = ids)
    }

    /// Remember a device's choices, its name as shown ([`Device::label`], so the main panel can
    /// name it after a restart) and that it was the last one used.
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn remember_device(
        &self,
        device: &Device,
        mut choice: DeviceChoice,
    ) -> Result<(), StoreError> {
        choice.name = device.label();
        self.update_state(|state| {
            // The form does not know which library the device was last synced from; the saved
            // entry does.
            if let Some(saved) = state.devices.get(device.id()) {
                choice.last_synced.clone_from(&saved.last_synced);
            }
            state.devices.insert(device.id().to_owned(), choice);
            state.last_device = Some(device.id().to_owned());
        })
    }

    /// Remember that a To-device run copied `library` to `device` (F12: the form's note says when
    /// another library was copied there before).
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn remember_synced_library(
        &self,
        device: &Device,
        library: LibraryId,
    ) -> Result<(), StoreError> {
        self.update_state(|state| {
            state
                .devices
                .entry(device.id().to_owned())
                .or_default()
                .last_synced = Some(library);
        })
    }

    /// Remember that this function added to `library`, so its form preselects it next time (F12).
    /// The owner's own library is the "For me" toggle and is not remembered.
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn remember_friend(&self, library: &Library) -> Result<(), StoreError> {
        if library.is_own() {
            return Ok(());
        }
        let task = self.task();
        let name = library.name().to_owned();
        self.update_state(|state| {
            state.last_friend.insert(task, name);
        })
    }

    /// Remember the Convert choices.
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn remember_convert(&self, choice: ConvertChoice) -> Result<(), StoreError> {
        self.update_state(|state| state.convert = choice)
    }

    /// Add a song to the "never download" set of `library` (by `WantedSong::key`).
    ///
    /// # Errors
    /// The state file could not be written.
    pub fn mark_never(&self, library: LibraryId, key: String) -> Result<(), StoreError> {
        self.update_state(|state| {
            state.never.insert(Never { library, key });
        })
    }

    /// Remember the device a sync wrote to, so [`Runner::eject_device`] can find it.
    pub fn set_device(&self, synced: Option<Device>) {
        *self.device.lock().unwrap_or_else(PoisonError::into_inner) = synced;
    }

    /// Add `note` to the last-run line the run ends with; the run's own result is not changed.
    pub fn note_on_line(&self, note: String) {
        *self
            .line_note
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(note);
    }

    /// End the run: write the last-run line, outcome and attention items (if any), then send the
    /// one terminal event. A failed save is reported after it, so the screen still sees how the
    /// run ended.
    fn end(&self, last: Option<(String, RunOutcome)>, event: EventKind) {
        let task = self.task();
        let attention = self.events.snapshot().attention;
        let note = self
            .line_note
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let saved = last.map_or(Ok(()), |(line, outcome)| {
            let line = match note {
                Some(note) => format!("{line}; {note}"),
                None => line,
            };
            self.update_state(|state| {
                state.last_run.insert(
                    task,
                    LastRun {
                        line,
                        at: now_secs(),
                        outcome,
                        attention,
                    },
                );
            })
        });
        self.events.send(event);
        if let Err(error) = saved {
            self.events.send(EventKind::Problem(state_error(&error)));
        }
    }

    /// Leave the running set and remove `.sdl-tmp` if this was the last task; a failure is told to
    /// the owner after the run's own events.
    fn tidy(&self) {
        if let Err(error) = self.active.leave(self.task(), &self.config.music_root) {
            self.events.send(EventKind::Problem(ErrorInfo {
                message: format!("The temporary folder could not be removed: {error}"),
                advice: "Remove .sdl-tmp in the music folder by hand.".to_owned(),
            }));
        }
    }

    /// End the run of a task body that returned `result`. Every temp file is gone by now, because
    /// the body has returned.
    pub fn conclude(&self, result: Result<Flow, TaskError>) {
        self.conclude_events(result);
        self.tidy();
    }

    fn conclude_events(&self, result: Result<Flow, TaskError>) {
        match result {
            Ok(Flow::Done { line, outcome }) => {
                self.end(Some((line, outcome)), EventKind::Finished);
            }
            Ok(Flow::Stopped { line }) => {
                self.end(Some((line, RunOutcome::Stopped)), EventKind::Stopped);
            }
            Ok(Flow::Cancelled) => self.end(None, EventKind::Stopped),
            Err(error) => {
                let event = failure_event(&error);
                let outcome = match event {
                    EventKind::RateLimited { .. } => RunOutcome::Stopped,
                    _ => RunOutcome::Failed,
                };
                self.end(Some((failure_line(&event), outcome)), event);
            }
        }
    }
}

pub(crate) fn state_error(error: &StoreError) -> ErrorInfo {
    ErrorInfo {
        message: error.to_string(),
        advice: STATE_ADVICE.to_owned(),
    }
}

/// A task that is running: its handle, its way to receive commands and its context.
#[derive(Debug)]
struct Running {
    handle: JoinHandle<()>,
    commands: UnboundedSender<Command>,
    context: TaskContext,
}

/// Starts, feeds and stops the five functions' tasks, one task per function (different functions
/// run side by side). The only writer of `state.toml` and `review.toml`.
///
/// # Contract with the UI
///
/// The UI owns one `Runner`, built with the sending half of [`sdl_core::channel`], and reads the
/// receiving half. Every [`sdl_core::Event`] carries its [`sdl_core::TaskId`].
///
/// * [`Runner::start`] starts a task (or fails with [`StartError::AlreadyRunning`]); its first
///   event, a headline, arrives at once. The UI then drives the one step form with [`Command`]s
///   through [`Runner::send`]: `Answer` (as often as the owner changes a row), `Offer`, `Go`,
///   `Cancel`. Four forms choose a library (F12) and so also get `Libraries(..)` when they open and
///   answer `Offer::Libraries` with it again.
/// * [`Runner::stop`] ends a running task from outside (the `s` key, after its confirmation):
///   Sync Spotify, Download link and Review are aborted; Convert and Sync device finish the file in
///   hand, then end. Either way the last-run line says "stopped" with the count finished.
/// * [`Runner::discard`] ends a task whose run never started, as the app closes, leaving its
///   last-run line as it was.
/// * Each run ends with exactly one terminal event: `Finished`, `Stopped`, `Error` or
///   `RateLimited`. Before it, the function's last-run line (with its [`sdl_core::RunOutcome`] and
///   attention items) is already in [`Runner::state`]. The `Stopped` that answers
///   `Command::Cancel` (or a stop before the run started) writes no last-run line, because
///   nothing ran.
/// * While the task is at its form, a step that takes time (listing Spotify songs, making a plan)
///   sends `Headline`s saying what it is doing; the form shows the latest one, so nothing looks
///   hung. `Started` marks the end of the form: from then on the running panel shows the task.
///   Until the first `Counts` with a total, the panel shows a spinner.
///
/// ## Per function, the events after `start`
/// * **Sync Spotify**: `Headline`. With no working stored sign-in: `SignIn(NeedsBrowser)`, and the
///   task waits; `Go` continues to the browser sign-in (`SignIn(WaitingForBrowser)`), `Cancel`
///   ends it. `SignIn(KeychainProblem)` at most once. Then `SignIn(SignedIn)`, `Lists(..)`.
///   `Answer(SyncSpotify { lists, recipient })` (ticked ids are saved): `Headline`s while listing,
///   `NewOfTotal`. `Go`: `Started`, `Headline`s, `Counts` (a full snapshot after every song),
///   `Attention`, `HeldCount`, `Finished`.
/// * **Download link**: `Headline`. `Answer(DownloadLink { urls, recipient })` starts the run at once:
///   `Started`, `Headline`s (reading each link: no total yet), `Counts`, `Attention`, `HeldCount`,
///   `Finished`.
/// * **Review**: `Headline`; the held songs are [`Runner::review`]. `Answer(Review { choices })`
///   starts the run at once: `Started`, `Headline`s, `Counts`, `Attention`, `HeldCount`,
///   `Finished`.
/// * **Sync device**: `Headline`, `Devices(..)` (empty means none; sent again when a volume
///   appears or goes), `Libraries(..)`, `Playlists(..)` (the owner's own library's).
///   `Offer::Playlists(library)`: `Playlists` of that library. `Offer::Devices`: `Devices`. `Offer::Folders(device)`:
///   `Folders`. `Answer(SyncDevice { device, choice })` (saved per device): `Headline`, `Plan`.
///   `Go`: `Started`, `Headline`, `Counts`, `Attention`, `Finished`; the run ends with the
///   device's clean-up, still inside the task. Then the UI asks "Eject <device>?" and on yes
///   awaits [`Runner::eject_device`], showing that it is ejecting until the future ends.
/// * **Convert**: `Headline`, `Libraries(..)`, `Playlists(..)` (the owner's own library's);
///   `Offer::Playlists(library)` answers as in Sync device. `Answer(Convert(choice))` (saved): `Headline`,
///   `Plan`; a destination inside the library comes back as a plan with a `refusal`. `Go`:
///   `Started`, `Headline`, `Counts`, `Attention`, `Finished`.
///
/// A run counts only the work it does: songs already in the library, in any format, are settled
/// before it starts and appear only as "already there" in the last-run line, so the bar and the
/// time estimate are not thrown off by items that were never going to be touched.
///
/// `.sdl-tmp` is removed only when the last running task ends.
#[derive(Debug)]
pub struct Runner {
    config: Arc<Config>,
    state: SharedState,
    state_path: Arc<PathBuf>,
    review: SharedReview,
    review_path: Arc<PathBuf>,
    events: UnboundedSender<Event>,
    queue: Queue,
    /// The enrichment worker (F6), aborted when the runner is dropped so nothing outlasts the
    /// app. `None` in a runner made without one.
    enricher: Option<AbortOnDropHandle<()>>,
    device: DeviceSlot,
    active: Arc<Active>,
    running: HashMap<TaskId, Running>,
}

impl Runner {
    /// A runner that reads `~/.config/sdl/state.toml`, `review.toml` and
    /// `pending_enrichment.toml` (a missing file is empty), and starts the enrichment worker
    /// (F6), which lives until the runner is dropped. Call it inside the tokio runtime.
    ///
    /// # Errors
    /// A file that exists but cannot be read or parsed, naming its path.
    pub fn new(config: Config, events: UnboundedSender<Event>) -> Result<Self, StoreError> {
        let state_path = State::path()?;
        let review_path = ReviewList::path()?;
        let pending_path = PendingEnrichment::path()?;
        let state = State::load_from(&state_path)?;
        let review = ReviewList::load_from(&review_path)?;
        let pending = PendingEnrichment::load_from(&pending_path)?;
        let mut runner =
            Self::with_files(config, (state, state_path), (review, review_path), events);
        runner.queue = Queue::new(pending, pending_path);
        let worker = work(runner.queue.clone(), Arc::clone(&runner.config));
        runner.enricher = Some(AbortOnDropHandle::new(tokio::spawn(worker)));
        Ok(runner)
    }

    /// A runner over the given state and review (and where each is saved), with an empty
    /// enrichment queue saved beside the state file and no enrichment worker.
    #[must_use]
    pub(crate) fn with_files(
        config: Config,
        state: (State, PathBuf),
        review: (ReviewList, PathBuf),
        events: UnboundedSender<Event>,
    ) -> Self {
        let pending_path = state.1.with_file_name("pending_enrichment.toml");
        Self {
            config: Arc::new(config),
            queue: Queue::new(PendingEnrichment::default(), pending_path),
            enricher: None,
            state: Arc::new(Mutex::new(state.0)),
            state_path: Arc::new(state.1),
            review: Arc::new(Mutex::new(review.0)),
            review_path: Arc::new(review.1),
            events,
            device: Arc::default(),
            active: Arc::default(),
            running: HashMap::new(),
        }
    }

    /// A copy of the state as it is now. A task saves its choices and its last-run line before it
    /// reports, so reading this when an event arrives is always current.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many songs are missing a genre: waiting for the enrichment worker, and asked with no
    /// genre found (F6). With [`Runner::ask_enrichment_again`], the connection point for an
    /// enrichment command; nothing on screen uses either yet.
    #[must_use]
    pub fn enrichment_counts(&self) -> EnrichmentCounts {
        self.queue.counts()
    }

    /// Ask `MusicBrainz` again for every song that was asked and has no genre (F6): they wait
    /// again and the worker takes them in turn. Returns how many that is.
    ///
    /// # Errors
    /// The queue file could not be written; the songs are asked for again all the same.
    pub fn ask_enrichment_again(&self) -> Result<usize, StoreError> {
        self.queue.ask_again()
    }

    /// A copy of the review list as it is now (its length is the number on the Review row).
    #[must_use]
    pub fn review(&self) -> ReviewList {
        self.review
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether `task` is running (started, and not yet ended).
    #[must_use]
    pub fn is_running(&self, task: TaskId) -> bool {
        self.running
            .get(&task)
            .is_some_and(|running| !running.handle.is_finished())
    }

    /// Send a command to a running task. `false` when the task is not running.
    #[must_use = "false means the task is not running"]
    pub fn send(&self, task: TaskId, command: Command) -> bool {
        self.running
            .get(&task)
            .is_some_and(|running| running.commands.send(command).is_ok())
    }

    /// Start `task`.
    ///
    /// # Errors
    /// [`StartError::AlreadyRunning`] when it is running.
    pub fn start(&mut self, task: TaskId) -> Result<(), StartError> {
        self.spawn(task, move |context, commands| async move {
            let events = &context.events;
            let result = match task {
                TaskId::SyncSpotify => {
                    drive(&mut spotify::SyncSpotify::new(&context), events, commands).await
                }
                TaskId::DownloadLink => {
                    drive(&mut link::DownloadLink::new(&context), events, commands).await
                }
                TaskId::Review => drive(&mut review::Review::new(&context), events, commands).await,
                TaskId::SyncDevice => {
                    drive(&mut device::SyncDevice::new(&context), events, commands).await
                }
                TaskId::Convert => {
                    drive(&mut convert::Convert::new(&context), events, commands).await
                }
            };
            context.conclude(result);
        })
    }

    /// Stop a running task from outside. A task whose run has not started (still at its form) is
    /// cancelled: it ends with `Stopped` and its last-run line stays as it was. Once the run has
    /// started, Convert and Sync device are asked to stop ([`CancellationToken`]) and end themselves after
    /// the file in hand; the others are aborted (`JoinHandle::abort`; children die through
    /// `kill_on_drop`) and their last-run line is written here as `stopped after N of M`. `false`
    /// when the task was not running.
    pub async fn stop(&mut self, task: TaskId) -> bool {
        let Some(running) = self.running.get(&task) else {
            return false;
        };
        if running.handle.is_finished() {
            return false;
        }
        if !running.context.events.snapshot().started {
            let sent = running.commands.send(Command::Cancel).is_ok();
            // The run may have started between the check and the send, and then nothing reads the
            // `Cancel`. Look again after sending: a run that has started is stopped below, and one
            // that has not will find the `Cancel` when it starts (`conversation::run`).
            if !running.context.events.snapshot().started {
                return sent;
            }
        }
        if matches!(task, TaskId::Convert | TaskId::SyncDevice) {
            running.context.stop.cancel();
            return true;
        }
        let Some(context) = self.abort(task).await else {
            return false;
        };
        let line = stopped_line(&context.events.snapshot().counts);
        context.end(Some((line, RunOutcome::Stopped)), EventKind::Stopped);
        context.tidy();
        true
    }

    /// End a task whose run never started, as the app closes. Sends nothing and leaves the
    /// function's last-run line as it was.
    pub async fn discard(&mut self, task: TaskId) {
        if let Some(context) = self.abort(task).await {
            context.tidy();
        }
    }

    /// Wait until every task left has ended by itself, as the app closes: a Convert or Sync
    /// device that was asked to stop finishes the file in hand and saves its "stopped" line
    /// first (F11).
    pub async fn wait_all(&mut self) {
        for (_, running) in self.running.drain() {
            // A task that ended in a panic has nothing more to save.
            let _ = running.handle.await;
        }
    }

    async fn abort(&mut self, task: TaskId) -> Option<TaskContext> {
        let running = self.running.remove(&task)?;
        if running.handle.is_finished() {
            return None;
        }
        running.handle.abort();
        // `Err(cancelled)` is the expected result of an abort. `Ok` means the task ended by
        // itself before the abort took effect, and has already sent its own terminal event.
        running.handle.await.err().map(|_| running.context)
    }

    /// Eject the device the last Sync device run wrote to (`sdl_devices::eject`; the run has
    /// already cleaned it up). The returned future does not borrow the runner.
    ///
    /// # Errors
    /// No device was synced, or the unmount failed, in plain words.
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
            sdl_devices::eject(&device)
                .await
                .map_err(|error| crate::failure::error_info(&TaskError::from(error)))?;
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = None;
            Ok(())
        }
    }

    /// A context for `task`: its own event sender, everything else shared.
    pub(crate) fn context(&self, task: TaskId) -> TaskContext {
        TaskContext {
            events: EventSender::new(task, self.events.clone()),
            config: Arc::clone(&self.config),
            stop: CancellationToken::new(),
            state: Arc::clone(&self.state),
            state_path: Arc::clone(&self.state_path),
            review: Arc::clone(&self.review),
            review_path: Arc::clone(&self.review_path),
            queue: self.queue.clone(),
            device: Arc::clone(&self.device),
            line_note: Arc::default(),
            active: Arc::clone(&self.active),
        }
    }

    fn spawn<F, Fut>(&mut self, task: TaskId, body: F) -> Result<(), StartError>
    where
        F: FnOnce(TaskContext, UnboundedReceiver<Command>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.is_running(task) {
            return Err(StartError::AlreadyRunning(task));
        }
        let context = self.context(task);
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
    use sdl_core::place::tmp_dir;
    use sdl_core::{Attention, Counts, channel};
    use std::num::NonZeroUsize;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn runner(dir: &Path) -> (Runner, sdl_core::EventReceiver) {
        let config = Config {
            client_id: "id".into(),
            music_root: dir.join("music"),
            my_library: "Me".into(),
            download_jobs: NonZeroUsize::MIN,
            animations: true,
        };
        let (tx, rx) = channel();
        let runner = Runner::with_files(
            config,
            (State::default(), dir.join("state.toml")),
            (ReviewList::default(), dir.join("review.toml")),
            tx,
        );
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
    async fn stopping_a_download_task_aborts_it_and_records_how_far_it_got() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        let root = runner.config.music_root.clone();
        runner.spawn(TaskId::SyncSpotify, |context, _commands| {
            let scratch = crate::progress::scratch_dir(&root);
            async move {
                let _held = scratch;
                context.events.send(EventKind::Started);
                context.events.send(EventKind::Counts(Counts {
                    total: Some(10),
                    done: 4,
                    held: 1,
                    ..Counts::default()
                }));
                context.events.send(EventKind::Attention(Attention::held(
                    "T".into(),
                    "A".into(),
                    "no result".into(),
                )));
                std::future::pending::<()>().await;
            }
        })?;
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert!(tmp_dir(&runner.config.music_root).exists());
        assert!(runner.stop(TaskId::SyncSpotify).await);
        assert!(!tmp_dir(&runner.config.music_root).exists());
        assert!(!runner.is_running(TaskId::SyncSpotify));
        let mut last = None;
        while let Ok(event) = rx.try_recv() {
            last = Some(event);
        }
        let event = last.ok_or("no event")?;
        assert_eq!(
            (event.task, event.kind),
            (TaskId::SyncSpotify, EventKind::Stopped)
        );
        let state = runner.state();
        let run = state.last_run.get(&TaskId::SyncSpotify).ok_or("no line")?;
        assert_eq!(run.line, "stopped after 4 of 10");
        assert_eq!(run.outcome, RunOutcome::Stopped);
        assert_eq!(run.attention.len(), 1, "attention items are saved");
        let saved = State::load_from(&dir.path().join("state.toml"))?;
        assert_eq!(saved, runner.state());
        assert!(!runner.stop(TaskId::SyncSpotify).await);
        Ok(())
    }

    #[tokio::test]
    async fn stopping_a_cooperative_task_sets_the_flag_and_lets_it_finish() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::Convert, |context, _commands| async move {
            context.events.send(EventKind::Started);
            while !context.stop.is_cancelled() {
                tokio::task::yield_now().await;
            }
            context.conclude(Ok(Flow::Stopped {
                line: "stopped after 1 of 2".into(),
            }));
        })?;
        tokio::task::yield_now().await;
        assert!(runner.stop(TaskId::Convert).await);
        let mut event = rx.recv().await.ok_or("no event")?;
        if event.kind == EventKind::Started {
            event = rx.recv().await.ok_or("no event")?;
        }
        assert_eq!(event.kind, EventKind::Stopped);
        let line = runner
            .state()
            .last_run
            .get(&TaskId::Convert)
            .map(|run| (run.line.clone(), run.outcome));
        assert_eq!(
            line,
            Some(("stopped after 1 of 2".to_owned(), RunOutcome::Stopped))
        );
        assert!(!runner.stop(TaskId::Review).await);
        Ok(())
    }

    #[tokio::test]
    async fn waiting_at_quit_lets_a_cooperative_stop_save_its_line() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, _rx) = runner(dir.path());
        runner.spawn(TaskId::Convert, |context, _commands| async move {
            context.events.send(EventKind::Started);
            // The file in hand takes a while to finish after the stop.
            context.stop.cancelled().await;
            for _ in 0..50 {
                tokio::task::yield_now().await;
            }
            context.conclude(Ok(Flow::Stopped {
                line: "stopped after 1 of 2".into(),
            }));
        })?;
        tokio::task::yield_now().await;
        assert!(runner.stop(TaskId::Convert).await);
        runner.wait_all().await;
        let saved = State::load_from(&dir.path().join("state.toml"))?;
        let line = saved
            .last_run
            .get(&TaskId::Convert)
            .map(|run| run.line.clone());
        assert_eq!(line.as_deref(), Some("stopped after 1 of 2"));
        assert!(!runner.is_running(TaskId::Convert));
        Ok(())
    }

    #[tokio::test]
    async fn a_problem_after_the_run_ended_is_not_a_second_ending() -> TestResult {
        let dir = tempfile::tempdir()?;
        // The state file cannot be written: its folder is a file.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"")?;
        let (tx, mut rx) = channel();
        let mut runner = Runner::with_files(
            runner(dir.path()).0.config.as_ref().clone(),
            (State::default(), blocked.join("state.toml")),
            (ReviewList::default(), dir.path().join("review.toml")),
            tx,
        );
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            context.conclude(Ok(Flow::Done {
                line: "1 added".into(),
                outcome: RunOutcome::Done,
            }));
        })?;
        let ending = rx.recv().await.ok_or("no event")?;
        assert_eq!(ending.kind, EventKind::Finished);
        let after = rx.recv().await.ok_or("no event")?;
        assert!(
            matches!(after.kind, EventKind::Problem(_)),
            "{:?}",
            after.kind
        );
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
    async fn the_runner_is_the_only_writer_of_state_and_review() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::Review, |context, _commands| async move {
            let _ = context.mark_never(LibraryId::Own, "t1".into());
            let _ = context.remember_ticked(vec!["liked-songs".into()]);
        })?;
        runner.spawn(TaskId::SyncSpotify, |context, _commands| async move {
            let _ = context.update_review(|_| {});
        })?;
        let event = rx.recv().await.ok_or("no event")?;
        assert_eq!(event.kind, EventKind::HeldCount(0));
        tokio::task::yield_now().await;
        let state = runner.state();
        assert!(state.is_never(&LibraryId::Own, "t1"));
        assert_eq!(state.ticked_lists, ["liked-songs"]);
        assert_eq!(State::load_from(&dir.path().join("state.toml"))?, state);
        assert!(dir.path().join("review.toml").exists());
        Ok(())
    }

    #[tokio::test]
    async fn outcomes_follow_how_the_run_ended() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            let counts = Counts {
                held: 1,
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
            let result = drive(&mut Waiting, &context.events, commands).await;
            context.conclude(result);
        })?;
        assert!(runner.send(TaskId::SyncDevice, Command::Cancel));
        let event = rx.recv().await.ok_or("no event")?;
        assert_eq!(event.kind, EventKind::Stopped);
        assert_eq!(runner.state().last_run, std::collections::BTreeMap::new());
        Ok(())
    }

    #[tokio::test]
    async fn stopping_a_task_still_at_its_form_cancels_it_and_keeps_the_last_run() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        for task in [TaskId::DownloadLink, TaskId::SyncDevice] {
            runner.spawn(task, |context, commands| async move {
                let result = drive(&mut Waiting, &context.events, commands).await;
                context.conclude(result);
            })?;
            assert!(runner.stop(task).await);
            let event = rx.recv().await.ok_or("no event")?;
            assert_eq!((event.task, event.kind), (task, EventKind::Stopped));
        }
        assert_eq!(runner.state().last_run, std::collections::BTreeMap::new());
        Ok(())
    }

    #[test]
    fn a_remembered_device_keeps_its_name_for_the_next_start() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let device = Device {
            name: "DISC".into(),
            mount_point: "/Volumes/DISC".into(),
            volume_uuid: Some("U1".into()),
            free_bytes: 0,
            disk_image: true,
            not_mounted: None,
        };
        runner
            .context(TaskId::SyncDevice)
            .remember_device(&device, DeviceChoice::default())?;
        let reloaded = State::load_from(&dir.path().join("state.toml"))?;
        assert_eq!(reloaded.last_device_name(), Some("DISC (disk image)"));
        Ok(())
    }

    #[test]
    fn the_friend_last_added_to_is_remembered_but_the_owner_is_not() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::DownloadLink);
        context.remember_friend(&Library::at("Me", &dir.path().join("Me"), true))?;
        assert!(runner.state().last_friend.is_empty());
        context.remember_friend(&Library::at("Sam", &dir.path().join("Sam"), false))?;
        let reloaded = State::load_from(&dir.path().join("state.toml"))?;
        assert_eq!(
            reloaded
                .last_friend
                .get(&TaskId::DownloadLink)
                .map(String::as_str),
            Some("Sam")
        );
        Ok(())
    }

    fn held(key: &str) -> HeldSong {
        HeldSong {
            library: LibraryId::Own,
            song: sdl_core::WantedSong {
                source: sdl_core::SongSource::Spotify {
                    track_id: key.into(),
                },
                title: key.into(),
                artists: vec!["A".into()],
                album: None,
                album_artist: None,
                track_number: None,
                disc_number: None,
                date: None,
                isrc: None,
                duration: None,
                cover_url: None,
            },
            from: Vec::new(),
            from_files: Vec::new(),
            candidates: Vec::new(),
            reason: "no result".into(),
        }
    }

    #[tokio::test]
    async fn a_sync_holding_songs_and_a_review_removing_landed_ones_lose_neither() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let sync = runner.context(TaskId::SyncSpotify);
        let review = runner.context(TaskId::Review);
        for n in 0..200 {
            sync.hold(held(&format!("old{n}")))?;
        }
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let adding = {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || -> Result<(), StoreError> {
                barrier.wait();
                for n in 0..200 {
                    sync.hold(held(&format!("new{n}")))?;
                }
                Ok(())
            })
        };
        let removing = std::thread::spawn(move || -> Result<(), StoreError> {
            barrier.wait();
            for n in 0..200 {
                review.update_review(|list| {
                    list.resolve(&LibraryId::Own, &format!("old{n}"));
                })?;
            }
            Ok(())
        });
        adding.join().map_err(|_| "the adding thread panicked")??;
        removing
            .join()
            .map_err(|_| "the removing thread panicked")??;
        let list = runner.review();
        assert_eq!(
            list.len(),
            200,
            "every new song kept, every landed one gone"
        );
        assert!((0..200).all(|n| list.contains(&LibraryId::Own, &format!("new{n}"))));
        assert!((0..200).all(|n| !list.contains(&LibraryId::Own, &format!("old{n}"))));
        assert_eq!(
            ReviewList::load_from(&dir.path().join("review.toml"))?,
            list
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_last_runs_attention_items_come_back_after_a_restart() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.spawn(TaskId::DownloadLink, |context, _commands| async move {
            let item = Attention::failed("T".into(), "A".into(), "gone".into());
            context.events.send(EventKind::Attention(item));
            let counts = Counts {
                failed: 1,
                ..Counts::default()
            };
            context.conclude(Ok(Flow::done("0 added, 1 needs attention".into(), &counts)));
        })?;
        while rx.recv().await.ok_or("no end")?.kind != EventKind::Finished {}
        let (restarted, _rx) = {
            let (tx, rx) = channel();
            let config = runner.config.as_ref().clone();
            let runner = Runner::with_files(
                config,
                (
                    State::load_from(&dir.path().join("state.toml"))?,
                    dir.path().join("state.toml"),
                ),
                (
                    ReviewList::load_from(&dir.path().join("review.toml"))?,
                    dir.path().join("review.toml"),
                ),
                tx,
            );
            (runner, rx)
        };
        let run = restarted
            .state()
            .last_run
            .get(&TaskId::DownloadLink)
            .cloned();
        let run = run.ok_or("no last run after the restart")?;
        assert_eq!(run.outcome, RunOutcome::NeedsAttention);
        assert_eq!(run.attention.len(), 1);
        assert_eq!(
            run.attention.first().map(|a| a.reason.as_str()),
            Some("gone")
        );
        Ok(())
    }

    /// A step whose every answer starts the run, and whose run never ends by itself.
    struct Runs;

    impl crate::conversation::Step for Runs {
        fn open(&mut self) -> impl Future<Output = Result<crate::conversation::Opened, TaskError>> {
            std::future::ready(Ok(crate::conversation::Opened::Ready))
        }

        fn answer(
            &mut self,
            _answers: crate::conversation::Answers,
        ) -> impl Future<Output = Result<crate::conversation::Answered, TaskError>> {
            std::future::ready(Ok(crate::conversation::Answered::Run))
        }

        fn go(&mut self) -> impl Future<Output = Result<Flow, TaskError>> {
            std::future::pending()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stop_at_the_instant_a_run_starts_still_stops_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        for _ in 0..300 {
            let (mut runner, mut rx) = runner(dir.path());
            runner.spawn(TaskId::DownloadLink, |context, commands| async move {
                let result = drive(&mut Runs, &context.events, commands).await;
                context.conclude(result);
            })?;
            // The answer starts the run on another thread while this one stops it: whichever way
            // the two interleave, the run must end with `Stopped` and not carry on for ever.
            let answer = Command::Answer(crate::conversation::Answers::DownloadLink {
                urls: vec!["u".into()],
                recipient: crate::conversation::Recipient::Me,
            });
            assert!(runner.send(TaskId::DownloadLink, answer));
            runner.stop(TaskId::DownloadLink).await;
            let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while let Some(event) = rx.recv().await {
                    if event.kind == EventKind::Stopped {
                        return true;
                    }
                }
                false
            })
            .await;
            assert_eq!(ended, Ok(true), "the stop was lost and the run carried on");
        }
        Ok(())
    }

    /// A step that only waits to be told to go.
    struct Waiting;

    impl crate::conversation::Step for Waiting {
        fn open(&mut self) -> impl Future<Output = Result<crate::conversation::Opened, TaskError>> {
            std::future::ready(Ok(crate::conversation::Opened::Ready))
        }

        fn answer(
            &mut self,
            _answers: crate::conversation::Answers,
        ) -> impl Future<Output = Result<crate::conversation::Answered, TaskError>> {
            std::future::ready(Ok(crate::conversation::Answered::Ignored))
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
}
