//! `YouTube` Music matching (`innertube-rs`) and downloading (the `yt-dlp` binary, run directly).

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use innertube_rs::{Innertube, MusicSearchFilter};
use sdl_core::{Format, Track};
use serde::Deserialize;
use tokio::process::Command;

/// The `yt-dlp` binary, found on `PATH`.
const YT_DLP: &str = "yt-dlp";

/// Longest accepted gap between Spotify's duration and a result's.
const DURATION_TOLERANCE: Duration = Duration::from_secs(3);

/// Two texts match when their Jaro-Winkler similarity is at least this.
const SIMILARITY_MIN: f64 = 0.9;

/// Added to the score when the album titles match.
const ALBUM_BONUS: f64 = 0.5;

/// How many search results are looked at, best-ranked first.
const RESULTS_CHECKED: usize = 5;

/// Errors from the `YouTube` crate. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum YoutubeError {
    /// The `YouTube` Music search failed.
    #[error("YouTube Music search failed: {0}")]
    Search(String),
    /// The `yt-dlp` binary could not be started because it is not on `PATH`.
    #[error("yt-dlp was not found on PATH")]
    YtDlpNotFound,
    /// `yt-dlp` ran and failed; carries the last `ERROR:` line of its stderr.
    #[error("yt-dlp failed: {0}")]
    YtDlp(String),
    /// `yt-dlp`'s JSON listing could not be read.
    #[error("could not read yt-dlp's listing: {0}")]
    Listing(String),
    /// File system or process failure.
    #[error("file or process error: {0}")]
    Io(#[from] std::io::Error),
}

/// What Spotify says about a song.
#[derive(Debug, Clone, Copy)]
struct Wanted<'a> {
    title: &'a str,
    artists: &'a [String],
    album: &'a str,
    duration: Duration,
}

/// One `YouTube` Music song result (from `MusicTrackItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    video_id: String,
    title: String,
    artists: Vec<String>,
    /// Album title, when the result has one.
    album: Option<String>,
    /// Duration, when the result has one.
    duration: Option<Duration>,
}

/// Whether a bracketed part names a feature or a version: it holds one of the words `feat`,
/// `featuring`, `ft`, `with`, `remaster`, `remastered`, `version` or `edit`.
fn names_feature_or_version(inner: &str) -> bool {
    inner.split(|c: char| !c.is_alphanumeric()).any(|word| {
        matches!(
            word,
            "feat" | "featuring" | "ft" | "with" | "remaster" | "remastered" | "version" | "edit"
        )
    })
}

/// Lower-case `text`, drop bracketed parts that name a feature or version, drop punctuation and
/// collapse whitespace.
fn normalise(text: &str) -> String {
    let mut base = String::new();
    let mut open: Vec<String> = Vec::new();
    for c in text.to_lowercase().chars() {
        match c {
            '(' | '[' | '{' => open.push(String::new()),
            ')' | ']' | '}' => {
                if let Some(inner) = open.pop()
                    && !names_feature_or_version(&inner)
                {
                    let target = open.last_mut().unwrap_or(&mut base);
                    target.push(' ');
                    target.push_str(&inner);
                    target.push(' ');
                }
            }
            _ => open.last_mut().unwrap_or(&mut base).push(c),
        }
    }
    for rest in open {
        base.push(' ');
        base.push_str(&rest);
    }
    let kept: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `short` is a whole-word prefix of `long`: every word of `short` equals the word at the
/// same place in `long`, and `long` has more words.
fn is_whole_word_prefix(short: &str, long: &str) -> bool {
    let mut short_words = short.split_whitespace();
    let mut long_words = long.split_whitespace();
    loop {
        match (short_words.next(), long_words.next()) {
            (Some(a), Some(b)) if a == b => {}
            (None, Some(_)) => return true,
            _ => return false,
        }
    }
}

/// Whether two normalised titles match: similarity at least [`SIMILARITY_MIN`], or the shorter is a
/// whole-word prefix of the longer. An empty text matches nothing.
fn titles_match(a: &str, b: &str) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && (strsim::jaro_winkler(a, b) >= SIMILARITY_MIN
            || is_whole_word_prefix(a, b)
            || is_whole_word_prefix(b, a))
}

