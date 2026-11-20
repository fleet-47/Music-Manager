//! The review list (F8): `~/.config/sdl/review.toml`, every held song until it is resolved.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::storage::{self, StoreError};
use crate::{Candidate, WantedSong};

/// A song that has no confident match and waits for the owner.
///
/// Identified by [`WantedSong::key`]: its Spotify track ID, or for a link item its item URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldSong {
    /// What was wanted.
    pub song: WantedSong,
    /// The names of the playlists that list it: the ticked Spotify lists, or a playlist link's
    /// name. Empty for a single link item (its URL is in `song.source`). Review shows these as
    /// where it came from, and corrects the song's line in each of these playlist files when it
    /// lands with another extension (F7).
    #[serde(default)]
    pub from: Vec<String>,
    /// The results found, up to 5, best-ranked first.
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    /// Why nothing passed, in plain words.
    pub reason: String,
}

/// The review list. A sync never searches again for a song that is in it (F8).
///
/// Invariant: at most one entry per [`WantedSong::key`]; order is the order songs were held.
/// Only the task layer's runner writes the file (F11); everyone else reads a copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewList {
    #[serde(default)]
    held: Vec<HeldSong>,
}

impl ReviewList {
    /// Path of the review file, `~/.config/sdl/review.toml`.
    ///
    /// # Errors
    /// [`StoreError::NoHome`] when the home directory is unknown.
    pub fn path() -> Result<PathBuf, StoreError> {
        Ok(config_dir()?.join("review.toml"))
    }

    /// Load from [`ReviewList::path`]. A missing file is the empty list.
    ///
    /// # Errors
    /// An unreadable or malformed file, naming the path.
    pub fn load() -> Result<Self, StoreError> {
        Self::load_from(&Self::path()?)
    }

    /// Load from `path`. A missing file is the empty list.
    ///
    /// # Errors
    /// An unreadable or malformed file, naming the path.
    pub fn load_from(path: &Path) -> Result<Self, StoreError> {
        storage::load_or_default(path)
    }

    /// Save to `path`, creating its directory if needed.
    ///
    /// # Errors
    /// The file could not be encoded or written.
    pub fn save_to(&self, path: &Path) -> Result<(), StoreError> {
        storage::save(self, path)
    }

    /// The held songs, in the order they were held.
    #[must_use]
    pub fn songs(&self) -> &[HeldSong] {
        &self.held
    }

    /// How many songs are held (the number on the Review row).
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Whether a song with this key ([`WantedSong::key`]) is held.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.held.iter().any(|held| held.song.key() == key)
    }

    /// Hold a song. A song with the same key is replaced in place (its position is kept).
    pub fn hold(&mut self, song: HeldSong) {
        match self
            .held
            .iter_mut()
            .find(|held| held.song.key() == song.song.key())
        {
            Some(existing) => *existing = song,
            None => self.held.push(song),
        }
    }

    /// Remove and return the song with this key, when it is held.
    pub fn resolve(&mut self, key: &str) -> Option<HeldSong> {
        let index = self.held.iter().position(|held| held.song.key() == key)?;
        Some(self.held.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::SongSource;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn held(key: &str, reason: &str) -> HeldSong {
        HeldSong {
            song: WantedSong {
                source: SongSource::Spotify {
                    track_id: key.into(),
                },
                title: "T".into(),
                artists: vec!["A".into()],
                album: Some("Al".into()),
                album_artist: None,
                track_number: Some(1),
                disc_number: None,
                date: Some("2020".into()),
                isrc: Some("ISRC".into()),
                duration: Some(Duration::from_secs(200)),
                cover_url: None,
            },
            from: vec!["Liked Songs".into()],
            candidates: vec![Candidate {
                video_id: "v1".into(),
                title: "T (live)".into(),
                artists: vec!["A".into()],
                album: None,
                duration: Some(Duration::from_secs(250)),
            }],
            reason: reason.into(),
        }
    }

    #[test]
    fn save_then_load_round_trips() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("nested/review.toml");
        let mut list = ReviewList::default();
        list.hold(held("a", "no result matched"));
        let mut link = held("b", "no results");
        link.song.source = SongSource::Link {
            item_url: "https://y/v".into(),
            video_id: "v".into(),
            direct: false,
        };
        link.candidates.clear();
        list.hold(link);
        list.save_to(&path)?;
        assert_eq!(ReviewList::load_from(&path)?, list);
        Ok(())
    }

    #[test]
    fn a_missing_file_is_the_empty_list() -> TestResult {
        let dir = tempfile::tempdir()?;
        assert!(ReviewList::load_from(&dir.path().join("none.toml"))?.is_empty());
        Ok(())
    }

    #[test]
    fn a_malformed_file_names_its_path() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("review.toml");
        std::fs::write(&path, "held = 3")?;
        let message = ReviewList::load_from(&path)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(message.contains("review.toml"), "{message}");
        Ok(())
    }

    #[test]
    fn hold_replaces_by_key_and_resolve_removes() {
        let mut list = ReviewList::default();
        list.hold(held("a", "one"));
        list.hold(held("b", "two"));
        list.hold(held("a", "three"));
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.songs().first().map(|h| h.reason.as_str()),
            Some("three")
        );
        assert!(list.contains("b"));
        assert_eq!(
            list.resolve("a").map(|h| h.reason),
            Some("three".to_owned())
        );
        assert!(!list.contains("a"));
        assert!(list.resolve("a").is_none());
    }
}
