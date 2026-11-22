//! Playlist files (F7, F12): `<library folder>/<List name>.m3u8` pointer files. The file model (format,
//! parsing, line correction) is `sdl_core::Playlist`; this module reads and writes the files.
//!
//! Plain blocking file I/O on small files; call from async code directly.

use std::path::Path;

use sdl_core::format::existing_format;
use sdl_core::landing;
use sdl_core::playlist::PLAYLIST_EXTENSION;
use sdl_core::{Format, Library, Place, Playlist, WantedSong};

use crate::{LibraryError, io_err};

/// The playlists of `library`: the stems of the `.m3u8` files directly in its folder, sorted,
/// skipping names that start with a dot. A missing folder has none.
///
/// # Errors
/// [`LibraryError::Io`] when the folder cannot be read.
pub fn list(library: &Library) -> Result<Vec<String>, LibraryError> {
    let music_root = library.root();
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

/// Write the playlist file of a synced list, rewriting it whole: `<library folder>/<name>.m3u8` with
/// one line per song of `songs`, in list order, at the song's place whether or not its file exists
/// yet. An existing file's real extension is used, otherwise `.opus`. `id` is the file-name
/// fallback when `name` is empty once made safe.
///
/// # Errors
/// [`LibraryError::Io`] when a place cannot be checked or the file cannot be written.
pub fn write(
    library: &Library,
    name: &str,
    id: &str,
    songs: &[WantedSong],
) -> Result<(), LibraryError> {
    let mut entries = Vec::with_capacity(songs.len());
    for song in songs {
        let place = Place::of(library, song);
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
    let path = library.root().join(Playlist::file_name(name, id));
    landing::write_atomic(&path, playlist.render().as_bytes()).map_err(io_err(&path))
}

/// Correct one song's line after review landed it with another extension: in each of the playlist
/// files of `library` named by `files` (the file names a held song kept,
/// [`sdl_core::HeldSong::playlist_files`], so a list written under its ID is found like any other),
/// the line for `place` now ends in `format`'s extension. A file that is missing, or has no line
/// for the song, is left alone.
///
/// # Errors
/// [`LibraryError::Io`] when a file cannot be read or written.
pub fn correct_line(
    library: &Library,
    files: &[String],
    place: &Place,
    format: Format,
) -> Result<(), LibraryError> {
    let relative_stem = place.relative(format).with_extension("");
    for file in files {
        let path = library.root().join(file);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_err(&path)(error)),
        };
        let fallback = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned());
        let mut playlist = Playlist::parse(&text, fallback.as_deref().unwrap_or_default());
        if playlist.correct(&relative_stem, format) {
            landing::write_atomic(&path, playlist.render().as_bytes()).map_err(io_err(&path))?;
        }
    }
    Ok(())
}

