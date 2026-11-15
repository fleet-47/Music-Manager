//! The single app state, shared by all screens. Updated only from key events and worker events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use sdl_core::{Config, Event, SignInState, Stage, Status};
use tui_input::Input;

use crate::screens::{self, Screen};

/// Run-table filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    /// Every song.
    #[default]
    All,
    /// Failed songs only.
    Failed,
    /// Unmatched songs only.
    Unmatched,
}

impl StatusFilter {
    /// The next filter in the cycle all -> failed -> unmatched -> all.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Failed,
            Self::Failed => Self::Unmatched,
            Self::Unmatched => Self::All,
        }
    }

    /// Whether a song with this status is shown under the filter.
    #[must_use]
    pub fn matches(self, status: &Status) -> bool {
        match self {
            Self::All => true,
            Self::Failed => matches!(status, Status::Failed(_)),
            Self::Unmatched => matches!(status, Status::Unmatched),
        }
    }

    /// Display name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Failed => "failed",
            Self::Unmatched => "unmatched",
        }
    }
}

/// One row of the run table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongRow {
    /// Row key (`Track::key`, or the item title for a link download).
    pub key: String,
    /// Song title.
    pub title: String,
    /// Artists, joined.
    pub artist: String,
    /// List name.
    pub list: String,
    /// Current stage.
    pub stage: Stage,
    /// Current status.
    pub status: Status,
}

/// What the event loop must do after a key press. The app itself never spawns work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    None,
    /// Start the Spotify sync.
    SyncSpotify,
    /// Download this link.
    DownloadLink(String),
    /// Sync the card.
    SyncCard,
    /// Clean the dot-folders and unmount the card mounted here.
    FinishCard(PathBuf),
    /// The user left the Run screen: stop any running work.
    Back,
}

/// All UI state.
#[derive(Debug)]
pub struct App {
    /// Loaded config.
    pub config: Config,
    /// Current screen.
    pub screen: Screen,
    /// Highlighted Home menu entry.
    pub home_selected: usize,
    /// Text input for the link on the Home screen.
    pub link_input: Input,
    /// Songs in the run table.
    pub rows: Vec<SongRow>,
    /// Row key to index in `rows`.
    pub row_index: HashMap<String, usize>,
    /// Run-table filter.
    pub filter: StatusFilter,
    /// Run-table selection and scroll position (indexes the filtered rows).
    pub table_state: TableState,
    /// Overall progress `(done, total)`.
    pub progress: (usize, usize),
    /// Current-activity line.
    pub activity: String,
    /// Spotify sign-in state, when a sign-in has started.
    pub sign_in: Option<SignInState>,
    /// Set when a Spotify 429 ended the run.
    pub rate_limited: bool,
    /// The 429's `Retry-After` wait, when Spotify sent one.
    pub retry_after: Option<Duration>,
    /// Card mount point after [`sdl_core::Event::CardSynced`], while the cleanup-and-unmount
    /// confirmation is pending.
    pub card_mount: Option<PathBuf>,
    /// The error that ended the run, if any.
    pub error: Option<String>,
    /// A worker is running.
    pub running: bool,
    /// Set when the user chose Quit.
    pub quit: bool,
}

