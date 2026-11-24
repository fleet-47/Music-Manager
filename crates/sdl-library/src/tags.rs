//! Genre, totals and tags (F6): one `lofty` write of the song's own tags to the temp file before
//! it lands, the `MusicBrainz` lookup, and adding what it found to the file in the library.

use std::collections::HashMap;
use std::io::{Cursor, Seek as _};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile as _, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use musicbrainz_rs::api_bindium::ApiClient;
use musicbrainz_rs::api_bindium::ureq::config::Config;
use musicbrainz_rs::api_bindium::ureq::http::Uri;
use musicbrainz_rs::api_bindium::ureq::unversioned::resolver::{
    DefaultResolver, ResolvedSocketAddrs, Resolver,
};
use musicbrainz_rs::api_bindium::ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use musicbrainz_rs::api_bindium::ureq::{self, Agent};
use musicbrainz_rs::client::MusicBrainzClient;
use musicbrainz_rs::entity::genre::Genre;
use musicbrainz_rs::entity::recording::{Recording, RecordingSearchQuery};
use musicbrainz_rs::entity::release::Release;
use musicbrainz_rs::prelude::*;
use sdl_core::landing::write_atomic;
use sdl_core::{Library, Place, WantedSong};
use tokio::sync::OnceCell;

use crate::LibraryError;

/// Extra tag data from `MusicBrainz`. A missing value stays `None` and leaves its tag empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enrichment {
    /// Genre.
    pub genre: Option<String>,
    /// Total tracks on the song's disc.
    pub track_total: Option<u32>,
    /// Number of discs on the release.
    pub disc_total: Option<u32>,
}

/// What one release lookup yields: the disc total and the release group's top genre.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReleaseInfo {
    disc_total: Option<u32>,
    group_genre: Option<String>,
}

/// Release lookups by release ID, cached for one run: one lookup per release ID, even when
/// several songs of the same album are looked up at once.
#[derive(Debug, Default)]
pub struct ReleaseCache {
    releases: Mutex<HashMap<String, Arc<OnceCell<ReleaseInfo>>>>,
}

impl ReleaseCache {
    /// The cell for `release_id`; the first caller to initialise it makes the one lookup.
    fn cell(&self, release_id: &str) -> Arc<OnceCell<ReleaseInfo>> {
        let mut map = self.releases.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(map.entry(release_id.to_owned()).or_default())
    }
}

/// Look up genre and totals by ISRC. Uses one process-wide `MusicBrainzClient` with a generic
/// User-Agent ([`USER_AGENT`]), so its `rate_limit` limiter is shared by every concurrent song.
///
/// `MusicBrainz` calls per song with an ISRC: one recording search (`isrc:<code>`), one recording
/// lookup (`inc=genres+artists+releases+media`; the response carries the genres of the recording
/// and of its artists), and one release lookup per release ID per run (`inc=genres+release-groups+media`,
/// through `cache`), which gives the disc total and the release group's genres. `musicbrainz_rs`
/// 0.14 has no `release-groups` include on a recording lookup, so the release group's genre comes
/// from that release lookup instead.
///
/// Genre: the recording's top genre, else its release group's, else its first artist's. Track
/// total: the track count of the medium whose position is the song's disc number, on the release
/// whose title matches the album. Missing data stays `None`.
///
/// # Errors
/// The recording search or lookup failed; the pipeline treats this as empty data. A failed
/// release lookup is not an error: the recording's genre and the track total are still returned.
pub async fn lookup(song: &WantedSong, cache: &ReleaseCache) -> Result<Enrichment, LibraryError> {
    let Some(isrc) = song.isrc.as_deref() else {
        return Ok(Enrichment::default());
    };
    let client = mb_client();
    let query = RecordingSearchQuery::query_builder().isrc(isrc).build();
    let found = Recording::search(query)
        .execute_with_client_async(client)
        .await
        .map_err(|e| LibraryError::MusicBrainz(e.to_string()))?;
    let Some(first) = found.entities.first() else {
        return Ok(Enrichment::default());
    };
    let recording = Recording::fetch()
        .id(&first.id)
        .with_genres()
        .with_artists()
        .with_releases()
        .with_medias()
        .execute_with_client_async(client)
        .await
        .map_err(|e| LibraryError::MusicBrainz(e.to_string()))?;
    let matched = matching_release(&recording, song);
    let release = match matched {
        Some((release, _)) => {
            let cell = cache.cell(&release.id);
            // A failed release lookup loses only what comes from the release (disc total and
            // the release group's genre); the recording's own genre and the track total stay.
            cell.get_or_try_init(|| release_info(&release.id))
                .await
                .ok()
                .cloned()
        }
        None => None,
    };
    let artist_genres = recording
        .artist_credit
        .iter()
        .flatten()
        .next()
        .and_then(|credit| credit.artist.genres.as_deref());
    let genre = top_genre(recording.genres.as_deref())
        .or_else(|| release.as_ref().and_then(|r| r.group_genre.clone()))
        .or_else(|| top_genre(artist_genres));
    Ok(Enrichment {
        genre,
        track_total: matched.map(|(_, total)| total),
        disc_total: release.and_then(|r| r.disc_total),
    })
}

