//! What a pasted link contains (F5), as wanted songs.
//!
//! Both calls run `yt-dlp` ([`crate::ytdlp`]) and read its JSON with `serde`; the reading itself
//! is pure and tested without the binary.

use std::ffi::OsString;
use std::time::Duration;

use sdl_core::{SongSource, WantedSong};
use serde::Deserialize;
use url::Url;

use crate::{YoutubeError, ytdlp};

/// A playlist link's name and id as `yt-dlp` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInfo {
    /// Playlist title; the playlist file is named after it (F7). The id when the title is empty.
    pub name: String,
    /// Playlist id, the file-name fallback.
    pub id: String,
}

/// One item of a flat listing. A flat listing can lack fields, so only the URL is relied on;
/// [`read_item`] reads the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkItem {
    /// The item's own URL, as the listing gave it (a `music.youtube.com` link keeps that host).
    pub url: String,
    /// Title in the flat listing (may be empty).
    pub title: String,
    /// Video id (may be empty).
    pub id: String,
}

/// What a link contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkListing {
    /// Set for a playlist link; its items are in list order.
    pub playlist: Option<PlaylistInfo>,
    /// The items, in order.
    pub items: Vec<LinkItem>,
}

/// The fields of `yt-dlp`'s JSON that either call reads. Missing and `null` both read as `None`.
#[derive(Debug, Default, Deserialize)]
struct Json {
    #[serde(rename = "_type")]
    kind: Option<String>,
    id: Option<String>,
    url: Option<String>,
    title: Option<String>,
    track: Option<String>,
    artist: Option<String>,
    uploader: Option<String>,
    album: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    thumbnails: Option<Vec<Thumbnail>>,
    webpage_url: Option<String>,
    original_url: Option<String>,
    entries: Option<Vec<Option<Json>>>,
}

/// One entry of `yt-dlp`'s `thumbnails` list (worst first).
#[derive(Debug, Default, Deserialize)]
struct Thumbnail {
    url: Option<String>,
}

/// The cover: `thumbnail` when it is a JPEG or PNG, else the best JPEG or PNG in `thumbnails`
/// (`yt-dlp` lists them worst first), else `thumbnail`. `YouTube`'s best thumbnail is usually
/// WebP, which `lofty` cannot store as a cover (`Picture::from_reader` knows PNG, JPEG, GIF, BMP
/// and TIFF only), so taking it would fail the song's tag write; the same picture is also served
/// as JPEG.
fn cover(thumbnail: Option<String>, thumbnails: Option<Vec<Thumbnail>>) -> Option<String> {
    let storable = |url: &str| {
        Url::parse(url).is_ok_and(|parsed| {
            let path = parsed.path().to_ascii_lowercase();
            [".jpg", ".jpeg", ".png"]
                .iter()
                .any(|ext| path.ends_with(ext))
        })
    };
    let thumbnail = text(thumbnail);
    if thumbnail.as_deref().is_some_and(storable) {
        return thumbnail;
    }
    thumbnails
        .unwrap_or_default()
        .into_iter()
        .rev()
        .filter_map(|entry| text(entry.url))
        .find(|url| storable(url))
        .or(thumbnail)
}

