//! Enrichment (F6): the queue of songs in the library that are missing a genre, and the
//! background worker that asks `MusicBrainz` for genre and totals and adds them to the files.
//!
//! Interface, in full:
//! * [`Queue`] is the only writer of `pending_enrichment.toml` (F11). A download adds every song
//!   it lands through `TaskContext::queue_enrichment`; nobody else adds. A song stays until its
//!   file has a genre, so the queue is the list of songs missing one: *waiting* (the worker will
//!   take it) or *asked* (`MusicBrainz` gave no genre; left alone).
//! * [`Queue::ask_again`] makes every asked song wait again and wakes the worker, and
//!   [`Queue::counts`] says how many stand where. The runner hands both out
//!   (`Runner::ask_enrichment_again`, `Runner::enrichment_counts`) as the connection point for an
//!   enrichment command; nothing in the app calls them yet.
//! * [`work`] is the worker: one waiting song at a time, oldest first, at the pace of the
//!   `MusicBrainz` client's own limiter. It sleeps while nothing waits and wakes when a song is
//!   added or asked again. It never ends by itself: the runner aborts it when the app closes,
//!   which is safe at any point, because a file is replaced whole or not at all
//!   (`sdl_library::tags::add_enrichment`) and it starts no outside tool.
//! * One song's turn ([`Turn`]): its file is gone, or has a genre already: it leaves the queue,
//!   with no request. `MusicBrainz` gave a genre and it was written: it leaves. `MusicBrainz`
//!   answered without a genre (or the song has no ISRC): what it gave is written and the song
//!   stays, asked. The request or the write failed: it stays waiting and is tried again after
//!   [`RETRY`].
//!
//! The worker is not one of the five functions: it has no card, sends no events and is never
//! stopped by the owner.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sdl_core::{
    Config, EnrichmentCounts, Library, LibraryId, PendingEnrichment, PendingSong, Place,
    StoreError, WantedSong,
};
use sdl_library::LibraryError;
use sdl_library::tags::{Enrichment, ReleaseCache, add_enrichment, has_genre, lookup};
use tokio::sync::Notify;

/// How long the worker waits before it tries again a song whose request or write failed.
const RETRY: Duration = Duration::from_secs(60);

/// The enrichment queue, where it is saved, and the worker's wake-up. Cheap to clone; every copy
/// is the same queue.
#[derive(Debug, Clone)]
pub(crate) struct Queue {
    songs: Arc<Mutex<PendingEnrichment>>,
    path: Arc<PathBuf>,
    wake: Arc<Notify>,
}

impl Queue {
    /// The queue as loaded, saved to `path` from now on.
    pub(crate) fn new(songs: PendingEnrichment, path: PathBuf) -> Self {
        Self {
            songs: Arc::new(Mutex::new(songs)),
            path: Arc::new(path),
            wake: Arc::default(),
        }
    }

    /// Change the queue and save it, holding the lock so saves cannot land out of order. The
    /// change stays in memory even when the save fails.
    fn update<T>(&self, change: impl FnOnce(&mut PendingEnrichment) -> T) -> Result<T, StoreError> {
        let mut songs = self.songs.lock().unwrap_or_else(PoisonError::into_inner);
        let changed = change(&mut songs);
        songs.save_to(&self.path).map(|()| changed)
    }

    /// Queue a landed song as waiting, save, and wake the worker.
    ///
    /// # Errors
    /// The queue file could not be written; the song is queued in memory all the same.
    pub(crate) fn add(&self, library: LibraryId, song: WantedSong) -> Result<(), StoreError> {
        let saved = self.update(|songs| {
            songs.add(PendingSong {
                library,
                asked: false,
                song,
            });
        });
        self.wake.notify_one();
        saved
    }

    /// Make every asked song wait again, save, and wake the worker: the connection point for an
    /// enrichment command. Returns how many songs will be asked for again.
    ///
    /// # Errors
    /// The queue file could not be written; the songs wait again in memory all the same.
    pub(crate) fn ask_again(&self) -> Result<usize, StoreError> {
        let again = self.update(PendingEnrichment::ask_again);
        self.wake.notify_one();
        again
    }

    /// How many songs are waiting and how many were asked.
    pub(crate) fn counts(&self) -> EnrichmentCounts {
        self.songs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .counts()
    }

    /// The waiting song that has been in the queue longest.
    fn next(&self) -> Option<PendingSong> {
        self.songs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next()
            .cloned()
    }

    /// Record how `song`'s turn ended. A failed save is left at that: the change holds in memory
    /// for as long as the app runs, and the file then still lists the song as waiting, so the
    /// next start of the app takes its turn again.
    fn settle(&self, song: &PendingSong, turn: Turn) {
        let key = song.song.key();
        let _saved = match turn {
            Turn::Leaves => self.update(|songs| songs.remove(&song.library, key)),
            Turn::Asked => self.update(|songs| songs.mark_asked(&song.library, key)),
            Turn::Later => return,
        };
    }
}

