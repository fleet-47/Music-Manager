//! `~/.config/sdl/state.toml`: the ticked Spotify lists and each function's last-run line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::TaskId;
use crate::config::{ConfigError, config_dir};

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

/// The result of a function's last run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    /// The line shown under the function in the main panel.
    pub line: String,
    /// When the run ended, in seconds since the Unix epoch.
    pub at: u64,
}

/// Everything remembered between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Ids of the Spotify lists that were ticked ([`crate::event::LIKED_SONGS_ID`] or playlist ids).
    #[serde(default)]
    pub ticked_lists: Vec<String>,
    /// Last-run line and time per function.
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
            },
        );
        last_run.insert(
            TaskId::SyncCard,
            LastRun {
                line: "stopped".into(),
                at: 1_700_000_100,
            },
        );
        State {
            ticked_lists: vec!["liked-songs".into(), "37i9dQZF1DX".into()],
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
        assert_eq!(State::parse("", Path::new("/s.toml"))?, State::default());
        Ok(())
    }

    #[test]
    fn malformed_names_path() {
        let err = State::parse("ticked_lists = 3", Path::new("/z/state.toml"));
        let msg = err.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(msg.contains("/z/state.toml"), "{msg}");
    }
}
