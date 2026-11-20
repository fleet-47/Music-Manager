//! Genre, totals and tags (F6): `MusicBrainz` lookup and one `lofty` write to the temp file.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use musicbrainz_rs::client::MusicBrainzClient;
use musicbrainz_rs::entity::genre::Genre;
use musicbrainz_rs::entity::recording::{Recording, RecordingSearchQuery};
use musicbrainz_rs::entity::release::Release;
use musicbrainz_rs::prelude::*;
use sdl_core::{Place, WantedSong};
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
/// User-Agent (`sdl/<version>`), so its `rate_limit` limiter is shared by every concurrent song.
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

/// Write all tags and the cover (downloaded from `song.cover_url`) to `file` in one write.
///
/// # Errors
/// The file cannot be read or written, or the cover download failed.
pub async fn write_tags(
    file: &Path,
    song: &WantedSong,
    enrichment: &Enrichment,
) -> Result<(), LibraryError> {
    let cover = match &song.cover_url {
        Some(url) => Some(reqwest::get(url).await?.error_for_status()?.bytes().await?),
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
    fill_tag(tag, song, enrichment);
    if let Some(bytes) = cover {
        let mut picture =
            Picture::from_reader(&mut Cursor::new(bytes)).map_err(|e| tag_err(e.to_string()))?;
        picture.set_pic_type(PictureType::CoverFront);
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(picture);
    }
    tag.save_to_path(file, WriteOptions::default())
        .map_err(|e| tag_err(e.to_string()))
}

/// The one process-wide client: the rate limiter lives inside it.
fn mb_client() -> &'static MusicBrainzClient {
    static CLIENT: OnceLock<MusicBrainzClient> = OnceLock::new();
    CLIENT.get_or_init(|| MusicBrainzClient::new(concat!("sdl/", env!("CARGO_PKG_VERSION"))))
}

/// Set every spec field on `tag`; empty enrichment values leave their tag unset.
fn fill_tag(tag: &mut Tag, song: &WantedSong, enrichment: &Enrichment) {
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
    if let Some(n) = enrichment.disc_total {
        tag.set_disk_total(n);
    }
    if let Some(n) = enrichment.track_total {
        tag.set_track_total(n);
    }
    if let Some(date) = &song.date {
        tag.insert_text(ItemKey::RecordingDate, date.clone());
    }
    if let Some(isrc) = &song.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }
    if let Some(genre) = &enrichment.genre {
        tag.set_genre(genre.clone());
    }
}

/// Where a file that carries tags belongs in the library under `music_root` (F10, From device):
/// album artist (else first artist), album (else `Singles`) and title from its tags, by
/// [`Place::new`], with the file's stem as the ID fallback. `None` when the file has no title or
/// no artist tag; the caller then files it under `Unsorted` (F10).
///
/// # Errors
/// [`LibraryError::NotBuilt`] until built; later, a file that cannot be read.
pub fn place_from_tags(file: &Path, music_root: &Path) -> Result<Option<Place>, LibraryError> {
    let _ = (file, music_root);
    Err(LibraryError::NotBuilt("reading a file's tags to place it"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