/// The one release lookup for a release ID: medium count and the release group's top genre.
async fn release_info(release_id: &str) -> Result<ReleaseInfo, LibraryError> {
    let release = Release::fetch()
        .id(release_id)
        .with_genres()
        .with_release_groups()
        .with_media()
        .execute_with_client_async(mb_client())
        .await
        .map_err(|e| LibraryError::MusicBrainz(e.to_string()))?;
    Ok(ReleaseInfo {
        disc_total: release
            .media
            .as_ref()
            .and_then(|media| u32::try_from(media.len()).ok()),
        group_genre: release
            .release_group
            .as_ref()
            .and_then(|group| top_genre(group.genres.as_deref())),
    })
}

/// The genre with the highest vote count; the first of equals.
fn top_genre(genres: Option<&[Genre]>) -> Option<String> {
    let mut best: Option<&Genre> = None;
    for genre in genres.unwrap_or_default() {
        if best.is_none_or(|b| genre.count.unwrap_or(0) > b.count.unwrap_or(0)) {
            best = Some(genre);
        }
    }
    best.map(|genre| genre.name.clone())
}

/// The first release whose title matches the album and that has a medium at the song's disc
/// number, with that medium's track count.
fn matching_release<'a>(recording: &'a Recording, song: &WantedSong) -> Option<(&'a Release, u32)> {
    let album = song.album.as_deref()?;
    let disc = song.disc_number.unwrap_or(1);
    recording
        .releases
        .iter()
        .flatten()
        .filter(|release| release.title.eq_ignore_ascii_case(album))
        .find_map(|release| {
            let medium = release
                .media
                .iter()
                .flatten()
                .find(|medium| medium.position == Some(disc))?;
            Some((release, medium.track_count))
        })
}

/// How long a cover download may take, from connecting to the last byte (F6).
const COVER_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether a written file carries the song's cover (F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    /// The cover is stored as the front cover.
    Stored,
    /// The song has no cover: it has no cover URL, the download failed or timed out, or the image
    /// is one the tag library cannot store. Every other tag was still written.
    Missing,
}

/// The cover image at `url`, giving up after 20 seconds.
async fn fetch_cover(url: &str) -> Result<Picture, Box<dyn std::error::Error + Send + Sync>> {
    let bytes = reqwest::Client::builder()
        .timeout(COVER_TIMEOUT)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let mut picture = Picture::from_reader(&mut Cursor::new(bytes))?;
    picture.set_pic_type(PictureType::CoverFront);
    Ok(picture)
}

