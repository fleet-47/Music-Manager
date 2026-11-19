//! Genre and disc-total lookup (`musicbrainz_rs`), tag writing (`lofty`) and Snowsky card sync.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Cursor, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tokio::process::Command;
use tokio::sync::OnceCell;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use musicbrainz_rs::client::MusicBrainzClient;
use musicbrainz_rs::entity::genre::Genre;
use musicbrainz_rs::entity::recording::{Recording, RecordingSearchQuery};
use musicbrainz_rs::entity::release::Release;
use musicbrainz_rs::prelude::*;
use sdl_core::paths::card_music_dir;
use sdl_core::{Counts, EventKind, EventSender, Track};
use walkdir::WalkDir;

/// Dot-folders macOS creates at the card root.
const DOT_FOLDERS: [&str; 3] = [".Spotlight-V100", ".Trashes", ".fseventsd"];

/// Errors from the library crate.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// `MusicBrainz` request failed.
    #[error("MusicBrainz lookup failed: {0}")]
    MusicBrainz(String),
    /// Cover download failed.
    #[error("cover download failed: {0}")]
    Cover(#[from] reqwest::Error),
    /// Tag read or write failed.
    #[error("tag error on {path}: {message}")]
    Tag {
        /// File concerned.
        path: PathBuf,
        /// Reason.
        message: String,
    },
    /// File system failure.
    #[error("{path}: {source}")]
    Io {
        /// Path concerned.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// Directory walk failure.
    #[error("walk failed: {0}")]
    Walk(#[from] walkdir::Error),
    /// A background task failed to run to completion.
    #[error("background task failed: {0}")]
    Task(String),
    /// `diskutil` could not be run or reported failure.
    #[error("diskutil {action} failed: {message}")]
    Diskutil {
        /// Sub-command.
        action: &'static str,
        /// Reason.
        message: String,
    },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> LibraryError {
    let path = path.to_path_buf();
    move |source| LibraryError::Io { path, source }
}

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

/// A mounted card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Where the card is mounted.
    pub mount: PathBuf,
}

/// One file to copy to the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyItem {
    /// Source in the music root.
    pub src: PathBuf,
    /// Destination under `<card>/Music/`.
    pub dest: PathBuf,
}

/// What a card sync would do: `N files to copy, M already on the card`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyPlan {
    /// Files missing on the card or of a different size.
    pub to_copy: Vec<CopyItem>,
    /// Files already on the card.
    pub already: usize,
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
/// A `MusicBrainz` request failed; the pipeline treats this as empty data.
pub async fn lookup(track: &Track, cache: &ReleaseCache) -> Result<Enrichment, LibraryError> {
    let Some(isrc) = track.isrc.as_deref() else {
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
    let matched = matching_release(&recording, track);
    let release = match matched {
        Some((release, _)) => {
            let cell = cache.cell(&release.id);
            let info = cell.get_or_try_init(|| release_info(&release.id)).await?;
            Some(info.clone())
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
fn matching_release<'a>(recording: &'a Recording, track: &Track) -> Option<(&'a Release, u32)> {
    recording
        .releases
        .iter()
        .flatten()
        .filter(|release| release.title.eq_ignore_ascii_case(&track.album))
        .find_map(|release| {
            let medium = release
                .media
                .iter()
                .flatten()
                .find(|medium| medium.position == Some(track.disc_number))?;
            Some((release, medium.track_count))
        })
}

/// Write all tags and the cover (downloaded from `track.cover_url`) to `file` in one write.
///
/// # Errors
/// The file cannot be read or written, or the cover download failed.
pub async fn write_tags(
    file: &Path,
    track: &Track,
    enrichment: &Enrichment,
) -> Result<(), LibraryError> {
    let cover = match &track.cover_url {
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
    fill_tag(tag, track, enrichment);
    if let Some(bytes) = cover {
        let mut picture =
            Picture::from_reader(&mut Cursor::new(bytes)).map_err(|e| tag_err(e.to_string()))?;
        picture.set_pic_type(PictureType::CoverFront);
        tag.set_picture(0, picture);
    }
    tag.save_to_path(file, WriteOptions::default())
        .map_err(|e| tag_err(e.to_string()))
}

/// Mount the card by volume UUID (`diskutil mount <uuid>`, args passed separately).
///
/// # Errors
/// `diskutil` failed.
pub async fn mount_card(uuid: &str) -> Result<Card, LibraryError> {
    run_diskutil("mount", &["mount", uuid]).await?;
    let info = run_diskutil("info", &["info", uuid]).await?;
    let mount = parse_mount_point(&info).ok_or_else(|| LibraryError::Diskutil {
        action: "info",
        message: "no mount point reported".to_owned(),
    })?;
    Ok(Card { mount })
}

/// Plan the copy of `<music_root>` to `<card>/Music/` ([`sdl_core::paths::card_music_dir`]): a file
/// is to be copied when it is missing there or different in size. Files and directories whose name
/// starts with a dot are never included.
///
/// # Errors
/// A read or walk failure.
pub async fn plan_copy(music_root: &Path, card: &Card) -> Result<CopyPlan, LibraryError> {
    let music_root = music_root.to_path_buf();
    let dest_root = card_music_dir(&card.mount);
    tokio::task::spawn_blocking(move || {
        let mut plan = CopyPlan::default();
        for (src, rel) in list_files(&music_root)? {
            let dest = dest_root.join(rel);
            if needs_copy(&src, &dest)? {
                plan.to_copy.push(CopyItem { src, dest });
            } else {
                plan.already = plan.already.saturating_add(1);
            }
        }
        Ok(plan)
    })
    .await
    .map_err(|e| LibraryError::Task(e.to_string()))?
}

/// Copy the planned files, add-only, one at a time: `File::create` + `io::copy` to `<name>.part`,
/// renamed when complete. Each file is copied in `spawn_blocking`, so aborting the task finishes the
/// file in progress and then stops. Sends [`EventKind::Counts`] after each file (`total` is the
/// plan's file count, `done` and `added` the files copied).
///
/// # Errors
/// A read or write failure.
pub async fn copy_files(plan: &CopyPlan, events: &EventSender) -> Result<(), LibraryError> {
    let total = plan.to_copy.len();
    let mut counts = Counts {
        total,
        ..Counts::default()
    };
    events.send(EventKind::Counts(counts));
    for item in &plan.to_copy {
        let item = item.clone();
        tokio::task::spawn_blocking(move || copy_file(&item.src, &item.dest))
            .await
            .map_err(|e| LibraryError::Task(e.to_string()))??;
        counts.done = counts.done.saturating_add(1);
        counts.added = counts.added.saturating_add(1);
        events.send(EventKind::Counts(counts));
    }
    Ok(())
}

/// Remove `.Spotlight-V100`, `.Trashes` and `.fseventsd` from the card root, then
/// `diskutil unmount <uuid>`. Call only after the owner confirms.
///
/// # Errors
/// A removal or `diskutil` failure.
pub async fn finish_card(card: &Card, uuid: &str) -> Result<(), LibraryError> {
    let mount = card.mount.clone();
    tokio::task::spawn_blocking(move || clean_dot_folders(&mount))
        .await
        .map_err(|e| LibraryError::Task(e.to_string()))??;
    run_diskutil("unmount", &["unmount", uuid])
        .await
        .map(|_| ())
}

/// Remove the three macOS dot-folders from the card root.
fn clean_dot_folders(card_mount: &Path) -> Result<(), LibraryError> {
    for name in DOT_FOLDERS {
        let path = card_mount.join(name);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&path)(e)),
        }
    }
    Ok(())
}

/// The one process-wide client: the rate limiter lives inside it.
fn mb_client() -> &'static MusicBrainzClient {
    static CLIENT: OnceLock<MusicBrainzClient> = OnceLock::new();
    CLIENT.get_or_init(|| MusicBrainzClient::new(concat!("sdl/", env!("CARGO_PKG_VERSION"))))
}

/// Set every spec field on `tag`; empty enrichment values leave their tag unset.
fn fill_tag(tag: &mut Tag, track: &Track, enrichment: &Enrichment) {
    tag.set_title(track.title.clone());
    tag.set_album(track.album.clone());
    tag.remove_artist();
    for artist in &track.artists {
        tag.push(lofty::tag::TagItem::new(
            ItemKey::TrackArtist,
            lofty::tag::ItemValue::Text(artist.clone()),
        ));
    }
    tag.insert_text(ItemKey::AlbumArtist, track.album_artist.clone());
    tag.set_track(track.track_number);
    tag.set_disk(track.disc_number);
    if let Some(n) = enrichment.disc_total {
        tag.set_disk_total(n);
    }
    if let Some(n) = enrichment.track_total {
        tag.set_track_total(n);
    }
    if let Some(date) = &track.release_date {
        tag.insert_text(ItemKey::RecordingDate, date.clone());
    }
    if let Some(isrc) = &track.isrc {
        tag.insert_text(ItemKey::Isrc, isrc.clone());
    }
    if let Some(genre) = &enrichment.genre {
        tag.set_genre(genre.clone());
    }
}

/// Run `diskutil` with separate arguments (no shell) and return stdout.
async fn run_diskutil(action: &'static str, args: &[&str]) -> Result<String, LibraryError> {
    let out = Command::new("diskutil")
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| LibraryError::Diskutil {
            action,
            message: e.to_string(),
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    // diskutil prints some failures on stdout only.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let message = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|text| !text.is_empty())
        .map_or_else(|| out.status.to_string(), str::to_owned);
    Err(LibraryError::Diskutil { action, message })
}

/// The `Mount Point:` value from `diskutil info` output.
fn parse_mount_point(info: &str) -> Option<PathBuf> {
    info.lines()
        .find_map(|line| line.trim().strip_prefix("Mount Point:"))
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// Files under `root` (source path, path relative to root), skipping every entry whose name starts
/// with a dot, so no dot-named path can be created on the card.
fn list_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>, LibraryError> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            files.push((entry.path().to_path_buf(), rel.to_path_buf()));
        }
    }
    Ok(files)
}