/// The text when it has anything in it.
fn text(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn args(flags: &[&str], url: &str) -> Vec<OsString> {
    flags
        .iter()
        .copied()
        .chain(["--", url])
        .map(OsString::from)
        .collect()
}

/// List a link: `yt-dlp --dump-single-json --flat-playlist -- <url>` gives the items and, for a
/// playlist, its name and order. A link that is a single video is a listing of one item and no
/// playlist. A playlist entry the listing gives no URL for (an unavailable video) is left out.
///
/// # Errors
/// [`YoutubeError::YtDlpNotFound`], [`YoutubeError::YtDlp`] (with `yt-dlp`'s own reason) or
/// [`YoutubeError::Listing`] when its answer cannot be read.
pub async fn list_link(url: &str) -> Result<LinkListing, YoutubeError> {
    let out = ytdlp::run(&args(&["--dump-single-json", "--flat-playlist"], url)).await?;
    parse_listing(&out)
}

/// Read one item's own metadata at its turn, with
/// `yt-dlp --dump-single-json --no-playlist -- <item url>`, as a wanted song: title is `track`,
/// else `title`; artist is `artist`, else `uploader`; `album`; `duration`; cover is `thumbnail`
/// (its JPEG or PNG form when it is WebP, see [`cover`]).
/// The song's source is a link item whose `direct` is [`is_music_link`] of the item's URL (the
/// extractor name is `youtube` for both hosts, so the host is what tells them apart).
///
/// # Errors
/// As [`list_link`]; a title-less item is [`YoutubeError::Listing`].
pub async fn read_item(item: &LinkItem) -> Result<WantedSong, YoutubeError> {
    let out = ytdlp::run(&args(&["--dump-single-json", "--no-playlist"], &item.url)).await?;
    parse_item(&out, item)
}

fn parse_json(bytes: &[u8]) -> Result<Json, YoutubeError> {
    serde_json::from_slice(bytes).map_err(|error| YoutubeError::Listing(error.to_string()))
}

fn parse_listing(bytes: &[u8]) -> Result<LinkListing, YoutubeError> {
    let json = parse_json(bytes)?;
    if json.kind.as_deref() != Some("playlist") {
        // A single video: its own URL as typed (so a `music.youtube.com` link stays one).
        let url = text(json.original_url)
            .or_else(|| text(json.webpage_url))
            .or_else(|| text(json.url))
            .ok_or_else(|| YoutubeError::Listing("the link has no address".to_owned()))?;
        return Ok(LinkListing {
            playlist: None,
            items: vec![LinkItem {
                url,
                title: json.title.unwrap_or_default(),
                id: json.id.unwrap_or_default(),
            }],
        });
    }
    let id = json.id.unwrap_or_default();
    let name = text(json.title).unwrap_or_else(|| id.clone());
    let items = json
        .entries
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(LinkItem {
                url: text(entry.url).or_else(|| text(entry.webpage_url))?,
                title: entry.title.unwrap_or_default(),
                id: entry.id.unwrap_or_default(),
            })
        })
        .collect();
    Ok(LinkListing {
        playlist: Some(PlaylistInfo { name, id }),
        items,
    })
}

fn parse_item(bytes: &[u8], item: &LinkItem) -> Result<WantedSong, YoutubeError> {
    let json = parse_json(bytes)?;
    let title = text(json.track)
        .or_else(|| text(json.title))
        .ok_or_else(|| YoutubeError::Listing("the item has no title".to_owned()))?;
    let artist = text(json.artist).or_else(|| text(json.uploader));
    let video_id = text(json.id).unwrap_or_else(|| item.id.clone());
    Ok(WantedSong {
        source: SongSource::Link {
            item_url: text(json.webpage_url).unwrap_or_else(|| item.url.clone()),
            video_id,
            direct: is_music_link(&item.url),
        },
        title,
        artists: artist.into_iter().collect(),
        album: text(json.album),
        album_artist: None,
        track_number: None,
        disc_number: None,
        date: None,
        isrc: None,
        duration: json
            .duration
            .map(f64::round)
            .and_then(|secs| Duration::try_from_secs_f64(secs).ok()),
        cover_url: cover(json.thumbnail, json.thumbnails),
    })
}

