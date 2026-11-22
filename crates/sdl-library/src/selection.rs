//! What a selection names (spec H3): the one place "Whole library and these playlists" becomes
//! files. The device plan (F10) and the Convert plan (F9) both ask here.
//!
//! Interface, in full:
//! * [`songs`] gives each song of `library` the selection names, once: the whole library first
//!   when ticked (`sdl_core::library_files`: every audio file at any depth, dot names skipped,
//!   links not followed, sorted by path), then each ticked playlist's songs,
//!   playlist by playlist in the order given, each in list order.
//! * A playlist is `<library folder>/<stem>.m3u8`; a missing file names nothing. A line counts
//!   when it is exactly three plain path components (`<artist>/<album>/<file>`), none starting
//!   with a dot, and a file with its name exists in the library in any format (the line's
//!   extension may be out of date: review can land a song with another one). Every other line,
//!   absolute or climbing out with `..` included, is left out without an error.
//! * "Once" is by path without extension, relative to the library's folder: the same folders and
//!   file stem are one song whatever lists name it.
//!
//! Plain blocking file calls; a plan calls it on a blocking thread.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use sdl_core::format::existing_format;
use sdl_core::playlist::PLAYLIST_EXTENSION;
use sdl_core::{Library, Playlist, Selection, library_files};

use crate::{LibraryError, io_err};

/// One song a selection names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    /// The file in the library, with the extension it really has.
    pub file: PathBuf,
    /// `file` relative to the library's folder: `<artist>/<album>/<file>` for a song the app
    /// placed, any depth for a file put there otherwise (F7).
    pub relative: PathBuf,
}

