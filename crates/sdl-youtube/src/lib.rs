//! `YouTube` Music matching and downloading, via the `yt-dlp` crate (uses `yt-dlp` and `ffmpeg` on `PATH`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sdl_core::paths::{file_stem, link_dir};
use sdl_core::{Event, EventSender, Stage, Status, Track, emit};
use strsim::jaro_winkler;
use yt_dlp::Downloader;
use yt_dlp::client::Libraries;
use yt_dlp::model::Video;
use yt_dlp::model::selector::AudioCodecPreference;

/// How far a result's duration may differ from Spotify's.
const DURATION_TOLERANCE: Duration = Duration::from_secs(3);

/// Minimum `strsim` Jaro-Winkler similarity (0..=1) for both title and artist.
const SIMILARITY_MIN: f64 = 0.85;

/// How many search results are looked at, best-ranked first.
const RESULTS_CHECKED: usize = 5;

/// Errors from the `YouTube` crate. The message carries the whole cause chain, because the UI shows
/// only this text; no `source()` is exposed, so a caller printing the chain does not repeat it.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct YoutubeError(String);

impl From<yt_dlp::error::Error> for YoutubeError {
    fn from(error: yt_dlp::error::Error) -> Self {
        // yt-dlp's IO errors put the OS reason only in `source()`; some other variants already
        // include their source in the message, which is not repeated.
        let mut text = format!("yt-dlp: {error}");
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            let cause_text = cause.to_string();
            if !text.contains(&cause_text) {
                text.push_str(": ");
                text.push_str(&cause_text);
            }
            source = cause.source();
        }
        Self(text)
    }
}

impl From<std::io::Error> for YoutubeError {
    fn from(error: std::io::Error) -> Self {
        Self(format!("file error: {error}"))
    }
}

impl From<url::ParseError> for YoutubeError {
    fn from(error: url::ParseError) -> Self {
        Self(format!("search URL: {error}"))
    }
}

/// A `YouTube` Music search result that passed the match rules, with the video info already
/// fetched for the match, so [`Client::download`] needs no second lookup.
#[derive(Debug, Clone)]
pub struct Candidate {
    video: Video,
}

/// What Spotify says about a song.
#[derive(Debug, Clone, Copy)]
struct Wanted<'a> {
    title: &'a str,
    artists: &'a [String],
    duration: Duration,
}

/// What a search result says about itself. `names` are its uploader and channel.
#[derive(Debug, Clone, Copy)]
struct Found<'a> {
    title: &'a str,
    names: &'a [&'a str],
    duration: Duration,
}