/// Write the song's own tags and its cover (downloaded from `song.cover_url`, giving up after 20
/// seconds) to the temp `file` in one write, before it lands. Genre and totals are not written
/// here: they come later ([`add_enrichment`]). A cover that cannot be downloaded or stored leaves
/// the song without one ([`Cover::Missing`]); the other tags are still written.
///
/// # Errors
/// The file cannot be read or written.
pub async fn write_tags(file: &Path, song: &WantedSong) -> Result<Cover, LibraryError> {
    let picture = match &song.cover_url {
        Some(url) => fetch_cover(url).await.ok(),
        None => None,
    };
    let tag_err = |message: String| LibraryError::Tag {
        path: file.to_path_buf(),
        message,
    };
    let mut tagged = lofty::read_from_path(file).map_err(|e| tag_err(e.to_string()))?;
    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .primary_tag_mut()
        .ok_or_else(|| tag_err("no writable tag".to_owned()))?;
    fill_tag(tag, song);
    let cover = if let Some(picture) = picture {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(picture);
        Cover::Stored
    } else {
        Cover::Missing
    };
    tag.save_to_path(file, WriteOptions::default())
        .map_err(|e| tag_err(e.to_string()))?;
    Ok(cover)
}

/// Whether a song's `file` has a genre tag (F6: a song with one is not missing anything the
/// enrichment queue is for). Blocking.
///
/// # Errors
/// The file cannot be read.
pub fn has_genre(file: &Path) -> Result<bool, LibraryError> {
    let tagged = lofty::read_from_path(file).map_err(|e| LibraryError::Tag {
        path: file.to_path_buf(),
        message: e.to_string(),
    })?;
    Ok(tagged
        .primary_tag()
        .and_then(Accessor::genre)
        .is_some_and(|genre| !genre.trim().is_empty()))
}

/// Add genre, track total and disc total to a song's `file` in the library (F6). The file is
/// read into memory, tagged there and written back whole ([`write_atomic`]), so a crash leaves
/// the old file or the new one. Every other tag, the cover and the audio stay as they are; a
/// value `enrichment` does not have leaves its tag as it is, and with none the file is not
/// touched. Blocking: call it from a blocking thread.
///
/// # Errors
/// The file cannot be read, tagged or written.
pub fn add_enrichment(file: &Path, enrichment: &Enrichment) -> Result<(), LibraryError> {
    if *enrichment == Enrichment::default() {
        return Ok(());
    }
    let tag_err = |message: String| LibraryError::Tag {
        path: file.to_path_buf(),
        message,
    };
    let mut bytes = Cursor::new(std::fs::read(file).map_err(|e| tag_err(e.to_string()))?);
    let mut tagged = Probe::new(&mut bytes)
        .guess_file_type()
        .map_err(|e| tag_err(e.to_string()))?
        .read()
        .map_err(|e| tag_err(e.to_string()))?;
    let tag = tagged
        .primary_tag_mut()
        .ok_or_else(|| tag_err("no tag to add to".to_owned()))?;
    if let Some(genre) = &enrichment.genre {
        tag.set_genre(genre.clone());
    }
    if let Some(n) = enrichment.track_total {
        tag.set_track_total(n);
    }
    if let Some(n) = enrichment.disc_total {
        tag.set_disk_total(n);
    }
    bytes.rewind().map_err(|e| tag_err(e.to_string()))?;
    tagged
        .save_to(&mut bytes, WriteOptions::default())
        .map_err(|e| tag_err(e.to_string()))?;
    write_atomic(file, bytes.get_ref()).map_err(|e| tag_err(e.to_string()))
}

/// What every `MusicBrainz` request says it comes from: the app, its version and where its
/// maintainer can be reached, as `MusicBrainz` requires (F6).
const USER_AGENT: &str = concat!(
    "sdl/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/fleet-47/Music-Manager )"
);

