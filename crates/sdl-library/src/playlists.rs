//! Playlist files (F7): `<music_root>/<List name>.m3u8` pointer files. The file model (format,
//! parsing, line correction) is `sdl_core::Playlist`; this module reads and writes the files.
//!
//! Plain blocking file I/O on small files; call from async code directly.

use std::path::Path;

use sdl_core::playlist::PLAYLIST_EXTENSION;
use sdl_core::{Format, Place, Playlist, WantedSong};

use crate::{LibraryError, io_err};

/// The playlists in the library: the stems of the `.m3u8` files directly in `music_root`, sorted,
/// skipping names that start with a dot. A missing folder has none.
///
/// # Errors
/// [`LibraryError::Io`] when the folder cannot be read.
pub fn list(music_root: &Path) -> Result<Vec<String>, LibraryError> {
    let entries = match std::fs::read_dir(music_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_err(music_root)(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let path = entry.map_err(io_err(music_root))?.path();
        let is_playlist = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(PLAYLIST_EXTENSION));
        if let (true, Some(stem)) = (is_playlist, path.file_stem().and_then(|s| s.to_str()))
            && !stem.starts_with('.')
        {
            names.push(stem.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Write the playlist file of a synced list, rewriting it whole: `<music_root>/<name>.m3u8` with
/// one line per song of `songs`, in list order, at the song's place whether or not its file exists
/// yet. An existing file's real extension is used, otherwise `.opus`. `id` is the file-name
/// fallback when `name` is empty once made safe.
///
/// # Errors
/// [`LibraryError::Io`] when a place cannot be checked or the file cannot be written.
pub fn write(
    music_root: &Path,
    name: &str,
    id: &str,
    songs: &[WantedSong],
) -> Result<(), LibraryError> {
    let mut entries = Vec::with_capacity(songs.len());
    for song in songs {
        let place = Place::of(music_root, song);
        let format = place
            .present()
            .map_err(io_err(&place.dir()))?
            .unwrap_or(Format::Opus);
        entries.push(place.relative(format));
    }
    let playlist = Playlist {
        name: name.to_owned(),
        entries,
    };
    let path = music_root.join(Playlist::file_name(name, id));
    std::fs::write(&path, playlist.render()).map_err(io_err(&path))
}

/// Correct one song's line after review landed it with another extension: in each of the playlist
/// files named by `list_names` (the display names in a held song's `from`), the line for `place`
/// now ends in `format`'s extension. A list with no file, or no line for the song, is left alone.
///
/// # Errors
/// [`LibraryError::Io`] when a file cannot be read or written.
pub fn correct_line(
    music_root: &Path,
    list_names: &[String],
    place: &Place,
    format: Format,
) -> Result<(), LibraryError> {
    let relative_stem = place.relative(format).with_extension("");
    for name in list_names {
        let path = music_root.join(Playlist::file_name(name, ""));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_err(&path)(error)),
        };
        let mut playlist = Playlist::parse(&text, name);
        if playlist.correct(&relative_stem, format) {
            std::fs::write(&path, playlist.render()).map_err(io_err(&path))?;
        }
    }
    Ok(())
}

/// Write the library playlist `<music_root>/<stem>.m3u8` to a device or a Convert destination as
/// `<dest_music_dir>/<stem>.m3u8`, listing only the songs present under `dest_music_dir`, each
/// with the extension it has there (`.opus`, `.mp3` or `.m4a`). Returns the number of songs
/// listed. A missing library playlist writes nothing and lists none.
///
/// # Errors
/// [`LibraryError::Io`] when a file cannot be read or written.
pub fn write_for_destination(
    music_root: &Path,
    dest_music_dir: &Path,
    stem: &str,
) -> Result<usize, LibraryError> {
    let file = format!("{stem}.{PLAYLIST_EXTENSION}");
    let source = music_root.join(&file);
    let text = match std::fs::read_to_string(&source) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_err(&source)(error)),
    };
    let library = Playlist::parse(&text, stem);
    let mut entries = Vec::new();
    for entry in &library.entries {
        let wanted = entry.with_extension("");
        for format in [Format::Opus, Format::Mp3, Format::M4a] {
            let candidate = wanted.with_extension(format.extension());
            if dest_music_dir
                .join(&candidate)
                .try_exists()
                .map_err(io_err(dest_music_dir))?
            {
                entries.push(candidate);
                break;
            }
        }
    }
    let count = entries.len();
    let playlist = Playlist {
        name: library.name,
        entries,
    };
    std::fs::create_dir_all(dest_music_dir).map_err(io_err(dest_music_dir))?;
    let target = dest_music_dir.join(&file);
    std::fs::write(&target, playlist.render()).map_err(io_err(&target))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sdl_core::SongSource;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn song(id: &str, title: &str) -> WantedSong {
        WantedSong {
            source: SongSource::Spotify {
                track_id: id.into(),
            },
            title: title.into(),
            artists: vec!["A".into()],
            album: Some("Al".into()),
            album_artist: None,
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: None,
            cover_url: None,
        }
    }

    fn touch(path: &Path) -> TestResult {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, b"x")?;
        Ok(())
    }

    #[test]
    fn write_lists_every_song_with_its_real_or_default_extension() -> TestResult {
        let root = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/two.mp3"))?;
        write(
            root.path(),
            "Road: trip",
            "id1",
            &[song("1", "one"), song("2", "two")],
        )?;
        let text = std::fs::read_to_string(root.path().join("Road trip.m3u8"))?;
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:Road: trip\nA/Al/one.opus\nA/Al/two.mp3\n"
        );
        assert_eq!(list(root.path())?, ["Road trip"]);
        assert_eq!(list(&root.path().join("missing"))?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn correct_line_changes_one_line_in_the_named_lists_only() -> TestResult {
        let root = tempfile::tempdir()?;
        write(root.path(), "One", "i", &[song("1", "a"), song("2", "b")])?;
        write(root.path(), "Two", "i", &[song("1", "a")])?;
        let place = Place::of(root.path(), &song("1", "a"));
        correct_line(
            root.path(),
            &["One".to_owned(), "Missing".to_owned()],
            &place,
            Format::M4a,
        )?;
        let one = std::fs::read_to_string(root.path().join("One.m3u8"))?;
        let two = std::fs::read_to_string(root.path().join("Two.m3u8"))?;
        assert!(one.contains("A/Al/a.m4a\nA/Al/b.opus\n"), "{one}");
        assert!(two.contains("A/Al/a.opus\n"), "{two}");
        Ok(())
    }

    #[test]
    fn a_destination_playlist_lists_only_what_is_there() -> TestResult {
        let root = tempfile::tempdir()?;
        let device = tempfile::tempdir()?;
        let dest = device.path().join("Music");
        write(
            root.path(),
            "Mix",
            "i",
            &[song("1", "a"), song("2", "b"), song("3", "c")],
        )?;
        touch(&dest.join("A/Al/a.mp3"))?;
        touch(&dest.join("A/Al/c.opus"))?;
        assert_eq!(write_for_destination(root.path(), &dest, "Mix")?, 2);
        assert_eq!(
            std::fs::read_to_string(dest.join("Mix.m3u8"))?,
            "#EXTM3U\n#PLAYLIST:Mix\nA/Al/a.mp3\nA/Al/c.opus\n"
        );
        assert_eq!(write_for_destination(root.path(), &dest, "Nope")?, 0);
        assert!(!dest.join("Nope.m3u8").exists());
        let _: PathBuf = dest;
        Ok(())
    }
}