/// Lowercase, letters and digits only, single spaces.
fn normalize(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    mapped
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `YouTube` auto-generated artist channels are named `<artist> - Topic`.
fn strip_topic(name: &str) -> &str {
    name.trim_end_matches(" - Topic")
}

/// The match rule: duration within [`DURATION_TOLERANCE`], title similar, and at least one Spotify
/// artist similar to the uploader or channel name.
fn is_match(wanted: Wanted<'_>, found: Found<'_>) -> bool {
    if wanted.duration.abs_diff(found.duration) > DURATION_TOLERANCE {
        return false;
    }
    let title = jaro_winkler(&normalize(wanted.title), &normalize(found.title));
    if title < SIMILARITY_MIN {
        return false;
    }
    wanted.artists.iter().any(|artist| {
        let artist = normalize(artist);
        found
            .names
            .iter()
            .any(|name| jaro_winkler(&artist, &normalize(strip_topic(name))) >= SIMILARITY_MIN)
    })
}

/// A `YouTube` Music search URL restricted to songs (`#songs`, handled by yt-dlp's
/// `youtube:music:search_url` extractor).
fn search_url(query: &str) -> Result<String, url::ParseError> {
    let mut url = url::Url::parse_with_params("https://music.youtube.com/search", [("q", query)])?;
    url.set_fragment(Some("songs"));
    Ok(url.into())
}

/// A file stem for `title` ([`sdl_core::paths::file_stem`]), never empty (falls back to `id`).
fn item_stem(title: &str, id: &str) -> String {
    let stem = file_stem(title);
    if stem.is_empty() { file_stem(id) } else { stem }
}

fn video_duration(video: &Video) -> Option<Duration> {
    video
        .duration
        .and_then(|secs| u64::try_from(secs).ok())
        .map(Duration::from_secs)
}

/// One `yt_dlp::Downloader`, built once per run and shared by every song, so all downloads go through
/// one download manager.
#[derive(Debug)]
pub struct Client {
    downloader: Downloader,
}

impl Client {
    /// Build the downloader with `Libraries::new("yt-dlp", "ffmpeg")` (bare names, resolved on `PATH`
    /// by the process spawn) and the system temp dir as its output dir. Never installs binaries
    /// (`with_new_binaries` / `LibraryInstaller` are not used).
    ///
    /// # Errors
    /// The downloader could not be built.
    pub async fn new() -> Result<Self, YoutubeError> {
        let libraries = Libraries::new(PathBuf::from("yt-dlp"), PathBuf::from("ffmpeg"));
        let downloader = Downloader::builder(libraries, std::env::temp_dir())
            .build()
            .await?;
        Ok(Self { downloader })
    }

    /// Find a match for `track`: search `YouTube` Music by ISRC first, then by `artist title`. Accept a
    /// candidate only when its duration is within a few seconds of Spotify's and artist/title are
    /// similar (`strsim`). `Ok(None)` means unmatched.
    ///
    /// The search results carry no duration or artist, so the first [`RESULTS_CHECKED`] results are
    /// fetched one by one, best-ranked first; the first that passes the rule is returned.
    ///
    /// # Errors
    /// The search or a result lookup failed.
    pub async fn find_match(&self, track: &Track) -> Result<Option<Candidate>, YoutubeError> {
        let wanted = Wanted {
            title: &track.title,
            artists: &track.artists,
            duration: track.duration,
        };
        if let Some(isrc) = &track.isrc
            && let Some(found) = self.search(isrc, wanted).await?
        {
            return Ok(Some(found));
        }
        let query = format!("{} {}", track.artists_joined(), track.title);
        self.search(&query, wanted).await
    }

    async fn search(
        &self,
        query: &str,
        wanted: Wanted<'_>,
    ) -> Result<Option<Candidate>, YoutubeError> {
        let results = self
            .downloader
            .fetch_playlist_infos(search_url(query)?)
            .await?;
        for entry in results.entries.iter().take(RESULTS_CHECKED) {
            let video = self.downloader.fetch_video_infos(&entry.url).await?;
            let Some(duration) = video_duration(&video) else {
                continue;
            };
            let names: Vec<&str> = [video.uploader.as_deref(), video.channel.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            let found = Found {
                title: &video.title,
                names: &names,
                duration,
            };
            if is_match(wanted, found) {
                return Ok(Some(Candidate { video }));
            }
        }
        Ok(None)
    }

    /// Download the best Opus stream of `candidate` as `.opus` (stream copy, no re-encoding) to a temp
    /// file outside the library, then rename it to `dest`.
    ///
    /// # Errors
    /// The download or the rename failed.
    pub async fn download(&self, candidate: &Candidate, dest: &Path) -> Result<(), YoutubeError> {
        self.save(&candidate.video, dest).await
    }

    /// Download `video`'s best Opus audio to a temp file, then rename it to `dest`.
    async fn save(&self, video: &Video, dest: &Path) -> Result<(), YoutubeError> {
        let temp = tempfile::Builder::new()
            .prefix("sdl-")
            .suffix(".opus")
            .tempfile()?
            .into_temp_path();
        self.downloader
            .download(video, &*temp)
            .audio_only()
            .audio_codec(AudioCodecPreference::Opus)
            .execute()
            .await?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        temp.persist(dest).map_err(|e| e.error)?;
        Ok(())
    }

    /// Download any link yt-dlp supports (track or playlist) into
    /// [`sdl_core::paths::link_dir`]`(music_root, extractor_key, playlist title)`. Sends a
    /// [`sdl_core::Event::Song`] per item (keyed by item title) and [`sdl_core::Event::Progress`].
    /// Returns the files written.
    ///
    /// A file that already exists is skipped. A failed item is reported as an event and the rest
    /// continue; the call fails only when nothing was written and something failed.
    ///
    /// # Errors
    /// The link is unsupported, or every item that was tried failed.
    pub async fn download_link(
        &self,
        url: &str,
        music_root: &Path,
        events: &EventSender,
    ) -> Result<Vec<PathBuf>, YoutubeError> {
        let listing = self.downloader.fetch_playlist_infos(url).await?;
        // A single video parses as a playlist with no entries and the video's own title.
        let (items, playlist): (Vec<(String, String)>, Option<&str>) = if listing.entries.is_empty()
        {
            (vec![(url.to_owned(), listing.title.clone())], None)
        } else {
            let items = listing
                .entries
                .iter()
                .map(|e| {
                    // A private video's title may be empty; its URL keeps the row visible.
                    let key = if e.title.is_empty() { &e.url } else { &e.title };
                    (e.url.clone(), key.clone())
                })
                .collect();
            (items, Some(listing.title.as_str()))
        };

        let total = items.len();
        let mut written = Vec::new();
        let mut first_error = None;
        for (index, (item_url, key)) in items.into_iter().enumerate() {
            let song = |status: Status| {
                emit(
                    events,
                    Event::Song {
                        key: key.clone(),
                        stage: Stage::Downloading,
                        status,
                    },
                );
            };
            song(Status::InProgress);
            emit(events, Event::Activity(format!("Downloading {key}")));
            match self.link_item(&item_url, music_root, playlist).await {
                Ok(Some(path)) => {
                    song(Status::Done);
                    written.push(path);
                }
                Ok(None) => song(Status::SkippedExists),
                Err(e) => {
                    song(Status::Failed(e.to_string()));
                    first_error.get_or_insert(e);
                }
            }
            emit(
                events,
                Event::Progress {
                    done: index.saturating_add(1),
                    total,
                },
            );
        }
        match first_error {
            Some(e) if written.is_empty() => Err(e),
            _ => Ok(written),
        }
    }

    /// One link item. `Ok(None)` when the file already exists.
    async fn link_item(
        &self,
        url: &str,
        music_root: &Path,
        playlist: Option<&str>,
    ) -> Result<Option<PathBuf>, YoutubeError> {
        let video = self.downloader.fetch_video_infos(url).await?;
        let dir = link_dir(music_root, &video.extractor_info.extractor_key, playlist);
        let dest = dir.join(format!("{}.opus", item_stem(&video.title, &video.id)));
        if tokio::fs::try_exists(&dest).await? {
            return Ok(None);
        }
        self.save(&video, &dest).await?;
        Ok(Some(dest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn artists(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn check(
        title: &str,
        spotify_artists: &[&str],
        secs: u64,
        found_title: &str,
        names: &[&str],
        found_secs: u64,
    ) -> bool {
        let spotify_artists = artists(spotify_artists);
        is_match(
            Wanted {
                title,
                artists: &spotify_artists,
                duration: Duration::from_secs(secs),
            },
            Found {
                title: found_title,
                names,
                duration: Duration::from_secs(found_secs),
            },
        )
    }

    #[test]
    fn exact_match_is_accepted() {
        assert!(check(
            "Get Lucky",
            &["Daft Punk"],
            248,
            "Get Lucky",
            &["Daft Punk"],
            248
        ));
    }

    #[test]
    fn duration_within_tolerance_is_accepted() {
        assert!(check("Song", &["Band"], 200, "Song", &["Band"], 203));
        assert!(check("Song", &["Band"], 200, "Song", &["Band"], 197));
    }

    #[test]
    fn duration_beyond_tolerance_is_rejected() {
        assert!(!check("Song", &["Band"], 200, "Song", &["Band"], 204));
        assert!(!check("Song", &["Band"], 200, "Song", &["Band"], 150));
    }

    #[test]
    fn different_title_is_rejected() {
        assert!(!check(
            "Song",
            &["Band"],
            200,
            "Another Tune Entirely",
            &["Band"],
            200
        ));
    }

    #[test]
    fn different_artist_is_rejected() {
        assert!(!check(
            "Song",
            &["Band"],
            200,
            "Song",
            &["Someone Else"],
            200
        ));
    }

    #[test]
    fn no_names_is_rejected() {
        assert!(!check("Song", &["Band"], 200, "Song", &[], 200));
    }

    #[test]
    fn topic_channel_matches_artist() {
        assert!(check(
            "Song",
            &["Band"],
            200,
            "Song",
            &["Band - Topic"],
            200
        ));
    }

    #[test]
    fn any_spotify_artist_may_match() {
        assert!(check(
            "Song",
            &["Lead", "Guest"],
            200,
            "Song",
            &["Guest"],
            200
        ));
    }

    #[test]
    fn any_uploader_or_channel_name_may_match() {
        assert!(check(
            "Song",
            &["Band"],
            200,
            "Song",
            &["Label Records", "Band"],
            200
        ));
    }

    #[test]
    fn case_and_punctuation_are_ignored() {
        assert!(check(
            "Don't Stop!",
            &["AC/DC"],
            200,
            "DON'T STOP",
            &["ac dc"],
            200
        ));
    }

    #[test]
    fn normalize_collapses_to_words() {
        assert_eq!(normalize("  Hello,  World! (Live) "), "hello world live");
    }

    #[test]
    fn strip_topic_only_removes_the_suffix() {
        assert_eq!(strip_topic("Band - Topic"), "Band");
        assert_eq!(strip_topic("Topic Band"), "Topic Band");
    }

    #[test]
    fn search_url_encodes_query_and_selects_songs() -> TestResult {
        let url = url::Url::parse(&search_url("AC/DC Back & Black")?)?;
        assert_eq!(url.host_str(), Some("music.youtube.com"));
        assert_eq!(url.path(), "/search");
        assert_eq!(url.fragment(), Some("songs"));
        let query: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(
            query,
            vec![("q".to_owned(), "AC/DC Back & Black".to_owned())]
        );
        Ok(())
    }

    #[test]
    fn ytdlp_error_message_carries_the_os_reason_once() {
        let source = std::io::Error::from(std::io::ErrorKind::NotFound);
        let reason = source.to_string();
        let error = YoutubeError::from(yt_dlp::error::Error::io("spawn", source));
        let text = error.to_string();
        assert_eq!(text, format!("yt-dlp: IO error during spawn: {reason}"));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn item_stem_is_safe_and_never_empty() {
        assert_eq!(item_stem("A: B?", "id1"), "A B");
        assert_eq!(item_stem("...", "id1"), "id1");
        assert_eq!(item_stem(".hidden", "id1"), "hidden");
    }
}