/// How one song's turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// It leaves the queue: its file has a genre, or is no longer in the library.
    Leaves,
    /// It stays, marked as asked: `MusicBrainz` answered without a genre.
    Asked,
    /// It stays waiting; try again after [`RETRY`].
    Later,
}

/// The worker: enrich the queue's waiting songs one at a time for as long as the app runs.
pub(crate) async fn work(queue: Queue, config: Arc<Config>) {
    // One release lookup per release while the app runs, across every run's songs.
    let cache = ReleaseCache::default();
    loop {
        let Some(next) = queue.next() else {
            // `add` and `ask_again` leave a wake-up behind when nobody waits yet, so none is
            // missed.
            queue.wake.notified().await;
            continue;
        };
        let turn = match missing_genre(&config, &next).await {
            Ok(Some(file)) => {
                let found = lookup(&next.song, &cache).await;
                write_found(file, found).await
            }
            Ok(None) => Turn::Leaves,
            Err(()) => Turn::Later,
        };
        queue.settle(&next, turn);
        if turn == Turn::Later {
            tokio::time::sleep(RETRY).await;
        }
    }
}

/// The file of `next`'s song when it is in the library and has no genre; `None` when there is
/// nothing to do for it (the file is gone, or has a genre already). `Err` when that could not be
/// told (the folder or the file cannot be read).
async fn missing_genre(config: &Config, next: &PendingSong) -> Result<Option<PathBuf>, ()> {
    let place = Place::of(&Library::resolve(config, &next.library), &next.song);
    let looked = tokio::task::spawn_blocking(move || {
        let Some(format) = place.present().map_err(|_| ())? else {
            return Ok(None);
        };
        let file = place.path(format);
        match has_genre(&file) {
            Ok(true) => Ok(None),
            Ok(false) => Ok(Some(file)),
            Err(_) => Err(()),
        }
    });
    looked.await.map_err(|_| ())?
}