/// Write the playlist `<library folder>/<stem>.m3u8` of `library` to a device or a Convert destination as
/// `<dest_music_dir>/<stem>.m3u8`, listing only the songs present under `dest_music_dir`, each
/// with the extension it has there (`.opus`, `.mp3` or `.m4a`). Returns the number of songs
/// listed. A missing library playlist writes nothing and lists none.
///
/// # Errors
/// [`LibraryError::Io`] when a file cannot be read or written.
pub fn write_for_destination(
    library: &Library,
    dest_music_dir: &Path,
    stem: &str,
) -> Result<usize, LibraryError> {
    let file = format!("{stem}.{PLAYLIST_EXTENSION}");
    let source = library.root().join(&file);
    let text = match std::fs::read_to_string(&source) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_err(&source)(error)),
    };
    let library = Playlist::parse(&text, stem);
    let mut entries = Vec::new();
    for entry in &library.entries {
        let there = dest_music_dir.join(entry);
        if let Some(format) = existing_format(&there).map_err(io_err(&there))? {
            entries.push(entry.with_extension(format.extension()));
        }
    }
    let count = entries.len();
    let playlist = Playlist {
        name: library.name,
        entries,
    };
    std::fs::create_dir_all(dest_music_dir).map_err(io_err(dest_music_dir))?;
    let target = dest_music_dir.join(&file);
    landing::write_atomic(&target, playlist.render().as_bytes()).map_err(io_err(&target))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sdl_core::SongSource;

    use super::*;
    use crate::testkit::lib;

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
            &lib(root.path()),
            "Road: trip",
            "id1",
            &[song("1", "one"), song("2", "two")],
        )?;
        let text = std::fs::read_to_string(root.path().join("Road trip.m3u8"))?;
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:Road: trip\nA/Al/one.opus\nA/Al/two.mp3\n"
        );
        assert_eq!(list(&lib(root.path()))?, ["Road trip"]);
        assert_eq!(
            list(&lib(&root.path().join("missing")))?,
            Vec::<String>::new()
        );
        Ok(())
    }

    #[test]
    fn correct_line_changes_one_line_in_the_named_lists_only() -> TestResult {
        let root = tempfile::tempdir()?;
        write(
            &lib(root.path()),
            "One",
            "i",
            &[song("1", "a"), song("2", "b")],
        )?;
        write(&lib(root.path()), "Two", "i", &[song("1", "a")])?;
        let place = Place::of(&lib(root.path()), &song("1", "a"));
        correct_line(
            &lib(root.path()),
            &["One.m3u8".to_owned(), "Missing.m3u8".to_owned()],
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
    fn a_list_written_under_its_id_is_corrected_by_its_file_name() -> TestResult {
        let root = tempfile::tempdir()?;
        let library = lib(root.path());
        write(&library, "???", "37i9dQZF1DX", &[song("1", "a")])?;
        assert!(root.path().join("37i9dQZF1DX.m3u8").exists());
        let place = Place::of(&library, &song("1", "a"));
        correct_line(
            &library,
            &["37i9dQZF1DX.m3u8".to_owned()],
            &place,
            Format::Mp3,
        )?;
        let text = std::fs::read_to_string(root.path().join("37i9dQZF1DX.m3u8"))?;
        assert!(text.contains("A/Al/a.mp3\n"), "{text}");
        Ok(())
    }

    #[test]
    fn a_library_keeps_its_playlists_to_itself() -> TestResult {
        let root = tempfile::tempdir()?;
        let mine = Library::at("Me", &root.path().join("Me"), true);
        let sam = Library::at("Sam", &root.path().join("Sam"), false);
        std::fs::create_dir_all(mine.root())?;
        std::fs::create_dir_all(sam.root())?;
        write(&sam, "Road trip", "id", &[song("1", "a")])?;
        assert_eq!(list(&sam)?, ["Road trip"]);
        assert_eq!(list(&mine)?, Vec::<String>::new());
        let text = std::fs::read_to_string(sam.root().join("Road trip.m3u8"))?;
        assert!(text.contains("A/Al/a.opus\n"), "{text}");
        Ok(())
    }

    #[test]
    fn a_destination_playlist_lists_only_what_is_there() -> TestResult {
        let root = tempfile::tempdir()?;
        let device = tempfile::tempdir()?;
        let dest = device.path().join("Music");
        write(
            &lib(root.path()),
            "Mix",
            "i",
            &[song("1", "a"), song("2", "b"), song("3", "c")],
        )?;
        touch(&dest.join("A/Al/a.mp3"))?;
        touch(&dest.join("A/Al/c.opus"))?;
        assert_eq!(write_for_destination(&lib(root.path()), &dest, "Mix")?, 2);
        assert_eq!(
            std::fs::read_to_string(dest.join("Mix.m3u8"))?,
            "#EXTM3U\n#PLAYLIST:Mix\nA/Al/a.mp3\nA/Al/c.opus\n"
        );
        assert_eq!(write_for_destination(&lib(root.path()), &dest, "Nope")?, 0);
        // A title with a dot keeps its whole name on the device.
        write(
            &lib(root.path()),
            "Dot",
            "i",
            &[song("4", "Mr. Brightside")],
        )?;
        touch(&dest.join("A/Al/Mr. Brightside.mp3"))?;
        assert_eq!(write_for_destination(&lib(root.path()), &dest, "Dot")?, 1);
        assert_eq!(
            std::fs::read_to_string(dest.join("Dot.m3u8"))?,
            "#EXTM3U\n#PLAYLIST:Dot\nA/Al/Mr. Brightside.mp3\n"
        );
        assert!(!dest.join("Nope.m3u8").exists());
        let _: PathBuf = dest;
        Ok(())
    }

    /// What Disco's `import_m3u_file` (`ui/musicdb.c`, read-only reference) needs of a file:
    /// no BOM (it strips one, but a name check scans the header first), `\n` or `\r\n` lines,
    /// `#PLAYLIST:` in the header (header lines are read into 192 bytes, longer ones end the
    /// header scan), `\` read as `/`, an entry is any non-blank line not starting with `#`, a
    /// relative entry is joined to the playlist's folder as `<dir>/<entry>` and that must fit 700
    /// bytes, and a line is read into 700 bytes (an entry of 699 bytes or more is skipped).
    #[test]
    fn the_file_has_the_form_disco_imports() -> TestResult {
        let root = tempfile::tempdir()?;
        let long = "é".repeat(300);
        let mut songs = vec![
            song("1", "one"),
            song("2", &long),
            song("3", "Mr. Brightside"),
        ];
        songs[1].artists = vec![long.clone()];
        songs[1].album = Some(long.clone());
        songs[2].artists = vec!["#1 Dads".into()];
        write(&lib(root.path()), "Road trip", "id", &songs)?;
        let bytes = std::fs::read(root.path().join("Road trip.m3u8"))?;
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "no BOM");
        assert!(
            !bytes.contains(&b'\r') && !bytes.contains(&b'\\'),
            "LF lines, forward slashes"
        );
        let text = String::from_utf8(bytes)?;
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#EXTM3U");
        assert_eq!(lines[1], "#PLAYLIST:Road trip");
        assert!(
            lines[1].len() < 192,
            "header lines past 191 bytes are not read as a directive"
        );
        let entries = &lines[2..];
        assert_eq!(entries.len(), songs.len());
        let mut longest = 0;
        for entry in entries {
            assert!(
                !entry.starts_with(['#', '/', ' ', '\t']),
                "{entry}: read as an entry, relative"
            );
            assert!(
                entry.len() < 699,
                "{} bytes: fits Disco's line buffer",
                entry.len()
            );
            // Folders are capped at 150 bytes; the file name is a 150-byte stem plus `.opus`.
            let parts: Vec<&str> = entry.split('/').collect();
            assert_eq!(parts.len(), 3, "{entry}");
            assert!(
                parts
                    .iter()
                    .all(|p| !p.is_empty() && *p != ".." && p.len() <= 155)
            );
            longest = longest.max(entry.len());
        }
        // The longest possible entry (three 150-byte components, two slashes, `.opus`) leaves
        // room in the 700-byte joined path for a playlist folder of over 200 bytes.
        let worst = 3 * 150 + 2 + ".opus".len();
        assert!(
            longest <= worst && 700 > worst + 1 + 200,
            "worst entry {worst} bytes"
        );
        Ok(())
    }

    #[test]
    fn a_very_long_name_still_gets_a_short_safe_file_name() -> TestResult {
        // Disco reads the name from the header only when the line is under 192 bytes; otherwise
        // it names the playlist after the file, which `Playlist::file_name` made safe.
        let root = tempfile::tempdir()?;
        let name = "n".repeat(400);
        write(&lib(root.path()), &name, "id", &[song("1", "one")])?;
        let file = root.path().join(Playlist::file_name(&name, "id"));
        assert!(file.exists() && file.file_name().is_some_and(|f| f.len() <= 155));
        Ok(())
    }
}