/// Copy rule: the destination is missing or its size differs.
fn needs_copy(src: &Path, dest: &Path) -> Result<bool, LibraryError> {
    match std::fs::metadata(dest) {
        Ok(meta) => {
            let src_len = std::fs::metadata(src).map_err(io_err(src))?.len();
            Ok(meta.len() != src_len)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(true),
        Err(e) => Err(io_err(dest)(e)),
    }
}

/// Copy `src` to `dest` through `<dest>.part`, renamed when complete.
fn copy_file(src: &Path, dest: &Path) -> Result<(), LibraryError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let mut part_name: OsString = dest.as_os_str().to_owned();
    part_name.push(".part");
    let part = PathBuf::from(part_name);
    let mut from = File::open(src).map_err(io_err(src))?;
    let mut to = File::create(&part).map_err(io_err(&part))?;
    std::io::copy(&mut from, &mut to).map_err(io_err(&part))?;
    std::fs::rename(&part, dest).map_err(io_err(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl_core::paths::card_music_dir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(path: &Path, bytes: &[u8]) -> TestResult {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn has_dot_component(root: &Path) -> Result<bool, walkdir::Error> {
        for entry in WalkDir::new(root).min_depth(1) {
            if entry?.file_name().to_string_lossy().starts_with('.') {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[tokio::test]
    async fn copies_missing_and_changed_skips_same_never_deletes() -> TestResult {
        let music = tempfile::tempdir()?;
        let card_dir = tempfile::tempdir()?;
        let dest = card_music_dir(card_dir.path());
        write(&music.path().join("Spotify/L/new.opus"), b"new")?;
        write(&music.path().join("Spotify/L/same.opus"), b"same")?;
        write(&music.path().join("Spotify/L/diff.opus"), b"longer")?;
        write(&dest.join("Spotify/L/same.opus"), b"SAME")?;
        write(&dest.join("Spotify/L/diff.opus"), b"x")?;
        write(&dest.join("Spotify/L/extra.opus"), b"keep")?;
        let card = Card {
            mount: card_dir.path().to_path_buf(),
        };

        let plan = plan_copy(music.path(), &card).await?;
        assert_eq!(plan.to_copy.len(), 2);
        assert_eq!(plan.already, 1);

        let (tx, mut rx) = sdl_core::channel();
        let events = EventSender::new(sdl_core::TaskId::SyncCard, tx);
        copy_files(&plan, &events).await?;

        assert_eq!(std::fs::read(dest.join("Spotify/L/new.opus"))?, b"new");
        assert_eq!(std::fs::read(dest.join("Spotify/L/diff.opus"))?, b"longer");
        assert_eq!(std::fs::read(dest.join("Spotify/L/same.opus"))?, b"SAME");
        assert_eq!(std::fs::read(dest.join("Spotify/L/extra.opus"))?, b"keep");
        assert!(!dest.join("Spotify/L/new.opus.part").exists());
        let mut last = None;
        while let Ok(ev) = rx.try_recv() {
            last = Some(ev.kind);
        }
        assert_eq!(
            last,
            Some(EventKind::Counts(Counts {
                total: 2,
                done: 2,
                added: 2,
                ..Counts::default()
            }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn never_plans_dot_components() -> TestResult {
        let music = tempfile::tempdir()?;
        let card_dir = tempfile::tempdir()?;
        write(&music.path().join("Spotify/L/a.opus"), b"a")?;
        write(&music.path().join(".DS_Store"), b"x")?;
        write(&music.path().join(".hidden/b.opus"), b"b")?;
        write(&music.path().join(".sdl-tmp/c.opus"), b"c")?;
        write(&music.path().join("Spotify/.cache/c.opus"), b"c")?;
        let card = Card {
            mount: card_dir.path().to_path_buf(),
        };

        let plan = plan_copy(music.path(), &card).await?;
        assert_eq!(plan.to_copy.len(), 1);
        copy_files(
            &plan,
            &EventSender::new(sdl_core::TaskId::SyncCard, sdl_core::channel().0),
        )
        .await?;
        assert!(!has_dot_component(&card_music_dir(card_dir.path()))?);
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

    #[test]
    fn cleans_only_the_three_dot_folders() -> TestResult {
        let card = tempfile::tempdir()?;
        for name in DOT_FOLDERS {
            write(&card.path().join(name).join("x"), b"x")?;
        }
        write(&card.path().join(".diskos/keep"), b"k")?;
        write(&card.path().join("Music/a.opus"), b"a")?;
        clean_dot_folders(card.path())?;
        for name in DOT_FOLDERS {
            assert!(!card.path().join(name).exists());
        }
        assert!(card.path().join(".diskos/keep").exists());
        assert!(card.path().join("Music/a.opus").exists());
        clean_dot_folders(card.path())?;
        Ok(())
    }

    #[test]
    fn parses_mount_point() {
        let info =
            "   Device Node:  /dev/disk4s1\n   Mount Point:               /Volumes/NO NAME\n";
        assert_eq!(
            parse_mount_point(info),
            Some(PathBuf::from("/Volumes/NO NAME"))
        );
        assert_eq!(parse_mount_point("   Mount Point:    \n"), None);
    }
}