/// The songs of `library` that `what` names; see the module's interface.
///
/// # Errors
/// [`LibraryError::Io`] when a folder or a playlist file exists but cannot be read.
pub fn songs(library: &Library, what: &Selection) -> Result<Vec<Song>, LibraryError> {
    let root = library.root();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut found = Vec::new();
    let mut add = |file: PathBuf, relative: PathBuf| {
        if seen.insert(relative.with_extension("")) {
            found.push(Song { file, relative });
        }
    };
    if what.whole_library {
        for file in library_files(library).map_err(io_err(root))? {
            if let Ok(relative) = file.strip_prefix(root) {
                let relative = relative.to_path_buf();
                add(file, relative);
            }
        }
    }
    for stem in &what.playlists {
        let path = root.join(format!("{stem}.{PLAYLIST_EXTENSION}"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(io_err(&path)(error)),
        };
        for entry in Playlist::parse(&text, stem).entries {
            if !is_song_path(&entry) {
                continue;
            }
            let wanted = root.join(&entry);
            if let Some(format) = existing_format(&wanted).map_err(io_err(&wanted))? {
                let relative = entry.with_extension(format.extension());
                add(wanted.with_extension(format.extension()), relative);
            }
        }
    }
    Ok(found)
}

/// Whether a playlist line is `<artist>/<album>/<file>`: three plain components, none starting
/// with a dot.
fn is_song_path(entry: &Path) -> bool {
    entry.components().count() == 3
        && entry.components().all(|part| match part {
            Component::Normal(name) => !name.to_string_lossy().starts_with('.'),
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::testkit::{TestResult, lib};

    fn put(root: &Path, relative: &str) -> std::io::Result<()> {
        let path = root.join(relative);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(path, b"audio")
    }

    fn playlist(root: &Path, stem: &str, lines: &[&str]) -> std::io::Result<()> {
        let mut text = format!("#EXTM3U\n#PLAYLIST:{stem}\n");
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        std::fs::write(root.join(format!("{stem}.m3u8")), text)
    }

    fn relatives(found: &[Song]) -> Vec<String> {
        found
            .iter()
            .map(|song| song.relative.to_string_lossy().into_owned())
            .collect()
    }

    fn whole() -> Selection {
        Selection {
            whole_library: true,
            playlists: Vec::new(),
        }
    }

    fn lists(stems: &[&str]) -> Selection {
        Selection {
            whole_library: false,
            playlists: stems.iter().map(|stem| (*stem).to_owned()).collect(),
        }
    }

    #[test]
    fn the_whole_library_is_its_songs_at_any_depth_sorted() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        for file in [
            "B/Album/two.mp3",
            "A/Album/one.opus",
            "A/Album/three.m4a",
            "A/stray.opus",
            "A/Album/deep/four.opus",
            "Unsorted/Old/sub/untagged.mp3",
            // Not the library's: not audio, or a dot name.
            "A/Album/cover.jpg",
            ".sdl-tmp/x/y.opus",
            "A/.hidden/z.opus",
            "A/Album/.five.opus",
        ] {
            put(root, file)?;
        }
        let found = songs(&lib(root), &whole())?;
        assert_eq!(
            relatives(&found),
            [
                "A/Album/deep/four.opus",
                "A/Album/one.opus",
                "A/Album/three.m4a",
                "A/stray.opus",
                "B/Album/two.mp3",
                "Unsorted/Old/sub/untagged.mp3",
            ]
        );
        assert_eq!(
            found.first().map(|song| song.file.clone()),
            Some(root.join("A/Album/deep/four.opus"))
        );
        Ok(())
    }

    #[test]
    fn a_playlist_names_its_songs_in_list_order_in_whatever_format_they_have() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        put(root, "Z/Album/last.opus")?;
        // The line says `.opus`; review landed the song as M4A.
        put(root, "A/Album/first.m4a")?;
        put(root, "A/Album/unlisted.opus")?;
        playlist(
            root,
            "Mix",
            &[
                "Z/Album/last.opus",
                "A/Album/first.opus",
                "A/Album/not there yet.opus",
            ],
        )?;
        let found = songs(&lib(root), &lists(&["Mix", "No such list"]))?;
        assert_eq!(
            relatives(&found),
            ["Z/Album/last.opus", "A/Album/first.m4a"]
        );
        assert_eq!(
            found.get(1).map(|song| song.file.clone()),
            Some(root.join("A/Album/first.m4a"))
        );
        Ok(())
    }

    #[test]
    fn a_line_that_is_not_a_place_in_the_library_is_left_out() -> TestResult {
        let dir = tempfile::tempdir()?;
        let outside = dir.path().join("outside.opus");
        std::fs::write(&outside, b"audio")?;
        let root = dir.path().join("Me");
        put(&root, "A/Album/in.opus")?;
        put(&root, "A/loose.opus")?;
        put(&root, "A/.dot/hidden.opus")?;
        let absolute = outside.to_string_lossy().into_owned();
        playlist(
            &root,
            "Mix",
            &[
                absolute.as_str(),
                "../outside.opus",
                "A/Album/../../../outside.opus",
                "A/loose.opus",
                "A/.dot/hidden.opus",
                "A/Album/in.opus",
            ],
        )?;
        let found = songs(&lib(&root), &lists(&["Mix"]))?;
        assert_eq!(relatives(&found), ["A/Album/in.opus"]);
        Ok(())
    }

    #[test]
    fn a_song_named_more_than_once_is_given_once() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        put(root, "A/Album/one.opus")?;
        put(root, "B/Album/two.opus")?;
        playlist(root, "Mix", &["B/Album/two.opus", "B/Album/two.mp3"])?;
        playlist(root, "Other", &["A/Album/one.opus", "B/Album/two.opus"])?;
        // The whole library comes first; the playlists then add nothing new.
        let both = Selection {
            whole_library: true,
            playlists: vec!["Mix".to_owned(), "Other".to_owned()],
        };
        assert_eq!(
            relatives(&songs(&lib(root), &both)?),
            ["A/Album/one.opus", "B/Album/two.opus"]
        );
        // Without it: playlist by playlist, each song once.
        assert_eq!(
            relatives(&songs(&lib(root), &lists(&["Mix", "Other"]))?),
            ["B/Album/two.opus", "A/Album/one.opus"]
        );
        Ok(())
    }

    #[test]
    fn nothing_ticked_names_nothing() -> TestResult {
        let dir = tempfile::tempdir()?;
        put(dir.path(), "A/Album/one.opus")?;
        assert_eq!(songs(&lib(dir.path()), &Selection::default())?, Vec::new());
        Ok(())
    }
}
