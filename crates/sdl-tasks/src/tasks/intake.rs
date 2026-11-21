//! Intake (F3): one wanted song into the library. Sync Spotify, Download link and Review all end
//! here; each decides only which songs to take and by which [`Route`].
//!
//! Steps: skip if present, claim the place, match (or go direct), download, look up and tag the
//! temp file, land it. A song that cannot be taken is a failed item with a plain reason; only a
//! failure that would repeat for every song (a missing tool, a settings file that cannot be
//! saved) ends the run.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use sdl_core::{
    Attention, EventKind, Format, HeldSong, Library, MatchOutcome, Place, Playlist, Preset,
    SongSource, WantedSong,
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

/// A playlist that lists a song: its name and the file name it is written under
/// ([`Playlist::file_name`], with the list's ID as fallback). Held songs keep both, so a line can
/// be corrected in the file that was written, whatever the name became once made safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) name: String,
    pub(crate) file: String,
}

impl Listed {
    /// The playlist `name` with Spotify (or link) list `id`.
    pub(crate) fn new(name: &str, id: &str) -> Self {
        Self {
            name: name.to_owned(),
            file: Playlist::file_name(name, id),
        }
    }

    /// The playlists a held song was listed in, with the files it kept.
    pub(crate) fn of_held(held: &HeldSong) -> Vec<Self> {
        held.from
            .iter()
            .zip(held.playlist_files())
            .map(|(name, file)| Self {
                name: name.clone(),
                file,
            })
            .collect()
    }
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

