//! Path rules: FAT-safe components with leading dots stripped, and the library path builders.

use std::path::{Path, PathBuf};

use sanitize_filename::{Options, sanitize_with_options};

use crate::Track;

/// List name used for Liked Songs.
pub const SAVED_TRACKS: &str = "Saved tracks";

/// Longest stem that still fits the 255-byte name limit once `.opus` is appended.
const STEM_MAX: usize = 250;

/// Make one path component FAT-safe and strip leading dots, so no dot-directory can be created.
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

/// A FAT-safe file stem ([`component`]) cut to fit the 255-byte name limit once `.opus` is
/// appended.
#[must_use]
pub fn file_stem(name: &str) -> String {
    let mut stem = component(name);
    if let Some(end) = (0..=STEM_MAX).rev().find(|&i| stem.is_char_boundary(i)) {
        stem.truncate(end);
    }
    stem
}

/// `<music_root>/Spotify/<list name>/<artists> - <title>.opus`
#[must_use]
pub fn spotify_track_path(music_root: &Path, track: &Track) -> PathBuf {
    let stem = file_stem(&format!("{} - {}", track.artists_joined(), track.title));
    music_root
        .join("Spotify")
        .join(component(&track.list_name))
        .join(format!("{stem}.opus"))
}

/// `<music_root>/<Site>/`, plus a subfolder named after the playlist for a playlist link.
/// `site` is yt-dlp's extractor name.
#[must_use]
pub fn link_dir(music_root: &Path, site: &str, playlist: Option<&str>) -> PathBuf {
    let dir = music_root.join(component(site));
    match playlist {
        Some(name) => dir.join(component(name)),
        None => dir,
    }
}

/// `<card>/Music/`, the mirror of the music root on the card.
#[must_use]
pub fn card_music_dir(card_mount: &Path) -> PathBuf {
    card_mount.join("Music")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
            spotify_id: String::new(),
            list_name: list.into(),
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
    fn spotify_path_layout() {
        let p = spotify_track_path(Path::new("/m"), &track(SAVED_TRACKS));
        assert_eq!(
            p,
            PathBuf::from("/m/Spotify/Saved tracks/A, B - WhyNot.opus")
        );
    }

    #[test]
    fn spotify_path_list_name_is_safe() {
        let p = spotify_track_path(Path::new("/m"), &track(".mix/1"));
        assert_eq!(p, PathBuf::from("/m/Spotify/mix1/A, B - WhyNot.opus"));
    }

    #[test]
    fn spotify_file_name_fits_255_bytes() {
        let mut t = track(SAVED_TRACKS);
        t.title = "é".repeat(200);
        let p = spotify_track_path(Path::new("/m"), &t);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        assert!(name.len() <= 255, "{}", name.len());
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("opus"));
    }

    #[test]
    fn link_dir_layout() {
        let root = Path::new("/m");
        assert_eq!(link_dir(root, "Youtube", None), PathBuf::from("/m/Youtube"));
        assert_eq!(
            link_dir(root, "Soundcloud", Some("My: List")),
            PathBuf::from("/m/Soundcloud/My List")
        );
    }

    #[test]
    fn card_dir() {
        assert_eq!(
            card_music_dir(Path::new("/Volumes/X")),
            PathBuf::from("/Volumes/X/Music")
        );
    }
}
