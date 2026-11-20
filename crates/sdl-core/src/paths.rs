//! Path rules. Every path component is FAT-safe, has leading dots stripped, is capped so a file
//! name fits 255 bytes, and falls back to the Spotify or video ID when it would be empty.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sanitize_filename::{Options, sanitize_with_options};

use crate::format::AUDIO_EXTENSIONS;
use crate::{Format, Track};

/// List name used for Liked Songs.
pub const SAVED_TRACKS: &str = "Saved tracks";

/// Name of the temp directory under the music root. A dot name, so device sync never copies it.
const TMP_DIR: &str = ".sdl-tmp";

/// Longest stem that still fits the 255-byte name limit once an extension (`.opus`, at most five
/// bytes) is appended.
const STEM_MAX: usize = 250;

/// A name and its ID, for a playlist whose ID is the fallback when the name is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameId<'a> {
    /// Display name.
    pub name: &'a str,
    /// ID used when the name is empty after sanitising.
    pub id: &'a str,
}

/// Make one path component FAT-safe (at most 255 bytes) and strip leading dots, so no
/// dot-directory can be created. May be empty; see [`component_or`].
#[must_use]
pub fn component(name: &str) -> String {
    let safe = sanitize_with_options(
        name,
        Options {
            windows: true,
            truncate: true,
            replacement: "",
        },
    );
    safe.trim_start_matches('.').to_owned()
}

/// [`component`] of `name`, or of `fallback_id` when that is empty.
#[must_use]
fn component_or(name: &str, fallback_id: &str) -> String {
    let safe = component(name);
    if safe.is_empty() {
        component(fallback_id)
    } else {
        safe
    }
}

/// A FAT-safe file stem ([`component`]) cut to fit the 255-byte name limit once `.opus` is
/// appended. May be empty; see [`file_stem_or`].
#[must_use]
fn file_stem(name: &str) -> String {
    let mut stem = component(name);
    if let Some(end) = (0..=STEM_MAX).rev().find(|&i| stem.is_char_boundary(i)) {
        stem.truncate(end);
    }
    stem
}

/// [`file_stem`] of `name`, or of `fallback_id` when that is empty.
#[must_use]
fn file_stem_or(name: &str, fallback_id: &str) -> String {
    let stem = file_stem(name);
    if stem.is_empty() {
        file_stem(fallback_id)
    } else {
        stem
    }
}

/// Where a song lives, apart from its format: a folder and a file stem. The one place that knows a
/// song is "present" when a file with its stem exists in the folder in any of the three formats.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    /// The folder.
    pub dir: PathBuf,
    /// The file name without extension.
    pub stem: String,
}

impl Target {
    /// The target of an existing or planned file: its folder and stem. `None` for a path with no
    /// folder or no stem.
    #[must_use]
    pub fn of_file(path: &Path) -> Option<Self> {
        Some(Self {
            dir: path.parent()?.to_path_buf(),
            stem: path.file_stem()?.to_str()?.to_owned(),
        })
    }

    /// `<dir>/<stem>.<extension of format>`.
    #[must_use]
    pub fn path(&self, format: Format) -> PathBuf {
        self.dir
            .join(format!("{}.{}", self.stem, format.extension()))
    }