/// The one process-wide client: the rate limiter lives inside it. Its agent tries a host's IPv4
/// addresses before its IPv6 ones ([`Ipv4First`]).
fn mb_client() -> &'static MusicBrainzClient {
    static CLIENT: OnceLock<MusicBrainzClient> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let config = Config::builder().user_agent(USER_AGENT).build();
        let agent = Agent::with_parts(config, DefaultConnector::default(), Ipv4First::default());
        MusicBrainzClient::builder()
            .api_client(ApiClient::builder().agent(agent).build())
            .build()
    })
}

/// ureq's own resolver with the IPv4 addresses moved to the front (F6). ureq connects to the
/// addresses in order and has no connect timeout by default, so an IPv6 address that does not
/// answer costs the system's 75 seconds before IPv4 is tried; IPv6 is still used when no IPv4
/// address is there or none connects.
#[derive(Debug, Default)]
struct Ipv4First(DefaultResolver);

impl Resolver for Ipv4First {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let mut addrs = self.0.resolve(uri, config, timeout)?;
        ipv4_first(&mut addrs);
        Ok(addrs)
    }
}

/// IPv4 addresses first, each family in the order it came (the sort is stable).
fn ipv4_first(addrs: &mut [SocketAddr]) {
    addrs.sort_by_key(SocketAddr::is_ipv6);
}

/// Set the song's own fields on `tag` (F6, at landing); a field the song lacks leaves its tag unset.
fn fill_tag(tag: &mut Tag, song: &WantedSong) {
    tag.set_title(song.title.clone());
    if let Some(album) = &song.album {
        tag.set_album(album.clone());
    }
    tag.remove_artist();
    for artist in &song.artists {
        tag.push(lofty::tag::TagItem::new(
            ItemKey::TrackArtist,
            lofty::tag::ItemValue::Text(artist.clone()),
        ));
    }
    if let Some(album_artist) = &song.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, album_artist.clone());
    }
    if let Some(number) = song.track_number {
        tag.set_track(number);
    }
    if let Some(number) = song.disc_number {
        tag.set_disk(number);
    }
    if let Some(date) = &song.date {
        tag.insert_text(ItemKey::RecordingDate, date.clone());
    }
    if let Some(isrc) = &song.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }
}

