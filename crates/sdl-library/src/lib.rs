//! Genre and disc-total lookup (`musicbrainz_rs`), tag writing (`lofty`) and Snowsky card sync.

use std::fs::File;
use std::io::{Cursor, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use musicbrainz_rs::client::MusicBrainzClient;
use musicbrainz_rs::entity::recording::{Recording, RecordingSearchQuery};
use musicbrainz_rs::prelude::*;
use sdl_core::paths::card_music_dir;
use sdl_core::{Event, EventSender, Track, emit};
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
///
/// There is no disc total: a recording lookup lists, per release, only the medium that holds the
/// recording, so the release's medium count is not in the response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enrichment {
    /// Genre.
    pub genre: Option<String>,
    /// Total tracks on the song's disc (`rspotify`'s album model carries no track count).
    pub track_total: Option<u32>,
}

/// What a card sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Files copied (missing on the card or different size).
    pub copied: usize,
    /// Files already on the card.
    pub skipped: usize,
}

/// Look up genre and track total by ISRC (recording search, then lookup with genres,
/// releases and media). Uses one process-wide `MusicBrainzClient` with a generic User-Agent
/// (`sdl/<version>`), so its `rate_limit` limiter is shared by every concurrent song.
/// Missing data returns an empty [`Enrichment`], never an error.
///
/// # Errors
/// Only when the lookup itself cannot run; the pipeline treats this as empty data.
pub async fn lookup(track: &Track) -> Result<Enrichment, LibraryError> {
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
        .with_releases()
        .with_medias()
        .execute_with_client_async(client)
        .await
        .map_err(|e| LibraryError::MusicBrainz(e.to_string()))?;
    Ok(enrichment_from(&recording, track))
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

/// Mount the card by volume UUID (`diskutil mount <uuid>`). Returns its mount point.
///
/// # Errors
/// `diskutil` failed.
pub fn mount_card(uuid: &str) -> Result<PathBuf, LibraryError> {
    run_diskutil("mount", &["mount", uuid])?;
    let info = run_diskutil("info", &["info", uuid])?;
    parse_mount_point(&info).ok_or_else(|| LibraryError::Diskutil {
        action: "info",
        message: "no mount point reported".to_owned(),
    })
}

/// Copy `<music_root>` to `<card>/Music/` ([`sdl_core::paths::card_music_dir`]), add-only: copy a file when it is missing on the card or its
/// size differs, never delete. Blocking; run it inside `spawn_blocking`. Sends
/// [`sdl_core::Event::Progress`] on `events`.
///
/// # Errors
/// A read, write or walk failure.
pub fn sync_card(
    music_root: &Path,
    card_mount: &Path,
    events: &EventSender,
) -> Result<SyncReport, LibraryError> {
    let dest_root = card_music_dir(card_mount);
    let files = list_files(music_root)?;
    let total = files.len();
    let mut report = SyncReport::default();
    for (index, (src, rel)) in files.iter().enumerate() {
        let dest = dest_root.join(rel);
        if needs_copy(src, &dest)? {
            copy_file(src, &dest)?;
            report.copied = report.copied.saturating_add(1);
        } else {
            report.skipped = report.skipped.saturating_add(1);
        }
        emit(
            events,
            Event::Progress {
                done: index.saturating_add(1),
                total,
            },
        );
    }
    Ok(report)
}

/// Remove `.Spotlight-V100`, `.Trashes` and `.fseventsd` from the card root.
///
/// # Errors
/// A removal failed.
pub fn clean_dot_folders(card_mount: &Path) -> Result<(), LibraryError> {
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

/// Unmount the card by volume UUID (`diskutil unmount <uuid>`). Call only after the user confirms.
///
/// # Errors
/// `diskutil` failed.
pub fn unmount_card(uuid: &str) -> Result<(), LibraryError> {
    run_diskutil("unmount", &["unmount", uuid]).map(|_| ())
}

/// The one process-wide client: the rate limiter lives inside it.
fn mb_client() -> &'static MusicBrainzClient {
    static CLIENT: OnceLock<MusicBrainzClient> = OnceLock::new();
    CLIENT.get_or_init(|| MusicBrainzClient::new(concat!("sdl/", env!("CARGO_PKG_VERSION"))))
}

/// Pick genre and track total out of a looked-up recording. The track total is the medium at the
/// song's disc number on a release whose title matches the album, so a different edition never
/// supplies a wrong count.
fn enrichment_from(recording: &Recording, track: &Track) -> Enrichment {
    let genre = recording
        .genres
        .iter()
        .flatten()
        .max_by_key(|g| g.count.unwrap_or(0))
        .map(|g| g.name.clone());
    let track_total = recording
        .releases
        .iter()
        .flatten()
        .filter(|r| r.title.eq_ignore_ascii_case(&track.album))
        .flat_map(|r| r.media.iter().flatten())
        .find(|m| m.position == Some(track.disc_number))
        .map(|m| m.track_count);
    Enrichment { genre, track_total }
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
fn run_diskutil(action: &'static str, args: &[&str]) -> Result<String, LibraryError> {
    let out = Command::new("diskutil")
        .args(args)
        .output()
        .map_err(|e| LibraryError::Diskutil {
            action,
            message: e.to_string(),
        })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(LibraryError::Diskutil {
            action,
            message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
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

fn copy_file(src: &Path, dest: &Path) -> Result<(), LibraryError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let mut from = File::open(src).map_err(io_err(src))?;
    let mut to = File::create(dest).map_err(io_err(dest))?;
    std::io::copy(&mut from, &mut to).map_err(io_err(dest))?;
    Ok(())
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

    #[test]
    fn copies_missing_and_changed_skips_same_never_deletes() -> TestResult {
        let music = tempfile::tempdir()?;
        let card = tempfile::tempdir()?;
        let dest = card_music_dir(card.path());
        write(&music.path().join("Spotify/L/new.opus"), b"new")?;
        write(&music.path().join("Spotify/L/same.opus"), b"same")?;
        write(&music.path().join("Spotify/L/diff.opus"), b"longer")?;
        write(&dest.join("Spotify/L/same.opus"), b"SAME")?;
        write(&dest.join("Spotify/L/diff.opus"), b"x")?;
        write(&dest.join("Spotify/L/extra.opus"), b"keep")?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let report = sync_card(music.path(), card.path(), &tx)?;

        assert_eq!(
            report,
            SyncReport {
                copied: 2,
                skipped: 1
            }
        );
        assert_eq!(std::fs::read(dest.join("Spotify/L/new.opus"))?, b"new");
        assert_eq!(std::fs::read(dest.join("Spotify/L/diff.opus"))?, b"longer");
        assert_eq!(std::fs::read(dest.join("Spotify/L/same.opus"))?, b"SAME");
        assert_eq!(std::fs::read(dest.join("Spotify/L/extra.opus"))?, b"keep");
        let mut last = None;
        while let Ok(ev) = rx.try_recv() {
            last = Some(ev);
        }
        assert_eq!(last, Some(Event::Progress { done: 3, total: 3 }));
        Ok(())
    }

    #[test]
    fn never_creates_dot_components() -> TestResult {
        let music = tempfile::tempdir()?;
        let card = tempfile::tempdir()?;
        write(&music.path().join("Spotify/L/a.opus"), b"a")?;
        write(&music.path().join(".DS_Store"), b"x")?;
        write(&music.path().join(".hidden/b.opus"), b"b")?;
        write(&music.path().join("Spotify/.cache/c.opus"), b"c")?;

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let report = sync_card(music.path(), card.path(), &tx)?;

        assert_eq!(report.copied, 1);
        assert!(!has_dot_component(&card_music_dir(card.path()))?);
        Ok(())
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