    /// Whether a song with this stem exists in the folder in any format ([`AUDIO_EXTENSIONS`]).
    ///
    /// # Errors
    /// A folder that exists but cannot be read. A missing folder or file is `Ok(false)`.
    pub fn is_present(&self) -> std::io::Result<bool> {
        for ext in AUDIO_EXTENSIONS {
            if self.dir.join(format!("{}.{ext}", self.stem)).try_exists()? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// `<music_root>/Spotify/<list name>/<artists> - <title>`. The list component falls back to the
/// list ID, the file stem to the Spotify track ID.
#[must_use]
pub fn spotify_target(music_root: &Path, track: &Track) -> Target {
    let stem = file_stem_or(
        &format!("{} - {}", track.artists_joined(), track.title),
        &track.spotify_id,
    );
    Target {
        dir: music_root
            .join("Spotify")
            .join(component_or(&track.list_name, &track.list_id)),
        stem,
    }
}

/// `<music_root>/<Site>/<item>`, with a subfolder named after the playlist for a playlist link.
/// `site` is yt-dlp's extractor key. An empty name falls back to its ID; the video ID is the
/// fallback for an empty item stem and for an empty site.
#[must_use]
pub fn link_item_target(
    music_root: &Path,
    site: &str,
    playlist: Option<NameId<'_>>,
    title: &str,
    video_id: &str,
) -> Target {
    let mut dir = music_root.join(component_or(site, video_id));
    if let Some(list) = playlist {
        dir = dir.join(component_or(list.name, list.id));
    }
    Target {
        dir,
        stem: file_stem_or(title, video_id),
    }
}

/// The path of the temp file a download or conversion writes in `dir`: `audio.<extension>`.
#[must_use]
pub fn temp_file(dir: &Path, format: Format) -> PathBuf {
    dir.join(format!("audio.{}", format.extension()))
}

/// `<device>/Music/`, the mirror of the music root on a device.
#[must_use]
pub fn device_music_dir(device_mount: &Path) -> PathBuf {
    device_mount.join("Music")
}

/// `<music_root>/.sdl-tmp`, on the same volume as the library so the final rename is atomic.
#[must_use]
pub fn tmp_dir(music_root: &Path) -> PathBuf {
    music_root.join(TMP_DIR)
}

/// Remove [`tmp_dir`] when it is empty. A missing or non-empty directory is not an error.
///
/// # Errors
/// The directory could not be removed for another reason.
pub fn remove_tmp_dir_if_empty(music_root: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir(tmp_dir(music_root)) {
        Err(error)
            if !matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Err(error)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn track(list: &str) -> Track {
        Track {
            title: "Why?/Not".into(),
            artists: vec!["A".into(), "B".into()],
            album: String::new(),
            album_artist: String::new(),
            track_number: 1,
            disc_number: 1,
            release_date: None,
            isrc: None,
            duration: Duration::from_secs(1),
            cover_url: None,
            spotify_id: "sid".into(),
            list_name: list.into(),
            list_id: "lid".into(),
        }
    }

    #[test]
    fn component_strips_illegal_and_leading_dots() {
        assert_eq!(component("a:b*c"), "abc");
        assert_eq!(component(".Spotlight-V100"), "Spotlight-V100");
        assert_eq!(component("..hidden"), "hidden");
        assert_eq!(component("name. "), "name");
    }

    #[test]
    fn component_never_yields_a_dot_name() {
        assert_eq!(component("."), "");
        assert_eq!(component(".."), "");
    }

    #[test]
    fn component_is_capped_at_255_bytes() {
        assert!(component(&"é".repeat(300)).len() <= 255);
    }

    #[test]
    fn empty_names_fall_back_to_the_id() {
        assert_eq!(component_or("...", "abc123"), "abc123");
        assert_eq!(file_stem_or("???", "vid"), "vid");
        assert_eq!(component_or("ok", "abc123"), "ok");
    }

    #[test]
    fn spotify_target_layout() {
        let t = spotify_target(Path::new("/m"), &track(SAVED_TRACKS));
        assert_eq!(t.dir, PathBuf::from("/m/Spotify/Saved tracks"));
        assert_eq!(t.stem, "A, B - WhyNot");
        assert_eq!(
            t.path(Format::Opus),
            PathBuf::from("/m/Spotify/Saved tracks/A, B - WhyNot.opus")
        );
        assert_eq!(
            t.path(Format::Mp3),
            PathBuf::from("/m/Spotify/Saved tracks/A, B - WhyNot.mp3")
        );
        assert_eq!(
            t.path(Format::M4a),
            PathBuf::from("/m/Spotify/Saved tracks/A, B - WhyNot.m4a")
        );
    }

    #[test]
    fn spotify_list_name_is_safe() {
        let t = spotify_target(Path::new("/m"), &track(".mix/1"));
        assert_eq!(t.dir, PathBuf::from("/m/Spotify/mix1"));
    }

    #[test]
    fn spotify_target_falls_back_to_ids() {
        let mut t = track("...");
        t.title = "...".into();
        t.artists = Vec::new();
        // The stem is " - ..." before sanitising: not empty, so only the list falls back.
        let target = spotify_target(Path::new("/m"), &t);
        assert_eq!(target.dir, PathBuf::from("/m/Spotify/lid"));
        assert_eq!(file_stem_or("..", &t.spotify_id), "sid");
    }

    #[test]
    fn spotify_file_name_fits_255_bytes_in_every_format() {
        let mut t = track(SAVED_TRACKS);
        t.title = "é".repeat(200);
        let target = spotify_target(Path::new("/m"), &t);
        for format in [Format::Opus, Format::Mp3, Format::M4a] {
            let path = target.path(format);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            assert!(name.len() <= 255, "{}", name.len());
            assert_eq!(
                path.extension().and_then(|e| e.to_str()),
                Some(format.extension())
            );
        }
    }

    #[test]
    fn link_item_layout() {
        let root = Path::new("/m");
        assert_eq!(
            link_item_target(root, "Youtube", None, "Song: 1", "v1").path(Format::Mp3),
            PathBuf::from("/m/Youtube/Song 1.mp3")
        );
        let list = NameId {
            name: "My: List",
            id: "PL1",
        };
        assert_eq!(
            link_item_target(root, "Soundcloud", Some(list), "S", "v1").path(Format::Opus),
            PathBuf::from("/m/Soundcloud/My List/S.opus")
        );
    }

    #[test]
    fn link_item_falls_back_to_ids() {
        let list = NameId {
            name: "..",
            id: "PL1",
        };
        assert_eq!(
            link_item_target(Path::new("/m"), "Youtube", Some(list), "?", "v1").path(Format::Opus),
            PathBuf::from("/m/Youtube/PL1/v1.opus")
        );
    }

    #[test]
    fn a_song_is_present_in_any_format() -> TestResult {
        let dir = tempfile::tempdir()?;
        let target = Target {
            dir: dir.path().to_path_buf(),
            stem: "Song".into(),
        };
        assert!(!target.is_present()?);
        for format in [Format::Mp3, Format::M4a, Format::Opus] {
            let file = target.path(format);
            std::fs::write(&file, b"x")?;
            assert!(target.is_present()?, "{format:?}");
            std::fs::remove_file(&file)?;
        }
        std::fs::write(dir.path().join("Song.flac"), b"x")?;
        std::fs::write(dir.path().join("Song 2.mp3"), b"x")?;
        assert!(!target.is_present()?);
        Ok(())
    }

    #[test]
    fn a_missing_folder_is_not_present() -> TestResult {
        let dir = tempfile::tempdir()?;
        let target = Target {
            dir: dir.path().join("nope"),
            stem: "Song".into(),
        };
        assert!(!target.is_present()?);
        Ok(())
    }

    #[test]
    fn target_of_a_file_and_back() {
        let target = Target::of_file(Path::new("/d/Music/A/b c.mp3"));
        assert_eq!(
            target,
            Some(Target {
                dir: PathBuf::from("/d/Music/A"),
                stem: "b c".into()
            })
        );
        assert_eq!(Target::of_file(Path::new("/")), None);
    }

    #[test]
    fn temp_file_carries_the_extension() {
        assert_eq!(
            temp_file(Path::new("/t"), Format::M4a),
            PathBuf::from("/t/audio.m4a")
        );
    }

    #[test]
    fn device_dir() {
        assert_eq!(
            device_music_dir(Path::new("/Volumes/X")),
            PathBuf::from("/Volumes/X/Music")
        );
    }

    #[test]
    fn tmp_dir_is_removed_only_when_empty() -> TestResult {
        let root = tempfile::tempdir()?;
        remove_tmp_dir_if_empty(root.path())?; // missing: fine
        let file = tmp_dir(root.path()).join("x.opus");
        std::fs::create_dir_all(tmp_dir(root.path()))?;
        std::fs::write(&file, b"x")?;
        remove_tmp_dir_if_empty(root.path())?; // not empty: stays
        assert!(tmp_dir(root.path()).exists());
        std::fs::remove_file(&file)?;
        remove_tmp_dir_if_empty(root.path())?;
        assert!(!tmp_dir(root.path()).exists());
        Ok(())
    }
}
