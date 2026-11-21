//! Intake (F3): one wanted song into the library. Sync Spotify, Download link and Review all end
//! here; each decides only which songs to take and by which [`Route`].
//!
//! Steps: skip if present, claim the place, match (or go direct), download, look up and tag the
//! temp file, land it. A song that cannot be taken is a failed item with a plain reason; only a
//! failure that would repeat for every song (a missing tool, a settings file that cannot be
//! saved) ends the run.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use sdl_core::{
    Attention, EventKind, Format, HeldSong, MatchOutcome, Place, Preset, SongSource, WantedSong,
};
use sdl_library::LibraryError;
use sdl_library::converter::convert_file;
use sdl_library::playlists::correct_line;
use sdl_library::tags::{Cover, Enrichment, ReleaseCache, lookup, write_tags};
use sdl_youtube::{Matcher, YoutubeError, download_opus, download_own_audio};
use tokio::sync::OnceCell;

use crate::failure::error_info;
use crate::progress::{self, Outcome, scratch_dir};
use crate::{TaskContext, TaskError};

/// Where a song's audio comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// Find it on `YouTube` Music (a song that is its own source goes direct, without matching).
    Search,
    /// Take the audio of this link as Opus (a chosen candidate, or a pasted link).
    Download(String),
    /// Take this link's own audio as served (review, F8).
    OwnAudio(String),
}

/// Takes wanted songs into the library for one run. Safe to share between the songs running at
/// once.
pub(crate) struct Intake<'a> {
    context: &'a TaskContext,
    matcher: OnceCell<Matcher>,
    cache: ReleaseCache,
    claimed: Mutex<HashSet<Place>>,
}

impl<'a> Intake<'a> {
    pub(crate) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            // Made at the first song that needs a search, so a run that never searches (review,
            // direct links) never starts the client.
            matcher: OnceCell::new(),
            cache: ReleaseCache::default(),
            claimed: Mutex::default(),
        }
    }

    /// Take one song. `from` names the playlists that list it (held songs keep them, and a song
    /// that lands in another format has its lines corrected there).
    ///
    /// # Errors
    /// Only a failure that ends the run ([`ends_the_run`]).
    pub(crate) async fn take(
        &self,
        song: &WantedSong,
        from: &[String],
        route: Route,
    ) -> Result<Outcome, TaskError> {
        match self.try_take(song, from, route).await {
            Err(error) if !ends_the_run(&error) => {
                Ok(failed(&song.title, &song.artists_joined(), &error))
            }
            other => other,
        }
    }

    /// `true` the first time a place is claimed in this run; later songs with the same place are
    /// not taken again.
    fn claim(&self, place: &Place) -> bool {
        self.claimed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(place.clone())
    }

    async fn try_take(
        &self,
        song: &WantedSong,
        from: &[String],
        route: Route,
    ) -> Result<Outcome, TaskError> {
        let root = &self.context.config.music_root;
        let place = Place::of(root, song);
        if place.present()?.is_some() {
            return Ok(Outcome::Skipped);
        }
        // A song that is held, or was marked "never", is never searched again (F8). Review takes
        // held songs by another route.
        if route == Route::Search {
            if self.context.is_held(song.key()) {
                return Ok(Outcome::Held(Attention::held(
                    song.title.clone(),
                    song.artists_joined(),
                    "Already waiting in Review: choose a match there.".to_owned(),
                )));
            }
            if self.context.is_never(song.key()) {
                return Ok(Outcome::Skipped);
            }
        }
        if !self.claim(&place) {
            return Ok(Outcome::Skipped);
        }
        self.context
            .events
            .send(EventKind::Headline(format!("Getting {}", describe(song))));
        let (url, own_audio) = match route {
            Route::Download(url) => (url, false),
            Route::OwnAudio(url) => (url, true),
            Route::Search => match &song.source {
                SongSource::Link {
                    item_url,
                    direct: true,
                    ..
                } => (item_url.clone(), false),
                _ => match self.find(song).await? {
                    MatchOutcome::Matched(candidate) => (candidate.url(), false),
                    MatchOutcome::Held { candidates, reason } => {
                        self.context.hold(HeldSong {
                            song: song.clone(),
                            from: from.to_vec(),
                            candidates,
                            reason: reason.clone(),
                        })?;
                        return Ok(Outcome::Held(Attention::held(
                            song.title.clone(),
                            song.artists_joined(),
                            format!("{reason} Choose what to do with it in Review."),
                        )));
                    }
                    MatchOutcome::Failed(reason) => {
                        return Ok(Outcome::Failed(Attention::failed(
                            song.title.clone(),
                            song.artists_joined(),
                            format!("{reason} Check the internet connection, then try again."),
                        )));
                    }
                },
            },
        };
        let downloaded = if own_audio {
            download_own_audio(&url, root).await?
        } else {
            download_opus(&url, root).await?
        };
        // Audio the library does not hold as it is becomes Opus (F3); the scratch folder lives
        // until the file has landed.
        let converted;
        let (file, format) = if let Some(format) = downloaded.format() {
            (downloaded.file().to_path_buf(), format)
        } else {
            converted = scratch_dir(root)?;
            let out = converted.path().join("audio.opus");
            convert_file(downloaded.file(), &out, Preset::OPUS).await?;
            (out, Format::Opus)
        };
        let enrichment = if song.isrc.is_some() {
            // A failed or empty lookup leaves those tags empty; the song still completes (F6).
            lookup(song, &self.cache).await.unwrap_or_default()
        } else {
            Enrichment::default()
        };
        let cover = write_tags(&file, song, &enrichment).await?;
        // Another task may have landed the song while this one worked, in any format.
        if place.present()?.is_some() {
            return Ok(Outcome::Skipped);
        }
        let landed = progress::place(&file, &place.path(format)).await?;
        if landed == Outcome::Added && format != Format::Opus {
            correct_line(root, from, &place, format)?;
        }
        Ok(match landed {
            Outcome::Added => added(enrichment.genre.is_none(), cover == Cover::Missing),
            other => other,
        })
    }

    async fn find(&self, song: &WantedSong) -> Result<MatchOutcome, TaskError> {
        let matcher = self.matcher.get_or_try_init(Matcher::new).await?;
        Ok(matcher.find(song).await)
    }
}

