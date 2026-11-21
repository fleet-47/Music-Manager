//! The review list (F8): `~/.config/sdl/review.toml`, every held song until it is resolved.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::playlist::Playlist;
use crate::storage::{self, StoreError};
use crate::{Candidate, LibraryId, WantedSong};

/// A song that has no confident match and waits for the owner.
///
/// Identified by its library and [`WantedSong::key`] (F8, F12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldSong {
    /// The library it was wanted for, and where it lands. Absent in files written before libraries,
    /// which means the owner's own.
    #[serde(default, skip_serializing_if = "LibraryId::is_own")]
    pub library: LibraryId,
    /// What was wanted.
    pub song: WantedSong,
    /// The names of the playlists that list it: the ticked Spotify lists, or a playlist link's
    /// name. Empty for a single link item (its URL is in `song.source`). Review shows these as
    /// where it came from, and corrects the song's line in each of these playlist files when it
    /// lands with another extension (F7).
    #[serde(default)]
    pub from: Vec<String>,
    /// The file names of those playlists (`<name>.m3u8`), as written by [`Playlist::file_name`]
    /// from the list's name and ID. Kept because a list whose name is empty once made safe is
    /// written under its ID, which the name alone cannot give back. Absent in files written
    /// before it was kept; [`HeldSong::playlist_files`] covers both.
    #[serde(default)]
    pub from_files: Vec<String>,
    /// The results found, up to 5, best-ranked first.
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    /// Why nothing passed, in plain words.
    pub reason: String,
}

impl HeldSong {
    /// The playlist files that list this song: the names kept when it was held, else (an older
    /// entry) the file names its list names would have.
    #[must_use]
    pub fn playlist_files(&self) -> Vec<String> {
        if self.from_files.is_empty() {
            self.from
                .iter()
                .map(|name| Playlist::file_name(name, ""))
                .collect()
        } else {
            self.from_files.clone()
        }
    }
}

/// The review list. A sync never searches again for a song that is in it (F8).
///
/// Invariant: at most one entry per library and [`WantedSong::key`]; order is the order songs were held.
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

    /// Whether a song with this key ([`WantedSong::key`]) is held for `library`.
    #[must_use]
    pub fn contains(&self, library: &LibraryId, key: &str) -> bool {
        self.held
            .iter()
            .any(|held| held.library == *library && held.song.key() == key)
    }

    /// Hold a song. A song with the same library and key is replaced in place (its position is
    /// kept).
    pub fn hold(&mut self, song: HeldSong) {
        match self
            .held
            .iter_mut()
            .find(|held| held.library == song.library && held.song.key() == song.song.key())
        {
            Some(existing) => *existing = song,
            None => self.held.push(song),
        }
    }

    /// Remove and return the song with this key held for `library`, when there is one.
    pub fn resolve(&mut self, library: &LibraryId, key: &str) -> Option<HeldSong> {
        let index = self
            .held
            .iter()
            .position(|held| held.library == *library && held.song.key() == key)?;
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
            library: LibraryId::Own,
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
            from_files: vec!["Liked Songs.m3u8".into()],
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
        let own = LibraryId::Own;
        assert!(list.contains(&own, "b"));
        assert_eq!(
            list.resolve(&own, "a").map(|h| h.reason),
            Some("three".to_owned())
        );
        assert!(!list.contains(&own, "a"));
        assert!(list.resolve(&own, "a").is_none());
    }

    #[test]
    fn the_same_song_held_for_two_libraries_is_two_entries() {
        let mut list = ReviewList::default();
        let mut sams = held("a", "sam's");
        sams.library = LibraryId::Other("Sam".into());
        list.hold(held("a", "mine"));
        list.hold(sams);
        assert_eq!(list.len(), 2);
        let sam = LibraryId::Other("Sam".into());
        assert!(list.contains(&sam, "a") && list.contains(&LibraryId::Own, "a"));
        assert_eq!(
            list.resolve(&sam, "a").map(|h| h.reason),
            Some("sam's".to_owned())
        );
        assert!(list.contains(&LibraryId::Own, "a") && !list.contains(&sam, "a"));
    }

    #[test]
    fn an_old_entry_has_no_library_and_its_files_come_from_its_names() -> TestResult {
        // A byte copy of ~/.config/sdl/review.toml as it was when libraries were added.
        let text = include_str!("../tests/fixtures/review-before-libraries.toml");
        let path = std::path::Path::new("/r.toml");
        let list: ReviewList = crate::storage::parse(text, path)?;
        let only = list.songs().first().ok_or("entry lost")?;
        assert_eq!(list.len(), 1);
        assert_eq!(only.library, LibraryId::Own);
        assert_eq!(only.song.key(), "f3_jWbM20AY");
        assert!(list.contains(&LibraryId::Own, "f3_jWbM20AY"));
        assert_eq!(only.playlist_files(), Vec::<String>::new());
        let mut named = only.clone();
        named.from = vec!["Road: trip".into()];
        assert_eq!(named.playlist_files(), ["Road trip.m3u8"]);
        // A list written under its ID keeps that file name.
        named.from = vec!["???".into()];
        named.from_files = vec!["37i9dQZF1DX.m3u8".into()];
        assert_eq!(named.playlist_files(), ["37i9dQZF1DX.m3u8"]);
        Ok(())
    }
}