/// Where a file that carries tags belongs in `library` (F10, F12, From device):
/// album artist (else first artist), album (else `Singles`) and title from its tags, by
/// [`Place::new`], with the file's stem as the ID fallback. `None` when the file has no title or
/// no artist tag; the caller then files it under `Unsorted` (F10).
///
/// # Errors
/// [`LibraryError::Tag`] when the file cannot be read.
pub fn place_from_tags(file: &Path, library: &Library) -> Result<Option<Place>, LibraryError> {
    let tagged = lofty::read_from_path(file).map_err(|e| LibraryError::Tag {
        path: file.to_path_buf(),
        message: e.to_string(),
    })?;
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };
    let text = |key| {
        tag.get_strings(key)
            .map(str::trim)
            .find(|value| !value.is_empty())
    };
    let Some(title) = text(ItemKey::TrackTitle) else {
        return Ok(None);
    };
    let Some(artist) = text(ItemKey::AlbumArtist).or_else(|| text(ItemKey::TrackArtist)) else {
        return Ok(None);
    };
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Some(Place::new(
        library,
        artist,
        text(ItemKey::AlbumTitle),
        title,
        &stem,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_addresses_are_tried_before_ipv6_ones() -> Result<(), std::net::AddrParseError> {
        let mut addrs: Vec<SocketAddr> = vec![
            "[2a01:4f8:c011:f68::1]:443".parse()?,
            "142.132.241.153:443".parse()?,
            "[2a01:4f8:c011:f68::2]:443".parse()?,
            "142.132.241.154:443".parse()?,
        ];
        ipv4_first(&mut addrs);
        let shown: Vec<String> = addrs.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "142.132.241.153:443",
                "142.132.241.154:443",
                "[2a01:4f8:c011:f68::1]:443",
                "[2a01:4f8:c011:f68::2]:443",
            ]
        );
        // IPv6 alone is kept: it is the fallback when a host has no IPv4 address.
        let mut only_v6: Vec<SocketAddr> = vec!["[2a01:4f8:c011:f68::1]:443".parse()?];
        ipv4_first(&mut only_v6);
        assert_eq!(only_v6.len(), 1);
        Ok(())
    }

    fn genre(name: &str, count: Option<u32>) -> Genre {
        Genre {
            id: None,
            count,
            name: name.to_owned(),
            disambiguation: None,
        }
    }

    #[test]
    fn top_genre_is_highest_count_first_of_equals() {
        let genres = [
            genre("a", Some(1)),
            genre("b", Some(5)),
            genre("c", Some(5)),
            genre("d", None),
        ];
        assert_eq!(top_genre(Some(&genres)), Some("b".to_owned()));
        assert_eq!(top_genre(Some(&[])), None);
        assert_eq!(top_genre(None), None);
    }

    #[test]
    fn release_cache_hands_out_one_cell_per_release_id() {
        let cache = ReleaseCache::default();
        assert!(Arc::ptr_eq(&cache.cell("r"), &cache.cell("r")));
        assert!(!Arc::ptr_eq(&cache.cell("r"), &cache.cell("q")));
    }

    use crate::testkit::{TestResult, lib, make_audio, make_cover};
    use sdl_core::SongSource;

    fn wanted(source: SongSource) -> WantedSong {
        WantedSong {
            source,
            title: "Blue in Green".into(),
            artists: vec!["Miles Davis".into(), "Bill Evans".into()],
            album: Some("Kind of Blue".into()),
            album_artist: Some("Miles Davis".into()),
            track_number: Some(3),
            disc_number: Some(1),
            date: Some("1959-08-17".into()),
            isrc: None,
            duration: None,
            cover_url: None,
        }
    }

    fn spotify() -> WantedSong {
        let mut song = wanted(SongSource::Spotify {
            track_id: "trk".into(),
        });
        song.isrc = Some("USSM15900001".into());
        song
    }

    fn link() -> WantedSong {
        wanted(SongSource::Link {
            item_url: "https://music.youtube.com/watch?v=abc".into(),
            video_id: "abc".into(),
            direct: true,
        })
    }

    /// Serve `body` as `image/png` to the first request on a local port; returns its URL.
    fn serve_once(body: Vec<u8>) -> Result<String, Box<dyn std::error::Error>> {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/cover.png", listener.local_addr()?);
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Ok(url)
    }

    fn read_tag(file: &Path) -> Result<Tag, Box<dyn std::error::Error>> {
        let tagged = lofty::read_from_path(file)?;
        Ok(tagged.primary_tag().ok_or("no tag")?.clone())
    }

    #[tokio::test]
    async fn a_spotify_song_lands_with_its_own_tags_and_cover_and_no_genre() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("t.opus");
        make_audio(&file, 1)?;
        let mut song = spotify();
        song.cover_url = Some(serve_once(make_cover(&dir.path().join("c.png"))?)?);
        assert_eq!(write_tags(&file, &song).await?, Cover::Stored);
        let tag = read_tag(&file)?;
        assert_eq!(tag.title().as_deref(), Some("Blue in Green"));
        assert_eq!(
            tag.get_strings(ItemKey::TrackArtist).collect::<Vec<_>>(),
            ["Miles Davis", "Bill Evans"]
        );
        assert_eq!(tag.album().as_deref(), Some("Kind of Blue"));
        assert_eq!(tag.get_string(ItemKey::AlbumArtist), Some("Miles Davis"));
        assert_eq!((tag.track(), tag.disk()), (Some(3), Some(1)));
        assert_eq!(tag.get_string(ItemKey::Isrc), Some("USSM15900001"));
        assert!(
            tag.get_string(ItemKey::RecordingDate)
                .is_some_and(|d| d.starts_with("1959"))
        );
        assert_eq!(
            tag.get_picture_type(PictureType::CoverFront)
                .map(|p| p.data().is_empty()),
            Some(false)
        );
        // What MusicBrainz supplies comes later.
        assert_eq!(
            (tag.genre(), tag.track_total(), tag.disk_total()),
            (None, None, None)
        );
        Ok(())
    }

    /// The pictures of a file as (type, size), to tell that a cover is untouched. An M4A cover
    /// has no type and reads as `Other`.
    fn pictures(file: &Path) -> Result<Vec<(PictureType, usize)>, Box<dyn std::error::Error>> {
        Ok(read_tag(file)?
            .pictures()
            .iter()
            .map(|picture| (picture.pic_type(), picture.data().len()))
            .collect())
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Result<Vec<String>, std::io::Error> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            found.push(entry?.file_name().to_string_lossy().into_owned());
        }
        found.sort();
        Ok(found)
    }

    #[tokio::test]
    async fn genre_and_totals_are_added_to_a_landed_song_and_nothing_else_changes() -> TestResult {
        for ext in ["opus", "m4a", "mp3"] {
            let dir = tempfile::tempdir()?;
            let file = dir.path().join(format!("t.{ext}"));
            make_audio(&file, 1)?;
            let mut song = spotify();
            song.cover_url = Some(serve_once(make_cover(&dir.path().join("c.png"))?)?);
            assert_eq!(write_tags(&file, &song).await?, Cover::Stored, "{ext}");
            let cover_before = pictures(&file)?;
            let stream_before = crate::testkit::probe(&file)?;
            let found = Enrichment {
                genre: Some("Jazz".into()),
                track_total: Some(5),
                disc_total: Some(1),
            };
            add_enrichment(&file, &found)?;
            let tag = read_tag(&file)?;
            assert_eq!(tag.genre().as_deref(), Some("Jazz"), "{ext}");
            assert_eq!(
                (tag.track(), tag.track_total()),
                (Some(3), Some(5)),
                "{ext}"
            );
            assert_eq!((tag.disk(), tag.disk_total()), (Some(1), Some(1)), "{ext}");
            assert_eq!(tag.title().as_deref(), Some("Blue in Green"), "{ext}");
            assert_eq!(tag.album().as_deref(), Some("Kind of Blue"), "{ext}");
            assert_eq!(tag.get_string(ItemKey::Isrc), Some("USSM15900001"), "{ext}");
            assert_eq!(tag.get_strings(ItemKey::TrackArtist).count(), 2, "{ext}");
            assert_eq!(
                pictures(&file)?,
                cover_before,
                "{ext}: the cover is untouched"
            );
            assert_eq!(cover_before.len(), 1, "{ext}");
            let stream_after = crate::testkit::probe(&file)?;
            for key in ["codec_name", "sample_rate", "channels"] {
                assert_eq!(
                    stream_before.get(key),
                    stream_after.get(key),
                    "{ext}: {key}"
                );
            }
            assert_eq!(
                names(dir.path())?,
                ["c.png".to_owned(), format!("t.{ext}")],
                "{ext}: no temp file is left"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn an_empty_answer_leaves_the_file_as_it_is_and_a_missing_value_its_tag() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("t.opus");
        make_audio(&file, 1)?;
        write_tags(&file, &spotify()).await?;
        assert!(!has_genre(&file)?, "a landed song has no genre yet");
        let bytes = std::fs::read(&file)?;
        add_enrichment(&file, &Enrichment::default())?;
        assert_eq!(std::fs::read(&file)?, bytes, "nothing found: not rewritten");
        let only_genre = Enrichment {
            genre: Some("Jazz".into()),
            ..Enrichment::default()
        };
        add_enrichment(&file, &only_genre)?;
        let tag = read_tag(&file)?;
        assert_eq!(tag.genre().as_deref(), Some("Jazz"));
        assert_eq!((tag.track_total(), tag.disk_total()), (None, None));
        assert!(has_genre(&file)?);
        // A file that is not there is an error naming it, not a new file.
        let gone = dir.path().join("gone.opus");
        assert!(matches!(
            add_enrichment(&gone, &only_genre),
            Err(LibraryError::Tag { .. })
        ));
        assert!(!gone.exists());
        Ok(())
    }

    #[tokio::test]
    async fn a_link_song_is_tagged_from_its_own_fields_with_no_lookup() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("t.opus");
        make_audio(&file, 1)?;
        let song = link();
        // No ISRC: nothing is asked of MusicBrainz, so nothing can fail and nothing is found.
        let found = lookup(&song, &ReleaseCache::default()).await?;
        assert_eq!(found, Enrichment::default());
        assert_eq!(write_tags(&file, &song).await?, Cover::Missing);
        let tag = read_tag(&file)?;
        assert_eq!(tag.title().as_deref(), Some("Blue in Green"));
        assert_eq!(tag.album().as_deref(), Some("Kind of Blue"));
        assert_eq!(tag.genre(), None);
        assert_eq!(tag.get_string(ItemKey::Isrc), None);
        assert_eq!(tag.track_total(), None);
        assert!(tag.get_picture_type(PictureType::CoverFront).is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_cover_download_leaves_the_song_without_a_cover_but_tagged() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("t.opus");
        make_audio(&file, 1)?;
        let mut song = link();
        song.cover_url = Some("http://127.0.0.1:1/none.png".into());
        let cover = write_tags(&file, &song).await?;
        assert_eq!(cover, Cover::Missing);
        let tag = read_tag(&file)?;
        assert_eq!(tag.title().as_deref(), Some("Blue in Green"));
        assert_eq!(tag.album().as_deref(), Some("Kind of Blue"));
        assert!(tag.get_picture_type(PictureType::CoverFront).is_none());
        Ok(())
    }

    #[tokio::test]
    async fn an_image_the_tag_library_cannot_store_leaves_the_song_without_a_cover() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("t.opus");
        make_audio(&file, 1)?;
        let mut song = link();
        song.cover_url = Some(serve_once(b"not an image".to_vec())?);
        let cover = write_tags(&file, &song).await?;
        assert_eq!(cover, Cover::Missing);
        assert_eq!(read_tag(&file)?.title().as_deref(), Some("Blue in Green"));
        Ok(())
    }

    #[test]
    fn a_tagged_file_belongs_at_album_artist_album_title() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("lib");
        let file = dir.path().join("x.opus");
        make_audio(&file, 1)?;
        let mut song = link();
        song.album_artist = None;
        let tagged = lofty::read_from_path(&file)?;
        let mut tag = Tag::new(tagged.primary_tag_type());
        fill_tag(&mut tag, &song);
        tag.save_to_path(&file, WriteOptions::default())?;
        let place = place_from_tags(&file, &lib(&root))?.ok_or("no place")?;
        // No album artist tag: the first artist.
        assert_eq!(place, Place::of(&lib(&root), &song));
        assert_eq!(
            place.path(sdl_core::Format::Opus),
            root.join("Miles Davis/Kind of Blue/Blue in Green.opus")
        );
        Ok(())
    }

    #[test]
    fn a_file_without_title_or_artist_has_no_place() -> TestResult {
        let dir = tempfile::tempdir()?;
        let bare = dir.path().join("bare.opus");
        make_audio(&bare, 1)?;
        assert_eq!(place_from_tags(&bare, &lib(dir.path()))?, None);
        let half = dir.path().join("half.opus");
        make_audio(&half, 1)?;
        let mut tag = Tag::new(lofty::read_from_path(&half)?.primary_tag_type());
        tag.set_title("Only a title".to_owned());
        tag.save_to_path(&half, WriteOptions::default())?;
        assert_eq!(place_from_tags(&half, &lib(dir.path()))?, None);
        let missing = place_from_tags(&dir.path().join("gone.opus"), &lib(dir.path()));
        assert!(matches!(missing, Err(LibraryError::Tag { .. })));
        Ok(())
    }
}