/// A song that landed: plain [`Outcome::Added`] unless it has no genre or no cover.
fn added(without_genre: bool, without_cover: bool) -> Outcome {
    if without_genre || without_cover {
        Outcome::AddedMissing {
            genre: without_genre,
            cover: without_cover,
        }
    } else {
        Outcome::Added
    }
}

/// `Title by Artist, Artist`, or the title alone.
fn describe(song: &WantedSong) -> String {
    if song.artists.is_empty() {
        song.title.clone()
    } else {
        format!("{} by {}", song.title, song.artists_joined())
    }
}

/// Whether `error` would repeat for every further song, so the run ends instead of listing a
/// failure per song.
pub(crate) fn ends_the_run(error: &TaskError) -> bool {
    matches!(
        error,
        TaskError::Youtube(YoutubeError::YtDlpNotFound | YoutubeError::Search(_))
            | TaskError::Library(LibraryError::FfmpegNotFound)
            | TaskError::State(_)
    )
}

/// A failed item: what happened and what to do, in one reason.
pub(crate) fn failed(title: &str, artist: &str, error: &TaskError) -> Outcome {
    let info = error_info(error);
    Outcome::Failed(Attention::failed(
        title.to_owned(),
        artist.to_owned(),
        format!("{}. {}", info.message.trim_end_matches('.'), info.advice),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use sdl_core::{Config, ReviewList, State, TaskId, channel};
    use std::num::NonZeroUsize;

    use super::*;
    use crate::runner::Runner;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    pub(crate) fn song(id: &str, title: &str) -> WantedSong {
        WantedSong {
            source: SongSource::Spotify {
                track_id: id.into(),
            },
            title: title.into(),
            artists: vec!["Band".into()],
            album: Some("Record".into()),
            album_artist: None,
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: Some(Duration::from_secs(200)),
            cover_url: None,
        }
    }

    pub(crate) fn runner(dir: &std::path::Path) -> (Runner, sdl_core::EventReceiver) {
        let config = Config {
            client_id: "id".into(),
            music_root: dir.join("music"),
            download_jobs: NonZeroUsize::MIN,
            animations: true,
        };
        let (tx, rx) = channel();
        let runner = Runner::with_files(
            config,
            (State::default(), dir.join("state.toml")),
            (ReviewList::default(), dir.join("review.toml")),
            tx,
        );
        (runner, rx)
    }

    #[test]
    fn a_place_is_claimed_once_per_run() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::SyncSpotify);
        let intake = Intake::new(&context);
        let first = Place::of(dir.path(), &song("a", "Same"));
        let again = Place::of(dir.path(), &song("b", "Same"));
        assert!(intake.claim(&first));
        assert!(!intake.claim(&again), "same artist, album and title");
        assert!(intake.claim(&Place::of(dir.path(), &song("c", "Other"))));
        Ok(())
    }

    #[tokio::test]
    async fn a_present_song_is_skipped_before_any_network_call() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::SyncSpotify);
        let wanted = song("a", "Here");
        let place = Place::of(&context.config.music_root, &wanted);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::Mp3), b"x")?;
        let intake = Intake::new(&context);
        assert_eq!(
            intake.take(&wanted, &[], Route::Search).await?,
            Outcome::Skipped,
            "present in another format counts"
        );
        Ok(())
    }

    #[test]
    fn only_failures_that_repeat_end_the_run() {
        assert!(ends_the_run(&TaskError::Youtube(
            YoutubeError::YtDlpNotFound
        )));
        assert!(ends_the_run(&TaskError::Library(
            LibraryError::FfmpegNotFound
        )));
        assert!(!ends_the_run(&TaskError::Youtube(YoutubeError::YtDlp(
            "Video unavailable".into()
        ))));
    }

    #[test]
    fn a_failed_item_says_what_happened_and_what_to_do() -> Result<(), String> {
        let error = TaskError::Youtube(YoutubeError::YtDlp("Video unavailable".into()));
        let Outcome::Failed(item) = failed("T", "A", &error) else {
            return Err("expected a failed item".to_owned());
        };
        assert!(item.reason.contains("Video unavailable"));
        assert!(item.reason.contains("try again"));
        Ok(())
    }
}