/// The similarity of two normalised artist or album names when they match (at least
/// [`SIMILARITY_MIN`], by similarity only: no substring rule), else `None`. An empty text matches
/// nothing.
fn close_match(a: &str, b: &str) -> Option<f64> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let similarity = strsim::jaro_winkler(a, b);
    (similarity >= SIMILARITY_MIN).then_some(similarity)
}

/// The score of one result, or `None` when it does not pass (F5).
fn score(wanted: &Wanted<'_>, found: &Found) -> Option<f64> {
    let duration = found.duration?;
    if duration.abs_diff(wanted.duration) > DURATION_TOLERANCE {
        return None;
    }
    let want_title = normalise(wanted.title);
    let found_title = normalise(&found.title);
    if !titles_match(&want_title, &found_title) {
        return None;
    }
    let found_artists: Vec<String> = found.artists.iter().map(|a| normalise(a)).collect();
    let mut best_artist: Option<f64> = None;
    for want in wanted.artists.iter().map(|a| normalise(a)) {
        for have in &found_artists {
            if let Some(similarity) = close_match(&want, have) {
                best_artist = Some(best_artist.map_or(similarity, |best| best.max(similarity)));
            }
        }
    }
    let artist = best_artist?;
    let album = found.album.as_deref().map_or(0.0, |album| {
        if close_match(&normalise(wanted.album), &normalise(album)).is_some() {
            ALBUM_BONUS
        } else {
            0.0
        }
    });
    Some(strsim::jaro_winkler(&want_title, &found_title) + artist + album)
}

/// The match rule (F5), a pure function: among the first [`RESULTS_CHECKED`] of `found`, score each
/// and return the index of the best that passes, or `None` (unmatched). Ties go to the earlier
/// result.
///
/// A result passes when its duration is within 3 s of Spotify's, its title matches and at least
/// one artist matches. Text is compared after [`normalise`]; titles match when
/// `strsim::jaro_winkler` is at least 0.9 or the shorter is a whole-word prefix of the longer;
/// artists match by similarity alone; empty text matches nothing. Score = title similarity +
/// best artist similarity + 0.5 when the album titles match.
fn best_match(wanted: &Wanted<'_>, found: &[Found]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, candidate) in found.iter().take(RESULTS_CHECKED).enumerate() {
        if let Some(value) = score(wanted, candidate)
            && best.is_none_or(|(_, top)| value > top)
        {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index)
}

/// An accepted `YouTube` Music result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Video id.
    pub video_id: String,
}

impl Candidate {
    /// `https://music.youtube.com/watch?v=<video_id>`, the URL handed to `yt-dlp`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.video_id)
    }
}

/// Result of matching one song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// A candidate passed the rule.
    Matched(Candidate),
    /// No candidate passed.
    Unmatched,
}

/// One `innertube-rs` client, built once per run and shared by every song.
pub struct Matcher {
    client: Innertube,
}

impl std::fmt::Debug for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matcher").finish_non_exhaustive()
    }
}

impl Matcher {
    /// Build the client (`Innertube::new`).
    ///
    /// # Errors
    /// [`YoutubeError::Search`] when the client cannot start up.
    pub async fn new() -> Result<Self, YoutubeError> {
        let client = Innertube::new()
            .await
            .map_err(|e| YoutubeError::Search(e.to_string()))?;
        Ok(Self { client })
    }

    /// Match `track`: query the ISRC first; if no candidate is accepted, query
    /// `<first artist> <title>`. Uses [`best_match`].
    ///
    /// # Errors
    /// A search failure, which the caller turns into an attention item.
    pub async fn find_match(&self, track: &Track) -> Result<MatchOutcome, YoutubeError> {
        let wanted = Wanted {
            title: &track.title,
            artists: &track.artists,
            album: &track.album,
            duration: track.duration,
        };
        if let Some(isrc) = &track.isrc
            && let Some(candidate) = self.search(isrc, &wanted).await?
        {
            return Ok(MatchOutcome::Matched(candidate));
        }
        let first_artist = track.artists.first().map_or("", String::as_str);
        let query = format!("{first_artist} {}", track.title);
        Ok(self
            .search(&query, &wanted)
            .await?
            .map_or(MatchOutcome::Unmatched, MatchOutcome::Matched))
    }