impl App {
    /// A fresh app on the Home screen.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            screen: Screen::Home,
            home_selected: 0,
            link_input: Input::default(),
            rows: Vec::new(),
            row_index: HashMap::new(),
            filter: StatusFilter::default(),
            table_state: TableState::default(),
            progress: (0, 0),
            activity: String::new(),
            sign_in: None,
            rate_limited: false,
            retry_after: None,
            card_mount: None,
            error: None,
            running: false,
            quit: false,
        }
    }

    /// Rows that pass the current filter.
    #[must_use]
    pub fn visible_rows(&self) -> Vec<&SongRow> {
        self.rows
            .iter()
            .filter(|row| self.filter.matches(&row.status))
            .collect()
    }

    /// Reset the run state and switch to the Run screen.
    pub fn begin_run(&mut self, activity: &str) {
        self.rows.clear();
        self.row_index.clear();
        self.filter = StatusFilter::default();
        self.table_state = TableState::default();
        self.progress = (0, 0);
        activity.clone_into(&mut self.activity);
        self.sign_in = None;
        self.rate_limited = false;
        self.retry_after = None;
        self.card_mount = None;
        self.error = None;
        self.running = true;
        self.screen = Screen::Run;
    }

    /// Handle a key press. Ctrl+C quits from every screen.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return Action::None;
        }
        match self.screen {
            Screen::Home => screens::home::on_key(self, key),
            Screen::Run => screens::run::on_key(self, key),
        }
    }

    /// Whether the run ended early: an error, a failed sign-in or a Spotify rate limit.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.error.is_some()
            || self.rate_limited
            || matches!(self.sign_in, Some(SignInState::Failed(_)))
    }

    /// Apply a worker event.
    pub fn on_worker_event(&mut self, event: Event) {
        match event {
            Event::SignIn(state) => self.sign_in = Some(state),
            Event::Listed(tracks) => {
                for track in tracks {
                    let row = self.row_mut(&track.key());
                    row.title.clone_from(&track.title);
                    row.artist = track.artists_joined();
                    row.list.clone_from(&track.list_name);
                }
            }
            Event::Song { key, stage, status } => {
                let row = self.row_mut(&key);
                row.stage = stage;
                row.status = status;
            }
            Event::Progress { done, total } => self.progress = (done, total),
            Event::Activity(text) => self.activity = text,
            Event::RateLimited { wait } => {
                self.rate_limited = true;
                self.retry_after = wait;
            }
            Event::CardSynced { mount } => self.card_mount = Some(mount),
            Event::Error(message) => {
                self.activity.clear();
                self.error = Some(message);
            }
            Event::Finished => self.running = false,
        }
    }

    /// The row for `key`, added (titled by the key) when the key is new.
    fn row_mut(&mut self, key: &str) -> &mut SongRow {
        let next = self.rows.len();
        let index = *self.row_index.entry(key.to_owned()).or_insert(next);
        if index == next {
            self.rows.push(SongRow {
                key: key.to_owned(),
                title: key.to_owned(),
                artist: String::new(),
                list: String::new(),
                stage: Stage::Listed,
                status: Status::InProgress,
            });
        }
        &mut self.rows[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl_core::Track;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn app() -> Result<App, Box<dyn std::error::Error>> {
        let cfg = Config::parse(
            "client_id = \"a\"\ncard_uuid = \"u\"\nmusic_root = \"/m\"\n",
            Path::new("/c.toml"),
        )?;
        Ok(App::new(cfg))
    }

    fn track(id: &str, list: &str) -> Track {
        Track {
            title: format!("T{id}"),
            artists: vec!["A".into()],
            album: String::new(),
            album_artist: String::new(),
            track_number: 1,
            disc_number: 1,
            release_date: None,
            isrc: None,
            duration: Duration::from_secs(1),
            cover_url: None,
            spotify_id: id.into(),
            list_name: list.into(),
        }
    }

    fn song(key: &str, status: Status) -> Event {
        Event::Song {
            key: key.into(),
            stage: Stage::Matching,
            status,
        }
    }

    #[test]
    fn listed_adds_rows_and_song_updates_them() -> TestResult {
        let mut app = app()?;
        app.on_worker_event(Event::Listed(vec![track("1", "L"), track("2", "L")]));
        assert_eq!(app.rows.len(), 2);
        app.on_worker_event(song("L/2", Status::Done));
        assert_eq!(app.rows[1].status, Status::Done);
        assert_eq!(app.rows[1].title, "T2");
        assert_eq!(app.rows[0].status, Status::InProgress);
        Ok(())
    }

    #[test]
    fn unknown_song_key_adds_a_link_row() -> TestResult {
        let mut app = app()?;
        app.on_worker_event(song("Some title", Status::InProgress));
        app.on_worker_event(song("Some title", Status::Done));
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].title, "Some title");
        assert_eq!(app.rows[0].status, Status::Done);
        Ok(())
    }

    #[test]
    fn rate_limit_progress_and_finish() -> TestResult {
        let mut app = app()?;
        app.running = true;
        app.on_worker_event(Event::Progress { done: 3, total: 9 });
        app.on_worker_event(Event::RateLimited {
            wait: Some(Duration::from_secs(90)),
        });
        app.on_worker_event(Event::Finished);
        assert_eq!(app.progress, (3, 9));
        assert!(app.rate_limited);
        assert_eq!(app.retry_after, Some(Duration::from_secs(90)));
        assert!(!app.running);
        assert!(app.stopped());
        Ok(())
    }

    #[test]
    fn sign_in_activity_and_error_are_stored() -> TestResult {
        let mut app = app()?;
        app.on_worker_event(Event::SignIn(SignInState::SignedIn));
        app.on_worker_event(Event::Activity("listing".into()));
        assert_eq!(app.activity, "listing");
        assert!(!app.stopped());
        app.on_worker_event(Event::Error("boom".into()));
        assert_eq!(app.sign_in, Some(SignInState::SignedIn));
        assert_eq!(
            app.activity, "",
            "a stale activity line is cleared by the error"
        );
        assert_eq!(app.error.as_deref(), Some("boom"));
        assert!(app.stopped());
        Ok(())
    }

    #[test]
    fn failed_sign_in_stops_the_run() -> TestResult {
        let mut app = app()?;
        app.on_worker_event(Event::SignIn(SignInState::Failed("denied".into())));
        assert!(app.stopped());
        Ok(())
    }

    #[test]
    fn filter_selects_by_status() -> TestResult {
        let mut app = app()?;
        app.on_worker_event(song("a", Status::Done));
        app.on_worker_event(song("b", Status::Failed("x".into())));
        app.on_worker_event(song("c", Status::Unmatched));
        assert_eq!(app.visible_rows().len(), 3);
        app.filter = StatusFilter::Failed;
        assert_eq!(app.visible_rows().len(), 1);
        assert_eq!(app.visible_rows()[0].key, "b");
        app.filter = StatusFilter::Unmatched;
        assert_eq!(app.visible_rows()[0].key, "c");
        assert_eq!(StatusFilter::All.next().next().next(), StatusFilter::All);
        Ok(())
    }
}
