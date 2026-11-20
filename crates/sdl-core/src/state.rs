//! `~/.config/sdl/state.toml`: everything remembered between runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use std::collections::BTreeSet;

use crate::config::config_dir;
use crate::storage::{self, StoreError};
use crate::{Attention, ConvertChoice, Counts, DeviceChoice, Format, TaskId};

/// How a run ended; it picks the colour of the last-run line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Finished with nothing to look at.
    Done,
    /// Finished, with held or failed items.
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
    /// The held and failed items of that run, for the attention list; still there after a
    /// restart.
    pub attention: Vec<Attention>,
}

/// A last-run entry as stored; `outcome` and `attention` are absent in files written before they
/// existed.
#[derive(Deserialize)]
struct RawLastRun {
    line: String,
    at: u64,
    outcome: Option<RunOutcome>,
    #[serde(default)]
    attention: Vec<Attention>,
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
            attention: raw.attention,
        }
    }
}

/// Everything remembered between runs. Every field has a default, and keys this version does not
/// know (the old `formats` table, a device's old keys) are ignored, so a file written by an older
/// version loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Ids of the Spotify lists that were ticked ([`crate::event::LIKED_SONGS_ID`] or playlist ids).
    #[serde(default)]
    pub ticked_lists: Vec<String>,
    /// The Convert choices.
    #[serde(default)]
    pub convert: ConvertChoice,
    /// What was chosen per device, keyed by [`crate::Device::id`].
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceChoice>,
    /// The [`crate::Device::id`] of the device last synced; preselected next time.
    #[serde(default)]
    pub last_device: Option<String>,
    /// Songs marked "Never download", by [`crate::WantedSong::key`]. A sync never searches for
    /// them again.
    #[serde(default)]
    pub never: BTreeSet<String>,
    /// Last-run line, time, outcome and attention items per function.
    #[serde(default)]
    pub last_run: BTreeMap<TaskId, LastRun>,
}

impl State {
    /// Path of the state file, `~/.config/sdl/state.toml`.
    ///
    /// # Errors
    /// [`StoreError::NoHome`] when the home directory is unknown.
    pub fn path() -> Result<PathBuf, StoreError> {
        Ok(config_dir()?.join("state.toml"))
    }

    /// Load the state from [`State::path`]. A missing file is the empty state (first run).
    ///
    /// # Errors
    /// An unreadable or malformed file, naming the path.
    pub fn load() -> Result<Self, StoreError> {
        Self::load_from(&Self::path()?)
    }

    /// Load the state from `path`. A missing file is the empty state.
    ///
    /// # Errors
    /// An unreadable or malformed file, naming the path.
    pub fn load_from(path: &Path) -> Result<Self, StoreError> {
        let mut state: Self = storage::load_or_default(path)?;
        state.normalise();
        Ok(state)
    }

    /// Parse state text; `path` only names the file in errors.
    ///
    /// # Errors
    /// Invalid TOML or wrong value types.
    pub fn parse(text: &str, path: &Path) -> Result<Self, StoreError> {
        let mut state: Self = storage::parse(text, path)?;
        state.normalise();
        Ok(state)
    }

    /// Save to `path`, creating its directory if needed.
    ///
    /// # Errors
    /// The file could not be encoded or written.
    pub fn save_to(&self, path: &Path) -> Result<(), StoreError> {
        storage::save(self, path)
    }

    /// Old files may remember Opus as the Convert format; the Format row now offers MP3 and M4A
    /// only, so Opus reads as the default MP3.
    fn normalise(&mut self) {
        if !Format::CONVERT.contains(&self.convert.format) {
            self.convert.format = ConvertChoice::default().format;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Quality, Selection};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sample() -> State {
        let mut last_run = BTreeMap::new();
        last_run.insert(
            TaskId::SyncSpotify,
            LastRun {
                line: "12 added, 3 skipped".into(),
                at: 1_700_000_000,
                outcome: RunOutcome::NeedsAttention,
                attention: vec![
                    Attention::held("T".into(), "A".into(), "no result matched".into()),
                    Attention::failed("U".into(), String::new(), "network".into()),
                ],
            },
        );
        last_run.insert(
            TaskId::SyncDevice,
            LastRun {
                line: "stopped".into(),
                at: 1_700_000_100,
                outcome: RunOutcome::Stopped,
                attention: Vec::new(),
            },
        );
        let mut devices = BTreeMap::new();
        devices.insert(
            "AAAA-BBBB".to_owned(),
            DeviceChoice {
                direction: crate::Direction::FromDevice,
                what: Selection {
                    whole_library: true,
                    playlists: vec!["Road trip".into()],
                },
                from_folders: vec!["Music".into()],
                format: Some(Format::Mp3),
                quality: Quality::Small,
                keep_cover: false,
            },
        );
        State {
            ticked_lists: vec!["liked-songs".into(), "37i9dQZF1DX".into()],
            devices,
            last_device: Some("AAAA-BBBB".into()),
            never: BTreeSet::from(["track1".to_owned()]),
            convert: ConvertChoice {
                what: Selection::default(),
                format: Format::M4a,
                quality: Quality::Medium,
                keep_cover: true,
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
        assert!(state.convert.keep_cover);
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
    fn finished_runs_need_attention_only_with_held_or_failed() {
        let counts = Counts {
            held: 1,
            ..Counts::default()
        };
        assert_eq!(RunOutcome::of_finished(&counts), RunOutcome::NeedsAttention);
        assert_eq!(
            RunOutcome::of_finished(&Counts::default()),
            RunOutcome::Done
        );
    }

    #[test]
    fn the_owners_real_old_shape_state_file_still_loads() -> TestResult {
        // A byte copy of ~/.config/sdl/state.toml as it was before this redesign.
        let text = include_str!("../tests/fixtures/state-old-shape.toml");
        let state = State::parse(text, Path::new("/s.toml"))?;
        assert_eq!(state.ticked_lists, Vec::<String>::new());
        assert_eq!(state.convert.format, Format::Mp3);
        assert_eq!(
            state.convert.destination,
            PathBuf::from("~/Music-converted")
        );
        let device = state
            .devices
            .get("0481966B-99D3-369A-B6B0-EE83F5D07E45")
            .ok_or("device choice lost")?;
        assert_eq!(device.direction, crate::Direction::ToDevice);
        assert!(
            device.from_folders.is_empty(),
            "the old library folders are not read as device folders"
        );
        assert_eq!(device.format, None);
        assert!(device.keep_cover);
        let outcome = |task| state.last_run.get(&task).map(|run| run.outcome);
        assert_eq!(outcome(TaskId::DownloadLink), Some(RunOutcome::Done));
        assert_eq!(outcome(TaskId::SyncDevice), Some(RunOutcome::Done));
        assert!(state.never.is_empty() && state.last_device.is_none());
        Ok(())
    }

    #[test]
    fn an_old_opus_convert_format_reads_as_mp3() -> TestResult {
        let state = State::parse("[convert]\nformat = \"opus\"\n", Path::new("/s.toml"))?;
        assert_eq!(state.convert.format, Format::Mp3);
        Ok(())
    }

    #[test]
    fn malformed_names_path() {
        let err = State::parse("ticked_lists = 3", Path::new("/z/state.toml"));
        let msg = err.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(msg.contains("/z/state.toml"), "{msg}");
    }
}