    async fn search(
        &self,
        query: &str,
        wanted: &Wanted<'_>,
    ) -> Result<Option<Candidate>, YoutubeError> {
        let results = self
            .client
            .search_music(query, Some(MusicSearchFilter::Songs))
            .await
            .map_err(|e| YoutubeError::Search(e.to_string()))?;
        let found: Vec<Found> = results
            .songs
            .into_iter()
            .take(RESULTS_CHECKED)
            .map(|song| Found {
                video_id: song.video_id,
                title: song.title,
                artists: song.artists.into_iter().map(|a| a.name).collect(),
                album: song.album.map(|a| a.title),
                duration: song.duration_ms.map(Duration::from_millis),
            })
            .collect();
        Ok(best_match(wanted, &found)
            .and_then(|index| found.get(index))
            .map(|hit| Candidate {
                video_id: hit.video_id.clone(),
            }))
    }
}

/// Run `yt-dlp` with `args`: stdin null, stdout and stderr piped, killed when dropped.
async fn run_yt_dlp(args: &[&OsStr]) -> Result<Output, YoutubeError> {
    let child = Command::new(YT_DLP)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                YoutubeError::YtDlpNotFound
            } else {
                YoutubeError::Io(error)
            }
        })?;
    let output = child.wait_with_output().await?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(YoutubeError::YtDlp(last_error_line(&output.stderr)))
    }
}

/// The last line of `stderr` that starts with `ERROR:`, without that prefix (the message already
/// says yt-dlp failed), or the exit message when there is none.
fn last_error_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("ERROR:"))
        .map_or_else(
            || "it exited with an error and printed no ERROR line".to_owned(),
            |line| line.trim().to_owned(),
        )
}

/// Who writes the tags and cover of a downloaded file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagsFrom {
    /// The caller does (Sync Spotify tags the temp file with `lofty`).
    Caller,
    /// yt-dlp does (Download link): `--embed-metadata --embed-thumbnail`, confirmed for Opus, MP3
    /// and M4A outputs (see [`download_audio`]).
    YtDlp,
}

/// The yt-dlp arguments that pick and convert the audio for `format` (F9). For `Opus` (the
/// "Original" row) an Opus stream is preferred and copied as served.
fn format_args(format: Format) -> &'static [&'static str] {
    match format {
        Format::Opus => &[
            "-f",
            "bestaudio[acodec=opus]/bestaudio",
            "-x",
            "--audio-format",
            "opus",
        ],
        Format::Mp3 => &[
            "-f",
            "bestaudio",
            "-x",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
        ],
        Format::M4a => &[
            "-f",
            "bestaudio[ext=m4a]/bestaudio",
            "-x",
            "--audio-format",
            "m4a",
        ],
    }
}

/// The whole `yt-dlp` argument list for one download: the format's arguments ([`format_args`]),
/// then `--embed-metadata --embed-thumbnail` for [`TagsFrom::YtDlp`], then
/// `--no-playlist --no-progress -o <template> -- <url>`.
fn download_args(format: Format, tags: TagsFrom, template: &str, url: &str) -> Vec<String> {
    let mut args: Vec<String> = format_args(format).iter().map(|&a| a.to_owned()).collect();
    if tags == TagsFrom::YtDlp {
        args.push("--embed-metadata".to_owned());
        args.push("--embed-thumbnail".to_owned());
    }
    args.extend(
        ["--no-playlist", "--no-progress", "-o", template, "--", url]
            .iter()
            .map(|&a| a.to_owned()),
    );
    args
}