/// Whether `url` is a `music.youtube.com` link: such an item is its own source and downloads
/// without matching. Every other link, plain `YouTube` included, is matched (F3).
#[must_use]
pub fn is_music_link(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host == "music.youtube.com"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn item(url: &str) -> LinkItem {
        LinkItem {
            url: url.to_owned(),
            title: String::new(),
            id: "listed".to_owned(),
        }
    }

    #[test]
    fn only_music_youtube_is_its_own_source() {
        assert!(is_music_link("https://music.youtube.com/watch?v=abc"));
        assert!(!is_music_link("https://www.youtube.com/watch?v=abc"));
        assert!(!is_music_link("https://youtu.be/abc"));
        assert!(!is_music_link("https://evil.example/music.youtube.com"));
        assert!(!is_music_link("not a url"));
    }

    #[test]
    fn a_playlist_keeps_its_name_and_order() -> TestResult {
        let json = br#"{"_type":"playlist","id":"PL1","title":"Road trip","entries":[
            {"url":"https://music.youtube.com/watch?v=b","title":"B","id":"b"},
            null,
            {"url":null,"title":"[Private video]","id":"p"},
            {"url":"https://music.youtube.com/watch?v=a","title":"A","id":"a"}]}"#;
        let listing = parse_listing(json)?;
        assert_eq!(
            listing.playlist,
            Some(PlaylistInfo {
                name: "Road trip".into(),
                id: "PL1".into()
            })
        );
        let urls: Vec<&str> = listing.items.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://music.youtube.com/watch?v=b",
                "https://music.youtube.com/watch?v=a"
            ]
        );
        Ok(())
    }

    #[test]
    fn a_playlist_without_a_title_is_named_by_its_id() -> TestResult {
        let listing =
            parse_listing(br#"{"_type":"playlist","id":"PL1","title":" ","entries":[]}"#)?;
        assert_eq!(listing.playlist.map(|p| p.name), Some("PL1".to_owned()));
        assert_eq!(listing.items, Vec::new());
        Ok(())
    }

    #[test]
    fn a_single_video_is_one_item_with_the_address_as_typed() -> TestResult {
        let json = br#"{"_type":"video","id":"v","title":"T",
            "webpage_url":"https://www.youtube.com/watch?v=v",
            "original_url":"https://music.youtube.com/watch?v=v"}"#;
        let listing = parse_listing(json)?;
        assert_eq!(listing.playlist, None);
        assert_eq!(listing.items.len(), 1);
        assert_eq!(
            listing.items.first().map(|i| i.url.as_str()),
            Some("https://music.youtube.com/watch?v=v")
        );
        Ok(())
    }

    #[test]
    fn unreadable_json_is_a_listing_error() {
        assert!(matches!(
            parse_listing(b"not json"),
            Err(YoutubeError::Listing(_))
        ));
    }

    #[test]
    fn an_item_prefers_track_and_artist_over_title_and_uploader() -> TestResult {
        let json = br#"{"id":"v","track":"Song","title":"Song (Official Video)","artist":"Band",
            "uploader":"BandVEVO","album":"Album","duration":213.4,"thumbnail":"https://i/t.jpg",
            "webpage_url":"https://www.youtube.com/watch?v=v"}"#;
        let song = parse_item(json, &item("https://music.youtube.com/watch?v=v"))?;
        assert_eq!(song.title, "Song");
        assert_eq!(song.artists, ["Band"]);
        assert_eq!(song.album.as_deref(), Some("Album"));
        assert_eq!(song.duration, Some(Duration::from_secs(213)));
        assert_eq!(song.cover_url.as_deref(), Some("https://i/t.jpg"));
        assert_eq!(
            song.source,
            SongSource::Link {
                item_url: "https://www.youtube.com/watch?v=v".into(),
                video_id: "v".into(),
                direct: true,
            }
        );
        Ok(())
    }

    #[test]
    fn an_item_falls_back_to_title_and_uploader_and_is_matched_off_music() -> TestResult {
        let json = br#"{"title":"Some Video","track":"","artist":null,"uploader":"Channel"}"#;
        let song = parse_item(json, &item("https://www.youtube.com/watch?v=x"))?;
        assert_eq!(song.title, "Some Video");
        assert_eq!(song.artists, ["Channel"]);
        assert_eq!(song.album, None);
        assert_eq!(song.duration, None);
        assert_eq!(
            song.source,
            SongSource::Link {
                item_url: "https://www.youtube.com/watch?v=x".into(),
                video_id: "listed".into(),
                direct: false,
            }
        );
        Ok(())
    }

    #[test]
    fn a_webp_thumbnail_is_swapped_for_the_best_jpeg_of_the_same_picture() -> TestResult {
        // The shape of real `yt-dlp` output for a `YouTube` Music track (2026.10.26).
        let json = br#"{"title":"T","uploader":"U",
            "thumbnail":"https://i.ytimg.com/vi_webp/v/maxresdefault.webp",
            "thumbnails":[{"url":"https://i.ytimg.com/vi/v/hqdefault.jpg?sqp=x"},
                {"url":"https://i.ytimg.com/vi_webp/v/hqdefault.webp"},
                {"url":"https://i.ytimg.com/vi/v/maxresdefault.jpg"},
                {"url":"https://i.ytimg.com/vi_webp/v/maxresdefault.webp"}]}"#;
        let song = parse_item(json, &item("https://music.youtube.com/watch?v=v"))?;
        assert_eq!(
            song.cover_url.as_deref(),
            Some("https://i.ytimg.com/vi/v/maxresdefault.jpg")
        );
        let only_webp = br#"{"title":"T","thumbnail":"https://i/x.webp","thumbnails":[]}"#;
        let song = parse_item(only_webp, &item("https://x/y"))?;
        assert_eq!(song.cover_url.as_deref(), Some("https://i/x.webp"));
        Ok(())
    }

    #[test]
    fn an_item_with_no_title_is_an_error() {
        let result = parse_item(br#"{"title":"","artist":"A"}"#, &item("https://x/y"));
        assert!(matches!(result, Err(YoutubeError::Listing(_))));
    }

    #[test]
    fn the_url_is_always_after_the_separator() {
        let args = args(&["--dump-single-json"], "--bad");
        assert_eq!(args.last().and_then(|a| a.to_str()), Some("--bad"));
        assert_eq!(args.get(1).and_then(|a| a.to_str()), Some("--"));
    }
}
