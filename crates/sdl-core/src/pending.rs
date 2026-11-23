//! The enrichment queue (F6): `~/.config/sdl/pending_enrichment.toml`, every landed song that
//! still waits for its genre and totals from `MusicBrainz`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::storage::{self, StoreError};
use crate::{LibraryId, WantedSong};

/// A song in a library that waits for enrichment.
///
/// Identified by its library and [`WantedSong::key`], as a held song is (F8, F12). Where its file
/// is comes from these two ([`crate::Place::of`]); no path is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSong {
    /// The library the song landed in. Absent in the file means the owner's own.
    #[serde(default, skip_serializing_if = "LibraryId::is_own")]
    pub library: LibraryId,
    /// The song as it was wanted: its ISRC is what is looked up, its album and disc number pick
    /// the release.
    pub song: WantedSong,
}

/// Invariant: at most one entry per library and [`WantedSong::key`]; order is the order songs
/// landed. Only the task layer's runner writes the file (F11).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEnrichment {
    #[serde(default)]
    pending: Vec<PendingSong>,
}

impl PendingEnrichment {
    /// Path of the queue file, `~/.config/sdl/pending_enrichment.toml`.
    ///
    /// # Errors
    /// [`StoreError::NoHome`] when the home directory is unknown.
    pub fn path() -> Result<PathBuf, StoreError> {
        Ok(config_dir()?.join("pending_enrichment.toml"))
    }

    /// Load from `path`. A missing file is the empty queue.
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

    /// How many songs wait.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether nothing waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The song that has waited longest.
    #[must_use]
    pub fn next(&self) -> Option<&PendingSong> {
        self.pending.first()
    }

    /// Queue a song at the end. A song already waiting for the same library is not queued twice.
    pub fn add(&mut self, song: PendingSong) {
        if self.position(&song.library, song.song.key()).is_none() {
            self.pending.push(song);
        }
    }

    /// Take the song with this key out of `library`'s queue; `false` when it was not waiting.
    pub fn remove(&mut self, library: &LibraryId, key: &str) -> bool {
        match self.position(library, key) {
            Some(index) => {
                self.pending.remove(index);
                true
            }
            None => false,
        }
    }

    fn position(&self, library: &LibraryId, key: &str) -> Option<usize> {
        self.pending
            .iter()
            .position(|waiting| waiting.library == *library && waiting.song.key() == key)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::SongSource;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn waiting(library: LibraryId, id: &str) -> PendingSong {
        PendingSong {
            library,
            song: WantedSong {
                source: SongSource::Spotify {
                    track_id: id.into(),
                },
                title: format!("Song {id}"),
                artists: vec!["Band".into()],
                album: Some("Record".into()),
                album_artist: None,
                track_number: Some(2),
                disc_number: Some(1),
                date: None,
                isrc: Some("USSM15900001".into()),
                duration: Some(Duration::from_secs(200)),
                cover_url: None,
            },
        }
    }

    #[test]
    fn a_song_is_queued_once_per_library_and_served_oldest_first() {
        let mut queue = PendingEnrichment::default();
        queue.add(waiting(LibraryId::Own, "a"));
        queue.add(waiting(LibraryId::Own, "b"));
        queue.add(waiting(LibraryId::Own, "a"));
        queue.add(waiting(LibraryId::Other("Sam".into()), "a"));
        assert_eq!(
            queue.len(),
            3,
            "the same song for a friend is its own entry"
        );
        assert_eq!(queue.next().map(|next| next.song.key()), Some("a"));
        assert!(queue.remove(&LibraryId::Own, "a"));
        assert!(!queue.remove(&LibraryId::Own, "a"), "already taken out");
        assert_eq!(queue.next().map(|next| next.song.key()), Some("b"));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn the_queue_survives_a_save_and_a_load_and_a_missing_file_is_empty() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("pending_enrichment.toml");
        assert!(PendingEnrichment::load_from(&path)?.is_empty());
        let mut queue = PendingEnrichment::default();
        queue.add(waiting(LibraryId::Own, "a"));
        queue.add(waiting(LibraryId::Other("Sam".into()), "b"));
        queue.save_to(&path)?;
        assert_eq!(PendingEnrichment::load_from(&path)?, queue);
        Ok(())
    }
}