/// Download one item's audio in `format` to `temp` (F6, F9): run `yt-dlp` with
/// `tokio::process::Command`, stdin null, `kill_on_drop(true)`, stdout and stderr piped, with the
/// arguments of [`download_args`]. `url` is [`Candidate::url`] for a match, or an item URL from
/// [`list_link`].
///
/// `temp` must end in `format.extension()` and must not exist yet. Checked with yt-dlp 2026.10.26
/// and ffmpeg 9.0.2 from mise, for every source codec (Opus, M4A, MP3) and target, with and
/// without the embed flags: the finished file lands at exactly `temp`, in the target codec (read
/// back with `ffprobe`); a source already in the target codec is copied, not re-encoded
/// ("`ExtractAudio`: Not converting audio ... already in target format"); any other source is
/// converted with ffmpeg and the intermediate file is deleted. `%` in `temp` is written as `%%`,
/// since `-o` is an output template. The caller owns `temp`'s directory and removes it.
///
/// What [`TagsFrom::YtDlp`] needs: Opus embeds the cover with `mutagen` and fails the download
/// with "module mutagen was not found" when it is missing; M4A tries `mutagen`, then
/// `AtomicParsley`, then ffmpeg; MP3 uses ffmpeg. The yt-dlp that mise installs bundles `mutagen`
/// (`yt-dlp -v` lists `mutagen-1.48.1`), and ffmpeg is a mise tool too. A source with no thumbnail
/// only prints a warning.
///
/// # Errors
/// [`YoutubeError::YtDlpNotFound`], or [`YoutubeError::YtDlp`] with the last `ERROR:` line.
pub async fn download_audio(
    url: &str,
    temp: &Path,
    format: Format,
    tags: TagsFrom,
) -> Result<(), YoutubeError> {
    let args = download_args(format, tags, &output_template(temp), url);
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    run_yt_dlp(&args).await.map(drop)
}

/// `path` as a yt-dlp output template: `%` is written `%%` (yt-dlp README, "OUTPUT TEMPLATE").
fn output_template(path: &Path) -> String {
    path.to_string_lossy().replace('%', "%%")
}

/// A playlist's name and id as `yt-dlp` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInfo {
    /// Playlist title.
    pub name: String,
    /// Playlist id.
    pub id: String,
}

/// One item of a link listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkItem {
    /// The item's own yt-dlp extractor key (e.g. `Youtube`), the site folder name. For a playlist
    /// entry this is its `ie_key`, never the playlist's `extractor_key` (e.g. `YoutubeTab`). Empty
    /// when yt-dlp gives none.
    pub site: String,
    /// Item URL, for [`download_audio`].
    pub url: String,
    /// Item title (may be empty).
    pub title: String,
    /// Video id.
    pub id: String,
}

/// What a link contains (F3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkListing {
    /// Set for a playlist link; its items go in a subfolder named after it.
    pub playlist: Option<PlaylistInfo>,
    /// The items.
    pub items: Vec<LinkItem>,
}

/// The fields of `--dump-single-json` that are read.
#[derive(Debug, Deserialize)]
struct RawListing {
    #[serde(default)]
    extractor_key: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    webpage_url: Option<String>,
    entries: Option<Vec<RawEntry>>,
}

/// The fields of one `entries` element that are read.
#[derive(Debug, Deserialize)]
struct RawEntry {
    #[serde(default)]
    ie_key: String,
    url: Option<String>,
    webpage_url: Option<String>,
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}

/// Turn `yt-dlp`'s JSON into a [`LinkListing`]. A listing with `entries` is a playlist, whose
/// `--flat-playlist` entries carry their own extractor in `ie_key`; one without is a single item
/// whose site is its `extractor_key` and whose URL is its `webpage_url`, else the link itself.
fn parse_listing(json: &[u8], link: &str) -> Result<LinkListing, YoutubeError> {
    let raw: RawListing =
        serde_json::from_slice(json).map_err(|error| YoutubeError::Listing(error.to_string()))?;
    let Some(entries) = raw.entries else {
        return Ok(LinkListing {
            playlist: None,
            items: vec![LinkItem {
                site: raw.extractor_key,
                url: raw.webpage_url.unwrap_or_else(|| link.to_owned()),
                title: raw.title,
                id: raw.id,
            }],
        });
    };
    let items = entries
        .into_iter()
        .map(|entry| {
            let url = entry.url.or(entry.webpage_url).ok_or_else(|| {
                YoutubeError::Listing(format!("item \"{}\" has no URL", entry.title))
            })?;
            Ok(LinkItem {
                site: entry.ie_key,
                url,
                title: entry.title,
                id: entry.id,
            })
        })
        .collect::<Result<Vec<_>, YoutubeError>>()?;
    Ok(LinkListing {
        playlist: Some(PlaylistInfo {
            name: raw.title,
            id: raw.id,
        }),
        items,
    })
}

