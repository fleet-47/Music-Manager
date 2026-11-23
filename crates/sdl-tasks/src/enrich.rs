//! Enrichment (F6): the queue of landed songs that wait for genre and totals, and the background
//! worker that asks `MusicBrainz` for them and adds them to the files in the library.
//!
//! Interface, in full:
//! * [`Queue`] is the only writer of `pending_enrichment.toml` (F11). A download adds a landed
//!   song through `TaskContext::queue_enrichment`; nobody else adds, and only the worker removes.
//! * [`work`] is the worker: one song at a time, oldest first, at the pace of the `MusicBrainz`
//!   client's own limiter. It sleeps while the queue is empty and wakes when a song is added. It
//!   never ends by itself: the runner aborts it when the app closes, which is safe at any point,
//!   because a file is replaced whole or not at all (`sdl_library::tags::add_enrichment`) and it
//!   starts no outside tool.
//! * A song leaves the queue when `MusicBrainz` answered (with or without data) or its file is no
//!   longer in the library. When the request or the write failed it stays and is tried again
//!   after [`RETRY`].
//!
//! The worker is not one of the five functions: it has no card, sends no events and is never
//! stopped by the owner.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sdl_core::{
    Config, Library, LibraryId, PendingEnrichment, PendingSong, Place, StoreError, WantedSong,
};
use sdl_library::LibraryError;
use sdl_library::tags::{Enrichment, ReleaseCache, add_enrichment, lookup};
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

    /// Queue a landed song, save, and wake the worker. The song is queued in memory even when
    /// the save fails.
    ///
    /// # Errors
    /// The queue file could not be written.
    pub(crate) fn add(&self, library: LibraryId, song: WantedSong) -> Result<(), StoreError> {
        let saved = {
            let mut songs = self.songs.lock().unwrap_or_else(PoisonError::into_inner);
            songs.add(PendingSong { library, song });
            songs.save_to(&self.path)
        };
        self.wake.notify_one();
        saved
    }

    /// The song that has waited longest.
    fn next(&self) -> Option<PendingSong> {
        self.songs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next()
            .cloned()
    }

    /// Take `done` out of the queue and save. It is out of the queue in memory even when the
    /// save fails; the file then still lists it, and the next start of the app asks again.
    fn remove(&self, done: &PendingSong) -> Result<(), StoreError> {
        let mut songs = self.songs.lock().unwrap_or_else(PoisonError::into_inner);
        songs.remove(&done.library, done.song.key());
        songs.save_to(&self.path)
    }

    /// How many songs wait.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.songs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// How one song's turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// It leaves the queue.
    Done,
    /// It stays; try again after [`RETRY`].
    Later,
}

/// The worker: enrich the queue's songs one at a time for as long as the app runs.
pub(crate) async fn work(queue: Queue, config: Arc<Config>) {
    // One release lookup per release while the app runs, across every run's songs.
    let cache = ReleaseCache::default();
    loop {
        let Some(next) = queue.next() else {
            // `add` leaves a wake-up behind when nobody waits yet, so none is missed.
            queue.wake.notified().await;
            continue;
        };
        let found = lookup(&next.song, &cache).await;
        match settle(&config, &next, found).await {
            Turn::Done => {
                // A failed save is covered by `Queue::remove`: nothing to do here.
                let _ = queue.remove(&next);
            }
            Turn::Later => tokio::time::sleep(RETRY).await,
        }
    }
}

/// What to do with `next` now that `MusicBrainz` answered `found`: add what was found to its
/// file in the library.
async fn settle(
    config: &Config,
    next: &PendingSong,
    found: Result<Enrichment, LibraryError>,
) -> Turn {
    let Ok(found) = found else {
        return Turn::Later;
    };
    let place = Place::of(&Library::resolve(config, &next.library), &next.song);
    let file = match place.present() {
        Ok(Some(format)) => place.path(format),
        // The song was taken out of the library: nothing to enrich.
        Ok(None) => return Turn::Done,
        Err(_) => return Turn::Later,
    };
    let written = tokio::task::spawn_blocking(move || add_enrichment(&file, &found)).await;
    match written {
        Ok(Ok(())) => Turn::Done,
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

    #[tokio::test]
    async fn a_queued_song_is_saved_at_once_and_wakes_the_worker() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("pending_enrichment.toml");
        let queue = Queue::new(PendingEnrichment::default(), path.clone());
        assert_eq!(queue.next(), None);
        queue.add(LibraryId::Own, song("a", "First"))?;
        queue.add(LibraryId::Own, song("b", "Second"))?;
        queue.add(LibraryId::Own, song("a", "First"))?;
        assert_eq!(queue.len(), 2, "a song waits once");
        assert_eq!(PendingEnrichment::load_from(&path)?.len(), 2);
        // The wake-up left by `add` is there for a worker that starts waiting only now.
        tokio::time::timeout(Duration::from_secs(5), queue.wake.notified()).await?;
        let first = queue.next().ok_or("nothing waits")?;
        assert_eq!(first.song.key(), "a");
        queue.remove(&first)?;
        assert_eq!(
            queue.next().map(|next| next.song.key().to_owned()),
            Some("b".into())
        );
        assert_eq!(PendingEnrichment::load_from(&path)?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_request_keeps_the_song_for_later() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        let next = waiting("a");
        let file = put(&config, &next, b"audio")?;
        let failed = Err(LibraryError::MusicBrainz("no route to host".into()));
        assert_eq!(settle(&config, &next, failed).await, Turn::Later);
        assert_eq!(std::fs::read(&file)?, b"audio", "the file is not touched");
        Ok(())
    }

    #[tokio::test]
    async fn a_song_no_longer_in_the_library_leaves_the_queue() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        assert_eq!(settle(&config, &waiting("a"), Ok(jazz())).await, Turn::Done);
        Ok(())
    }

    #[tokio::test]
    async fn an_answer_with_nothing_in_it_leaves_the_queue_and_the_file_alone() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        let next = waiting("a");
        let file = put(&config, &next, b"audio")?;
        let nothing = Ok(Enrichment::default());
        assert_eq!(settle(&config, &next, nothing).await, Turn::Done);
        assert_eq!(std::fs::read(&file)?, b"audio");
        Ok(())
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_tagged_stays_as_it_is_and_is_tried_again() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path());
        let next = waiting("a");
        let file = put(&config, &next, b"not audio at all")?;
        assert_eq!(settle(&config, &next, Ok(jazz())).await, Turn::Later);
        assert_eq!(std::fs::read(&file)?, b"not audio at all");
        Ok(())
    }
}
