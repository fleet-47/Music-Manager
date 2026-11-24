//! The enrichment queue (F6): `~/.config/sdl/pending_enrichment.toml`, every landed song that
//! still waits for its genre and totals from `MusicBrainz`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::storage::{self, StoreError};
use crate::{LibraryId, WantedSong};

/// A song in a library whose file has no genre yet.
///
/// Identified by its library and [`WantedSong::key`], as a held song is (F8, F12). Where its file
/// is comes from these two ([`crate::Place::of`]); no path is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSong {
    /// The library the song landed in. Absent in the file means the owner's own.
    #[serde(default, skip_serializing_if = "LibraryId::is_own")]
    pub library: LibraryId,
    /// `MusicBrainz` was asked and gave no genre (or the song has no ISRC to ask with): the
    /// worker leaves it alone until it is asked again ([`PendingEnrichment::ask_again`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asked: bool,
    /// The song as it was wanted: its ISRC is what is looked up, its album and disc number pick
    /// the release.
    pub song: WantedSong,
}

/// How many songs are missing a genre, by where they stand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnrichmentCounts {
    /// Not asked yet: the worker will take them.
    pub waiting: usize,
    /// Asked, and no genre came: they wait for the owner to ask again.
    pub asked: usize,
}

/// The songs missing a genre (F6): a song stays until its file has one. Invariant: at most one
/// entry per library and [`WantedSong::key`]; order is the order songs landed. Only the task
/// layer's runner writes the file (F11).
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

    /// How many songs are in the queue, waiting or asked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// How many songs are waiting and how many were asked.
    #[must_use]
    pub fn counts(&self) -> EnrichmentCounts {
        let asked = self.pending.iter().filter(|song| song.asked).count();
        EnrichmentCounts {
            waiting: self.pending.len().saturating_sub(asked),
            asked,
        }
    }

    /// The waiting song that has been in the queue longest; asked songs are passed over.
    #[must_use]
    pub fn next(&self) -> Option<&PendingSong> {
        self.pending.iter().find(|song| !song.asked)
    }

    /// Queue a song at the end. A song already in the queue for the same library is not queued
    /// twice and keeps where it stands.
    pub fn add(&mut self, song: PendingSong) {
        if self.position(&song.library, song.song.key()).is_none() {
            self.pending.push(song);
        }
    }

    /// Mark the song with this key in `library`'s queue as asked; `false` when it is not there.
    pub fn mark_asked(&mut self, library: &LibraryId, key: &str) -> bool {
        match self.position(library, key) {
            Some(index) => {
                if let Some(song) = self.pending.get_mut(index) {
                    song.asked = true;
                }
                true
            }
            None => false,
        }
    }

    /// Make every asked song wait again, so the worker asks for each once more. Returns how many
    /// that was.
    pub fn ask_again(&mut self) -> usize {
        let mut again = 0_usize;
        for song in self.pending.iter_mut().filter(|song| song.asked) {
            song.asked = false;
            again = again.saturating_add(1);
        }
        again
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
            asked: false,
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
        assert!(queue.mark_asked(&LibraryId::Own, "a"));
        queue.save_to(&path)?;
        let loaded = PendingEnrichment::load_from(&path)?;
        assert_eq!(loaded, queue);
        assert_eq!(loaded.counts().asked, 1, "asked is remembered");
        Ok(())
    }

    #[test]
    fn an_asked_song_stays_but_is_passed_over_until_it_is_asked_again() {
        let mut queue = PendingEnrichment::default();
        queue.add(waiting(LibraryId::Own, "a"));
        queue.add(waiting(LibraryId::Own, "b"));
        assert!(queue.mark_asked(&LibraryId::Own, "a"));
        assert!(!queue.mark_asked(&LibraryId::Own, "gone"));
        let counts = queue.counts();
        assert_eq!((counts.waiting, counts.asked), (1, 1));
        assert_eq!(queue.next().map(|next| next.song.key()), Some("b"));
        // Landing it again does not make it wait again.
        queue.add(waiting(LibraryId::Own, "a"));
        assert_eq!(queue.counts().asked, 1);
        assert!(queue.mark_asked(&LibraryId::Own, "b"));
        assert_eq!(queue.next(), None, "nothing is left for the worker");
        assert_eq!(queue.len(), 2, "both are still missing a genre");
        assert_eq!(queue.ask_again(), 2);
        assert_eq!(queue.next().map(|next| next.song.key()), Some("a"));
        assert_eq!(queue.ask_again(), 0);
    }
}