/// Add what `MusicBrainz` answered to `file`, and say how the song's turn ended.
async fn write_found(file: PathBuf, found: Result<Enrichment, LibraryError>) -> Turn {
    let Ok(found) = found else {
        return Turn::Later;
    };
    let has_genre = found.genre.is_some();
    let written = tokio::task::spawn_blocking(move || add_enrichment(&file, &found)).await;
    match written {
        Ok(Ok(())) if has_genre => Turn::Leaves,
        Ok(Ok(())) => Turn::Asked,
        Ok(Err(_)) | Err(_) => Turn::Later,
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::Format;

    use super::*;
    use crate::tasks::intake::tests::song;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn config(dir: &std::path::Path) -> Config {
        Config {
            client_id: "id".into(),
            music_root: dir.join("music"),
            my_library: "Me".into(),
            download_jobs: std::num::NonZeroUsize::MIN,
            animations: true,
        }
    }

    fn waiting(id: &str) -> PendingSong {
        PendingSong {
            library: LibraryId::Own,
            asked: false,
            song: song(id, &format!("Song {id}")),
        }
    }

    fn jazz() -> Enrichment {
        Enrichment {
            genre: Some("Jazz".into()),
            ..Enrichment::default()
        }
    }

    /// Put `bytes` where `next`'s song lives in the library; returns the file.
    fn put(config: &Config, next: &PendingSong, bytes: &[u8]) -> Result<PathBuf, std::io::Error> {
        let place = Place::of(&Library::resolve(config, &next.library), &next.song);
        std::fs::create_dir_all(place.dir())?;
        let file = place.path(Format::Opus);
        std::fs::write(&file, bytes)?;
        Ok(file)
    }

    fn queue(dir: &std::path::Path) -> (Queue, PathBuf) {
        let path = dir.join("pending_enrichment.toml");
        (Queue::new(PendingEnrichment::default(), path.clone()), path)
    }

    #[tokio::test]
    async fn a_queued_song_is_saved_at_once_and_wakes_the_worker() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (queue, path) = queue(dir.path());
        assert_eq!(queue.next(), None);
        queue.add(LibraryId::Own, song("a", "First"))?;
        queue.add(LibraryId::Own, song("b", "Second"))?;
        queue.add(LibraryId::Own, song("a", "First"))?;
        assert_eq!(queue.counts().waiting, 2, "a song is queued once");
        assert_eq!(PendingEnrichment::load_from(&path)?.len(), 2);
        // The wake-up left by `add` is there for a worker that starts waiting only now.
        tokio::time::timeout(Duration::from_secs(5), queue.wake.notified()).await?;
        assert_eq!(
            queue.next().map(|next| next.song.key().to_owned()),
            Some("a".into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_turn_ends_with_the_song_gone_asked_or_still_waiting() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (queue, path) = queue(dir.path());
        for id in ["a", "b", "c"] {
            queue.add(LibraryId::Own, song(id, id))?;
        }
        queue.settle(&waiting("a"), Turn::Leaves);
        queue.settle(&waiting("b"), Turn::Asked);
        queue.settle(&waiting("c"), Turn::Later);
        let counts = queue.counts();
        assert_eq!((counts.waiting, counts.asked), (1, 1));
        assert_eq!(
            queue.next().map(|next| next.song.key().to_owned()),
            Some("c".into())
        );
        // What is on disk is what a restart finds.
        let saved = PendingEnrichment::load_from(&path)?;
        assert_eq!((saved.counts().waiting, saved.counts().asked), (1, 1));
        Ok(())
    }

    #[tokio::test]
    async fn asking_again_makes_asked_songs_wait_and_wakes_the_worker() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (queue, path) = queue(dir.path());
        queue.add(LibraryId::Own, song("a", "First"))?;
        queue.settle(&waiting("a"), Turn::Asked);
        assert_eq!(queue.next(), None, "an asked song is left alone");
        // Use up the wake-up `add` left, so the next one can only come from `ask_again`.
        tokio::time::timeout(Duration::from_secs(5), queue.wake.notified()).await?;
        assert_eq!(queue.ask_again()?, 1);
        tokio::time::timeout(Duration::from_secs(5), queue.wake.notified()).await?;
        assert_eq!(
            queue.next().map(|next| next.song.key().to_owned()),
            Some("a".into())
        );
        assert_eq!(PendingEnrichment::load_from(&path)?.counts().waiting, 1);
        assert_eq!(queue.ask_again()?, 0, "nothing is asked: nothing changes");
        Ok(())
    }

    #[tokio::test]
    async fn a_song_no_longer_in_the_library_has_nothing_to_do() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        assert_eq!(missing_genre(&config, &waiting("a")).await, Ok(None));
        Ok(())
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_read_is_tried_again_later() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        let next = waiting("a");
        let file = put(&config, &next, b"not audio at all")?;
        assert_eq!(missing_genre(&config, &next).await, Err(()));
        assert_eq!(write_found(file.clone(), Ok(jazz())).await, Turn::Later);
        assert_eq!(std::fs::read(&file)?, b"not audio at all");
        Ok(())
    }

    /// One second of real Opus audio where `next`'s song lives (needs `ffmpeg`, as the gate has).
    fn put_audio(
        config: &Config,
        next: &PendingSong,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let file = put(config, next, b"")?;
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1"])
            .arg(&file)
            .output()?;
        if !made.status.success() {
            return Err(String::from_utf8_lossy(&made.stderr).into_owned().into());
        }
        Ok(file)
    }

    #[tokio::test]
    async fn a_song_leaves_the_queue_only_once_its_file_has_a_genre() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        let next = waiting("a");
        let file = put_audio(&config, &next)?;
        // In the library with no genre: this is the file to enrich.
        assert_eq!(missing_genre(&config, &next).await, Ok(Some(file.clone())));
        // Totals but no genre: written, and the song stays, asked.
        let totals = Enrichment {
            track_total: Some(9),
            ..Enrichment::default()
        };
        assert_eq!(write_found(file.clone(), Ok(totals)).await, Turn::Asked);
        assert_eq!(missing_genre(&config, &next).await, Ok(Some(file.clone())));
        // A genre: written, and the song leaves; a later turn finds nothing to do.
        assert_eq!(write_found(file.clone(), Ok(jazz())).await, Turn::Leaves);
        assert_eq!(missing_genre(&config, &next).await, Ok(None));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_request_keeps_the_song_waiting_and_the_file_as_it_is() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("song.opus");
        std::fs::write(&file, b"audio")?;
        let failed = Err(LibraryError::MusicBrainz("no route to host".into()));
        assert_eq!(write_found(file.clone(), failed).await, Turn::Later);
        assert_eq!(std::fs::read(&file)?, b"audio");
        Ok(())
    }

    #[tokio::test]
    async fn an_answer_without_a_genre_leaves_the_song_asked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("song.opus");
        std::fs::write(&file, b"audio")?;
        // Nothing at all came back (as for a song with no ISRC): nothing is written.
        let nothing = Ok(Enrichment::default());
        assert_eq!(write_found(file.clone(), nothing).await, Turn::Asked);
        assert_eq!(std::fs::read(&file)?, b"audio");
        Ok(())
    }
}
