//! `~/.config/sdl/state.toml`: everything remembered between runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, config_dir};
use crate::{ConvertChoice, Counts, DeviceChoice, Format, TaskId};

/// Errors loading or saving the state file. Every variant names the path.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// No home directory could be determined.
    #[error("cannot determine the home directory to locate the state file")]
    NoHome,
    /// The file exists but cannot be read.
    #[error("cannot read state file {}", path.display())]
    Read {
        /// The state path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The file is not valid state TOML.
    #[error("malformed state file {}", path.display())]
    Parse {
        /// The state path.
        path: PathBuf,
        /// Underlying error.
        source: toml::de::Error,
    },
    /// The state could not be written.
    #[error("cannot write state file {}", path.display())]
    Write {
        /// The state path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The state could not be turned into TOML.
    #[error("cannot encode state for {}", path.display())]
    Encode {
        /// The state path.
        path: PathBuf,
        /// Underlying error.
        source: toml::ser::Error,
    },
}

impl From<ConfigError> for StateError {
    fn from(_: ConfigError) -> Self {
        Self::NoHome
    }
}

/// How a run ended; it picks the colour of the last-run line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Finished with nothing to look at.
    Done,
    /// Finished, with unmatched or failed items.
    NeedsAttention,
    /// The task failed.
    Failed,
    /// The owner (or a rate limit) stopped it.
    Stopped,
}

impl RunOutcome {
    /// How a run that went to the end ended: `NeedsAttention` when `counts` has any item that does,
    /// otherwise `Done`.
    #[must_use]
    pub fn of_finished(counts: &Counts) -> Self {
        if counts.need_attention() > 0 {
            Self::NeedsAttention
        } else {
            Self::Done
        }
    }

    /// The outcome of a line written before outcomes were saved, read from its wording.
    fn of_old_line(line: &str) -> Self {
        if line.starts_with("failed") {
            Self::Failed
        } else if line.starts_with("stopped") {
            Self::Stopped
        } else if line.contains("need attention") {
            Self::NeedsAttention
        } else {
            Self::Done
        }
    }
}

/// The result of a function's last run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawLastRun")]
pub struct LastRun {
    /// The line shown under the function in the main panel.
    pub line: String,
    /// When the run ended, in seconds since the Unix epoch.
    pub at: u64,
    /// How it ended.
    pub outcome: RunOutcome,
}

/// A last-run entry as stored; `outcome` is absent in files written before it existed.
#[derive(Deserialize)]
struct RawLastRun {
    line: String,
    at: u64,
    outcome: Option<RunOutcome>,
}

impl From<RawLastRun> for LastRun {
    fn from(raw: RawLastRun) -> Self {
        let outcome = raw
            .outcome
            .unwrap_or_else(|| RunOutcome::of_old_line(&raw.line));
        Self {
            line: raw.line,
            at: raw.at,
            outcome,
        }
    }
}

/// Everything remembered between runs. Every field has a default, so a file written by an older
/// version loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Ids of the Spotify lists that were ticked ([`crate::event::LIKED_SONGS_ID`] or playlist ids).
    #[serde(default)]
    pub ticked_lists: Vec<String>,
    /// The format the owner asked to remember, per function (Sync Spotify, Download link). A
    /// function with no entry starts on [`Format::Opus`] ("Original").
    #[serde(default)]
    pub formats: BTreeMap<TaskId, Format>,
    /// Folders, direction and format per device, keyed by [`crate::Device::id`].
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceChoice>,
    /// The Convert choices.
    #[serde(default)]
    pub convert: ConvertChoice,
    /// Last-run line, time and outcome per function.
    #[serde(default)]
    pub last_run: BTreeMap<TaskId, LastRun>,
}

impl State {
    /// Path of the state file, `~/.config/sdl/state.toml`.
    ///
    /// # Errors
    /// [`StateError::NoHome`] when the home directory is unknown.
    pub fn path() -> Result<PathBuf, StateError> {
        Ok(config_dir()?.join("state.toml"))
    }