/// List a link: `yt-dlp --dump-single-json --flat-playlist -- <url>`.
///
/// # Errors
/// [`YoutubeError::YtDlpNotFound`], [`YoutubeError::YtDlp`] or [`YoutubeError::Listing`].
pub async fn list_link(url: &str) -> Result<LinkListing, YoutubeError> {
    let args: [&OsStr; 4] = [
        OsStr::new("--dump-single-json"),
        OsStr::new("--flat-playlist"),
        OsStr::new("--"),
        OsStr::new(url),
    ];
    let output = run_yt_dlp(&args).await?;
    parse_listing(&output.stdout, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&n| n.to_owned()).collect()
    }

    fn found(title: &str, artists: &[&str], album: Option<&str>, secs: u64) -> Found {
        Found {
            video_id: "id".into(),
            title: title.into(),
            artists: names(artists),
            album: album.map(str::to_owned),
            duration: Some(Duration::from_secs(secs)),
        }
    }

    fn wanted(artists: &[String]) -> Wanted<'_> {
        Wanted {
            title: "Get Lucky",
            artists,
            album: "Random Access Memories",
            duration: Duration::from_secs(369),
        }
    }

    #[test]
    fn candidate_url_is_the_music_watch_url() {
        let candidate = Candidate {
            video_id: "abc123".into(),
        };
        assert_eq!(candidate.url(), "https://music.youtube.com/watch?v=abc123");
    }

    #[test]
    fn normalise_drops_feature_and_version_brackets_only() {
        assert_eq!(
            normalise("Get Lucky (feat. Pharrell Williams and Nile Rodgers)"),
            "get lucky"
        );
        assert_eq!(normalise("Song [Remastered 2011]"), "song");
        assert_eq!(normalise("Song (Radio Edit)"), "song");
        assert_eq!(normalise("Song (ft. X) (Live)"), "song live");
        assert_eq!(normalise("Don't Stop!"), "dont stop");
        assert_eq!(normalise("Song (Featuring X)"), "song");
        assert_eq!(normalise("Song (2009 Remaster)"), "song");
        assert_eq!(normalise("Song (Live [feat. X])"), "song live");
        assert_eq!(normalise("Song (Unclosed"), "song unclosed");
    }

    #[test]
    fn brackets_that_only_start_like_a_keyword_are_kept() {
        assert_eq!(normalise("Birds (Feather Mix)"), "birds feather mix");
        assert_eq!(normalise("Night (Editor's Cut)"), "night editors cut");
        assert_eq!(normalise("Love (Without You)"), "love without you");
    }

    #[test]
    fn a_bracketed_part_that_is_not_a_feature_still_counts() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Get Lucky (Without You)", &["Daft Punk"], None, 369)];
        let mut spotify = wanted(&artists);
        spotify.title = "Get Lucky (Daft Punk Remix)";
        assert_eq!(best_match(&spotify, &results), None);
    }

    #[test]
    fn each_format_has_its_documented_arguments() {
        assert_eq!(
            format_args(Format::Opus).join(" "),
            "-f bestaudio[acodec=opus]/bestaudio -x --audio-format opus"
        );
        assert_eq!(
            format_args(Format::Mp3).join(" "),
            "-f bestaudio -x --audio-format mp3 --audio-quality 0"
        );
        assert_eq!(
            format_args(Format::M4a).join(" "),
            "-f bestaudio[ext=m4a]/bestaudio -x --audio-format m4a"
        );
    }

    #[test]
    fn download_args_end_with_the_fixed_tail_and_embed_only_when_asked() {
        let plain = download_args(Format::Mp3, TagsFrom::Caller, "/t/a.mp3", "https://u");
        assert_eq!(
            plain.join(" "),
            "-f bestaudio -x --audio-format mp3 --audio-quality 0 \
             --no-playlist --no-progress -o /t/a.mp3 -- https://u"
        );
        for format in Format::DOWNLOAD {
            let tagged = download_args(format, TagsFrom::YtDlp, "/t/a", "https://u");
            let joined = tagged.join(" ");
            assert!(joined.contains("--embed-metadata --embed-thumbnail --no-playlist"));
            assert!(joined.ends_with("-o /t/a -- https://u"));
        }
    }

    #[test]
    fn output_template_escapes_percent() {
        assert_eq!(
            output_template(Path::new("/m/100% Hits/.sdl-tmp/x/audio.opus")),
            "/m/100%% Hits/.sdl-tmp/x/audio.opus"
        );
    }

    #[test]
    fn get_lucky_matches_its_featuring_title() {
        let artists = names(&["Daft Punk"]);
        let results = [found(
            "Get Lucky (feat. Pharrell Williams and Nile Rodgers)",
            &["Daft Punk", "Pharrell Williams", "Nile Rodgers"],
            Some("Random Access Memories"),
            369,
        )];
        assert_eq!(best_match(&wanted(&artists), &results), Some(0));
    }

    #[test]
    fn live_version_with_another_length_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Get Lucky (Live Version)", &["Daft Punk"], None, 452)];
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn cover_by_another_artist_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Get Lucky", &["Midnight Cover Band"], None, 369)];
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn wrong_duration_is_rejected_and_three_seconds_passes() {
        let artists = names(&["Daft Punk"]);
        let far = [found("Get Lucky", &["Daft Punk"], None, 373)];
        let near = [found("Get Lucky", &["Daft Punk"], None, 372)];
        assert_eq!(best_match(&wanted(&artists), &far), None);
        assert_eq!(best_match(&wanted(&artists), &near), Some(0));
    }

    #[test]
    fn missing_duration_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let mut result = found("Get Lucky", &["Daft Punk"], None, 369);
        result.duration = None;
        assert_eq!(best_match(&wanted(&artists), &[result]), None);
    }

    #[test]
    fn unrelated_title_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Around the World", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn one_matching_artist_is_enough() {
        let artists = names(&["Pharrell Williams", "Daft Punk"]);
        let results = [found("Get Lucky", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&wanted(&artists), &results), Some(0));
    }

    #[test]
    fn album_match_breaks_a_tie() {
        let artists = names(&["Daft Punk"]);
        let results = [
            found("Get Lucky", &["Daft Punk"], Some("Greatest Hits"), 369),
            found(
                "Get Lucky",
                &["Daft Punk"],
                Some("Random Access Memories"),
                369,
            ),
        ];
        assert_eq!(best_match(&wanted(&artists), &results), Some(1));
    }

    #[test]
    fn equal_scores_keep_the_earlier_result() {
        let artists = names(&["Daft Punk"]);
        let results = [
            found("Get Lucky", &["Daft Punk"], None, 369),
            found("Get Lucky", &["Daft Punk"], None, 369),
        ];
        assert_eq!(best_match(&wanted(&artists), &results), Some(0));
    }

    #[test]
    fn only_the_first_five_results_are_checked() {
        let artists = names(&["Daft Punk"]);
        let mut results: Vec<Found> = (0..RESULTS_CHECKED)
            .map(|_| found("Other Song", &["Someone"], None, 100))
            .collect();
        results.push(found("Get Lucky", &["Daft Punk"], None, 369));
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn empty_normalised_text_matches_nothing() {
        let artists = names(&["Daft Punk"]);
        let mut spotify = wanted(&artists);
        spotify.title = "(feat. X)";
        let results = [found("Get Lucky", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&spotify, &results), None);
    }

    fn matches(title: &str, artist: &str, result: &Found) -> bool {
        let artists = names(&[artist]);
        let mut spotify = wanted(&artists);
        spotify.title = title;
        best_match(&spotify, std::slice::from_ref(result)).is_some()
    }

    fn plain_get_lucky() -> Found {
        found("Get Lucky", &["Daft Punk"], None, 369)
    }

    #[test]
    fn tribute_band_is_not_the_artist() {
        let tribute = found("Get Lucky", &["Daft Punk Tribute Band"], None, 369);
        assert!(!matches("Get Lucky", "Daft Punk", &tribute));
        assert!(!matches(
            "Get Lucky",
            "Daft Punk Tribute Band",
            &plain_get_lucky()
        ));
    }

    #[test]
    fn artist_inside_a_longer_name_is_not_a_match() {
        let result = found("Get Lucky", &["The Daft Punk Experience"], None, 369);
        assert!(!matches("Get Lucky", "Daft Punk", &result));
    }

    #[test]
    fn remaster_suffix_matches_bracketed_or_dashed_on_either_side() {
        let dashed = found("Get Lucky - 2013 Remaster", &["Daft Punk"], None, 369);
        assert!(matches("Get Lucky", "Daft Punk", &dashed));
        let bracketed = found("Get Lucky (2013 Remaster)", &["Daft Punk"], None, 369);
        assert!(matches("Get Lucky", "Daft Punk", &bracketed));
        assert!(matches(
            "Get Lucky [Remastered 2013]",
            "Daft Punk",
            &plain_get_lucky()
        ));
    }

    #[test]
    fn featured_artist_suffix_matches_on_either_side() {
        let result = found(
            "Get Lucky (feat. Pharrell Williams)",
            &["Daft Punk"],
            None,
            369,
        );
        assert!(matches("Get Lucky", "Daft Punk", &result));
        assert!(matches(
            "Get Lucky (ft. Pharrell Williams)",
            "Daft Punk",
            &plain_get_lucky()
        ));
    }

    #[test]
    fn a_short_title_inside_a_long_one_needs_a_whole_word_prefix() {
        let mid = found("Crazy in Love", &["Beyonce"], None, 369);
        assert!(!matches("Love", "Beyonce", &mid));
        let partial_word = found("Heyday", &["Beyonce"], None, 369);
        assert!(!matches("Hey", "Beyonce", &partial_word));
        let prefix = found("Love Me Do", &["Beyonce"], None, 369);
        assert!(matches("Love", "Beyonce", &prefix));
        let short = found("Love", &["Beyonce"], None, 369);
        assert!(matches("Love Me Do", "Beyonce", &short));
    }

    #[test]
    fn a_whole_word_prefix_title_with_the_wrong_length_is_rejected() {
        let artists = names(&["Beyonce"]);
        let mut spotify = wanted(&artists);
        spotify.title = "Love";
        let results = [found("Love Me Do", &["Beyonce"], None, 150)];
        assert_eq!(best_match(&spotify, &results), None);
    }

    #[test]
    fn live_version_is_left_to_the_duration_check() {
        let live = found("Get Lucky (Live)", &["Daft Punk"], None, 369);
        assert!(matches("Get Lucky", "Daft Punk", &live));
        let long_live = found("Get Lucky (Live)", &["Daft Punk"], None, 452);
        assert!(!matches("Get Lucky", "Daft Punk", &long_live));
    }

    #[test]
    fn empty_artist_text_matches_nothing() {
        let blank = found("Get Lucky", &["", "(feat. X)"], None, 369);
        assert!(!matches("Get Lucky", "", &blank));
        assert!(!matches("Get Lucky", "Daft Punk", &blank));
    }

    #[test]
    fn last_error_line_is_the_last_one_starting_with_error() {
        let stderr =
            b"WARNING: x\nERROR: first\nnoise\nERROR: [youtube] abc: Video unavailable\ntrailing\n";
        assert_eq!(last_error_line(stderr), "[youtube] abc: Video unavailable");
        assert_eq!(
            YoutubeError::YtDlp(last_error_line(b"noise\n")).to_string(),
            "yt-dlp failed: it exited with an error and printed no ERROR line"
        );
    }

    #[test]
    fn a_playlist_and_a_single_item_are_told_apart() -> TestResult {
        let playlist = parse_listing(
            br#"{"extractor_key":"YoutubeTab","id":"PL1","title":"Mix","entries":[{"ie_key":"Youtube","url":"https://u/1","id":"1","title":"One"}]}"#,
            "link",
        )?;
        assert_eq!(playlist.playlist.map(|p| p.name), Some("Mix".to_owned()));
        assert_eq!(playlist.items.len(), 1);
        // The item's own extractor, never the playlist wrapper's.
        assert_eq!(
            playlist.items.first().map(|i| i.site.as_str()),
            Some("Youtube")
        );
        let single = parse_listing(
            br#"{"extractor_key":"Youtube","id":"v","title":"T","webpage_url":"https://u/v"}"#,
            "link",
        )?;
        assert!(single.playlist.is_none());
        assert_eq!(
            single.items.first().map(|i| i.site.as_str()),
            Some("Youtube")
        );
        assert_eq!(
            single.items.first().map(|i| i.url.as_str()),
            Some("https://u/v")
        );
        Ok(())
    }
}