    /// Take one song into `library`. `from` is the playlists that list it (held songs keep them,
    /// and a song that lands in another format has its lines corrected there).
    ///
    /// # Errors
    /// Only a failure that ends the run ([`ends_the_run`]).
    pub(crate) async fn take(
        &self,
        library: &Library,
        song: &WantedSong,
        from: &[Listed],
        route: Route,
    ) -> Result<Outcome, TaskError> {
        match self.try_take(library, song, from, route).await {
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
        library: &Library,
        song: &WantedSong,
        from: &[Listed],
        route: Route,
    ) -> Result<Outcome, TaskError> {
        // The temp folders are under the music root, on the same volume as every library.
        let root = &self.context.config.music_root;
        let place = Place::of(library, song);
        if place.present()?.is_some() {
            return Ok(Outcome::Skipped);
        }
        // A song that is held, or was marked "never", is never searched again (F8). Review takes
        // held songs by another route.
        if route == Route::Search {
            let id = library.id();
            if self.context.is_held(&id, song.key()) {
                return Ok(Outcome::Held(Attention::held(
                    song.title.clone(),
                    song.artists_joined(),
                    "Already waiting in Review: choose a match there.".to_owned(),
                )));
            }
            if self.context.is_never(&id, song.key()) {
                return Ok(Outcome::Skipped);
            }
        }
        if !self.claim(&place) {
            return Ok(Outcome::Skipped);
        }
        // Another library already has it (F12): copy that file, no search and no download.
        if route == Route::Search
            && let Some(source) = place.held_elsewhere(&Library::list(&self.context.config)?)?
        {
            return self.copy_from(&source, &place).await;
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
                            library: library.id(),
                            song: song.clone(),
                            from: from.iter().map(|list| list.name.clone()).collect(),
                            from_files: from.iter().map(|list| list.file.clone()).collect(),
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
            let files: Vec<String> = from.iter().map(|list| list.file.clone()).collect();
            correct_line(library, &files, &place, format)?;
        }
        Ok(match landed {
            Outcome::Added => added(enrichment.genre.is_none(), cover == Cover::Missing),
            other => other,
        })
    }

    /// Land a copy of `source` (a file of another library) at `place`. The copy is made to a temp
    /// file and landed without replacing anything; `source` is only read. It counts as added.
    async fn copy_from(&self, source: &Path, place: &Place) -> Result<Outcome, TaskError> {
        let format = Format::of(source).unwrap_or_default();
        let scratch = scratch_dir(&self.context.config.music_root)?;
        let copy = scratch.path().join(format!("copy.{}", format.extension()));
        let from = source.to_path_buf();
        let to = copy.clone();
        tokio::task::spawn_blocking(move || std::fs::copy(from, to))
            .await
            .map_err(|error| LibraryError::Task(error.to_string()))??;
        Ok(progress::place(&copy, &place.path(format)).await?)
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
            my_library: "Me".into(),
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
        let first = Place::of(&Library::at("Me", dir.path(), true), &song("a", "Same"));
        let again = Place::of(&Library::at("Me", dir.path(), true), &song("b", "Same"));
        assert!(intake.claim(&first));
        assert!(!intake.claim(&again), "same artist, album and title");
        assert!(intake.claim(&Place::of(
            &Library::at("Me", dir.path(), true),
            &song("c", "Other")
        )));
        Ok(())
    }

    #[tokio::test]
    async fn a_present_song_is_skipped_before_any_network_call() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::SyncSpotify);
        let wanted = song("a", "Here");
        let library = Library::mine(&context.config);
        let place = Place::of(&library, &wanted);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::Mp3), b"x")?;
        let intake = Intake::new(&context);
        assert_eq!(
            intake.take(&library, &wanted, &[], Route::Search).await?,
            Outcome::Skipped,
            "present in another format counts"
        );
        Ok(())
    }

    /// Put a file with `bytes` at the place of `wanted` in `library`, as `format`.
    fn put(library: &Library, wanted: &WantedSong, format: Format, bytes: &[u8]) -> TestResult {
        let place = Place::of(library, wanted);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(format), bytes)?;
        Ok(())
    }

    #[tokio::test]
    async fn a_song_another_library_has_is_copied_into_the_one_wanted() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::SyncSpotify);
        let mine = Library::mine(&context.config);
        let sam = Library::resolve(&context.config, &sdl_core::LibraryId::Other("Sam".into()));
        let wanted = song("a", "Shared");
        // Sam has it as M4A; nobody searches or downloads, and nothing can (no network here).
        put(&sam, &wanted, Format::M4a, b"sams audio")?;
        let intake = Intake::new(&context);
        assert_eq!(
            intake.take(&mine, &wanted, &[], Route::Search).await?,
            Outcome::Added
        );
        let landed = Place::of(&mine, &wanted).path(Format::M4a);
        assert_eq!(std::fs::read(&landed)?, b"sams audio");
        let source = Place::of(&sam, &wanted).path(Format::M4a);
        assert_eq!(
            std::fs::read(&source)?,
            b"sams audio",
            "the source is untouched"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_song_for_a_friend_never_lands_in_the_owners_library_or_the_reverse() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::SyncSpotify);
        let mine = Library::mine(&context.config);
        let sam = Library::resolve(&context.config, &sdl_core::LibraryId::Other("Sam".into()));
        let theirs = song("t", "Theirs");
        let ours = song("o", "Ours");
        put(&mine, &theirs, Format::Opus, b"mine")?;
        put(&sam, &ours, Format::Opus, b"sams")?;
        let intake = Intake::new(&context);
        assert_eq!(
            intake.take(&sam, &theirs, &[], Route::Search).await?,
            Outcome::Added
        );
        assert_eq!(
            intake.take(&mine, &ours, &[], Route::Search).await?,
            Outcome::Added
        );
        assert_eq!(
            std::fs::read(Place::of(&sam, &theirs).path(Format::Opus))?,
            b"mine"
        );
        assert_eq!(
            std::fs::read(Place::of(&mine, &ours).path(Format::Opus))?,
            b"sams"
        );
        // Each library holds exactly its two songs, and never a copy of a third.
        assert_eq!(sdl_core::library_files(&mine)?.len(), 2);
        assert_eq!(sdl_core::library_files(&sam)?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn the_copy_shortcut_never_replaces_a_file_and_skips_what_is_present() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, _rx) = runner(dir.path());
        let context = runner.context(TaskId::SyncSpotify);
        let mine = Library::mine(&context.config);
        let sam = Library::resolve(&context.config, &sdl_core::LibraryId::Other("Sam".into()));
        let wanted = song("a", "Both");
        put(&sam, &wanted, Format::Opus, b"sams")?;
        put(&mine, &wanted, Format::Mp3, b"mine")?;
        let intake = Intake::new(&context);
        assert_eq!(
            intake.take(&mine, &wanted, &[], Route::Search).await?,
            Outcome::Skipped
        );
        assert_eq!(
            std::fs::read(Place::of(&mine, &wanted).path(Format::Mp3))?,
            b"mine"
        );
        assert!(!Place::of(&mine, &wanted).path(Format::Opus).exists());
        Ok(())
    }

    #[test]
    fn a_held_song_keeps_the_file_names_its_playlists_were_written_under() -> TestResult {
        // A list whose name is empty once made safe is written under its ID (F7).
        let listed = [
            Listed::new("???", "37i9dQZF1DX"),
            Listed::new("Road: trip", "p1"),
        ];
        assert_eq!(listed[0].file, "37i9dQZF1DX.m3u8");
        assert_eq!(listed[1].file, "Road trip.m3u8");
        let held = HeldSong {
            library: sdl_core::LibraryId::Own,
            song: song("a", "T"),
            from: listed.iter().map(|list| list.name.clone()).collect(),
            from_files: listed.iter().map(|list| list.file.clone()).collect(),
            candidates: Vec::new(),
            reason: "r".into(),
        };
        assert_eq!(Listed::of_held(&held), listed);
        // The correction then finds the file that was written, not one made from the name.
        let dir = tempfile::tempdir()?;
        let library = Library::at("Me", dir.path(), true);
        let wanted = song("a", "T");
        sdl_library::playlists::write(
            &library,
            "???",
            "37i9dQZF1DX",
            std::slice::from_ref(&wanted),
        )?;
        let files: Vec<String> = Listed::of_held(&held).into_iter().map(|l| l.file).collect();
        let place = Place::of(&library, &wanted);
        correct_line(&library, &files, &place, Format::Mp3)?;
        let text = std::fs::read_to_string(dir.path().join("37i9dQZF1DX.m3u8"))?;
        assert!(text.contains("Band/Record/T.mp3"), "{text}");
        // An entry kept before file names were stored falls back to the names.
        let old = HeldSong {
            from_files: Vec::new(),
            ..held
        };
        assert_eq!(
            Listed::of_held(&old).get(1).map(|list| list.file.as_str()),
            Some("Road trip.m3u8")
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