    /// Load the state from [`State::path`]. A missing file is the empty state (first run).
    ///
    /// # Errors
    /// An unreadable or malformed file, naming the path.
    pub fn load() -> Result<Self, StateError> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, &path),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(StateError::Read { path, source }),
        }
    }

    /// Parse state text; `path` only names the file in errors.
    ///
    /// # Errors
    /// Invalid TOML or wrong value types.
    pub fn parse(text: &str, path: &Path) -> Result<Self, StateError> {
        toml::from_str(text).map_err(|source| StateError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Save to `path`, creating its directory if needed.
    ///
    /// # Errors
    /// The file could not be encoded or written.
    pub fn save_to(&self, path: &Path) -> Result<(), StateError> {
        let text = toml::to_string_pretty(self).map_err(|source| StateError::Encode {
            path: path.to_path_buf(),
            source,
        })?;
        let write = |source| StateError::Write {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(write)?;
        }
        std::fs::write(path, text).map_err(write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample() -> State {
        let mut last_run = BTreeMap::new();
        last_run.insert(
            TaskId::SyncSpotify,
            LastRun {
                line: "12 added, 3 skipped".into(),
                at: 1_700_000_000,
                outcome: RunOutcome::Done,
            },
        );
        last_run.insert(
            TaskId::SyncDevice,
            LastRun {
                line: "stopped".into(),
                at: 1_700_000_100,
                outcome: RunOutcome::Stopped,
            },
        );
        let mut formats = BTreeMap::new();
        formats.insert(TaskId::DownloadLink, Format::M4a);
        let mut devices = BTreeMap::new();
        devices.insert(
            "AAAA-BBBB".to_owned(),
            DeviceChoice {
                folders: vec!["Spotify".into()],
                direction: crate::Direction::FromDevice,
                format: Some(Format::Mp3),
            },
        );
        State {
            ticked_lists: vec!["liked-songs".into(), "37i9dQZF1DX".into()],
            formats,
            devices,
            convert: ConvertChoice {
                folders: vec!["Youtube".into()],
                format: Format::Opus,
                destination: "~/Out".into(),
            },
            last_run,
        }
    }

    #[test]
    fn save_then_load_round_trips() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("nested/state.toml");
        let state = sample();
        state.save_to(&path)?;
        let text = std::fs::read_to_string(&path)?;
        assert_eq!(State::parse(&text, &path)?, state);
        Ok(())
    }

    #[test]
    fn empty_file_is_the_empty_state() -> TestResult {
        let state = State::parse("", Path::new("/s.toml"))?;
        assert_eq!(state, State::default());
        assert_eq!(state.convert.format, Format::Mp3);
        Ok(())
    }

    #[test]
    fn an_old_state_file_loads_with_outcomes_read_from_the_lines() -> TestResult {
        let old = r#"
ticked_lists = ["liked-songs"]

[last_run.sync_spotify]
line = "12 added, 3 skipped, 2 need attention"
at = 5

[last_run.download_link]
line = "failed: yt-dlp was not found on PATH"
at = 6

[last_run.sync_card]
line = "stopped"
at = 7
"#;
        let state = State::parse(old, Path::new("/s.toml"))?;
        assert_eq!(state.ticked_lists, ["liked-songs"]);
        let outcome = |task| state.last_run.get(&task).map(|run| run.outcome);
        assert_eq!(
            outcome(TaskId::SyncSpotify),
            Some(RunOutcome::NeedsAttention)
        );
        assert_eq!(outcome(TaskId::DownloadLink), Some(RunOutcome::Failed));
        assert_eq!(outcome(TaskId::SyncDevice), Some(RunOutcome::Stopped));
        Ok(())
    }

    #[test]
    fn finished_runs_need_attention_only_with_unmatched_or_failed() {
        let counts = Counts {
            unmatched: 1,
            ..Counts::default()
        };
        assert_eq!(RunOutcome::of_finished(&counts), RunOutcome::NeedsAttention);
        assert_eq!(
            RunOutcome::of_finished(&Counts::default()),
            RunOutcome::Done
        );
    }

    #[test]
    fn malformed_names_path() {
        let err = State::parse("ticked_lists = 3", Path::new("/z/state.toml"));
        let msg = err.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(msg.contains("/z/state.toml"), "{msg}");
    }
}
