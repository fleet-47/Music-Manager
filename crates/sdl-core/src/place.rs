//! The library layout (F7): where a song lives, whether it is present, and what the library
//! holds.
//!
//! A song lives at `<music_root>/<Album artist>/<Album>/<Title>.<ext>`, one file however many
//! lists name it. Every component is FAT-safe, has its leading dots stripped, is capped at 150
//! bytes on a character boundary (Disco reads each playlist line into a 700-byte buffer), and
//! falls back to the song's ID when it would be empty. A song with no album goes in `Singles`.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sanitize_filename::{Options, sanitize_with_options};
use walkdir::WalkDir;

use crate::format::{existing_format, is_audio_file};
use crate::{Format, Library, WantedSong};

/// Folder of songs that have no album.
pub const SINGLES: &str = "Singles";

/// Name of the temp directory under the music root. A dot name, so the library never lists it and
/// device sync never copies it.
const TMP_DIR: &str = ".sdl-tmp";

/// Longest path component, in bytes.
const COMPONENT_MAX: usize = 150;

/// One path component made safe: FAT-illegal characters removed (`sanitize-filename`), leading
/// dots stripped (so no dot-directory can be made), leading `#` and blanks stripped (so a
/// playlist line never reads as a comment), cut to [`COMPONENT_MAX`] bytes on a character
/// boundary, trailing spaces and dots trimmed. May be empty; see [`Place::new`] for the ID
/// fallback.
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
    // A leading `#` or space would make the playlist line that starts with this name a comment
    // or get trimmed by Disco's reader (`import_m3u_file` skips `#` lines and strips leading
    // blanks), so the song would be missing from the playlist.
    let mut safe = safe
        .trim_start_matches(|c: char| c == '.' || c == '#' || c.is_whitespace())
        .to_owned();
    if let Some(end) = (0..=COMPONENT_MAX)
        .rev()
        .find(|&index| safe.is_char_boundary(index))
    {
        safe.truncate(end);
    }
    safe.trim_end_matches([' ', '.']).to_owned()
}

/// [`component`] of `name`, or of `fallback_id` when that is empty.
fn component_or(name: &str, fallback_id: &str) -> String {
    let safe = component(name);
    if safe.is_empty() {
        component(fallback_id)
    } else {
        safe
    }
}

/// Where a song lives, apart from its extension: a library's root, an artist folder, an album folder and a
/// file stem. Two wanted songs with equal places are the same song for the library.
///
/// Invariant: the three components are single path components, never empty unless the song's
/// ID was empty too, and never start with a dot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    root: PathBuf,
    artist: String,
    album: String,
    stem: String,
}

impl Place {
    /// The place of `song` in `library`: album artist (else the first artist), album (else
    /// [`SINGLES`]), title; fields as the source gave them.
    #[must_use]
    pub fn of(library: &Library, song: &WantedSong) -> Self {
        let artist = song
            .album_artist
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| song.artists.first().map(String::as_str))
            .unwrap_or_default();
        Self::new(
            library,
            artist,
            song.album.as_deref(),
            &song.title,
            song.fallback_id(),
        )
    }

    /// A place from names. `album` of `None` or blank is [`SINGLES`]. A component that is empty
    /// after making it safe becomes `fallback_id` made safe.
    #[must_use]
    pub fn new(
        library: &Library,
        artist: &str,
        album: Option<&str>,
        title: &str,
        fallback_id: &str,
    ) -> Self {
        let album = match album {
            Some(name) if !name.trim().is_empty() => component_or(name, fallback_id),
            _ => SINGLES.to_owned(),
        };
        Self {
            root: library.root().to_path_buf(),
            artist: component_or(artist, fallback_id),
            album,
            stem: component_or(title, fallback_id),
        }
    }

    /// The same place under another root: a device's `Music` folder, a Convert destination, or
    /// another library's folder ([`Place::held_elsewhere`]).
    #[must_use]
    pub fn under(&self, root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            ..self.clone()
        }
    }

    /// The file stem (the title, made safe).
    #[must_use]
    pub fn stem(&self) -> &str {
        &self.stem
    }

    /// `<root>/<artist>/<album>`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.root.join(&self.artist).join(&self.album)
    }

    /// `<artist>/<album>/<stem>.<ext>`, relative to the root: the line of a playlist file and
    /// the path under a device's `Music` folder.
    #[must_use]
    pub fn relative(&self, format: Format) -> PathBuf {
        PathBuf::from(&self.artist).join(&self.album).join(format!(
            "{}.{}",
            self.stem,
            format.extension()
        ))
    }

    /// `<root>/<artist>/<album>/<stem>.<ext>`.
    #[must_use]
    pub fn path(&self, format: Format) -> PathBuf {
        self.root.join(self.relative(format))
    }

    /// The file that another of `libraries` already has at this place, in whatever format (the
    /// copy shortcut of F12): the first library, in the order given, whose folder is not this
    /// place's own and that holds a file here. `None` when no other library has it.
    ///
    /// # Errors
    /// A folder that exists but cannot be read.
    pub fn held_elsewhere(&self, libraries: &[Library]) -> std::io::Result<Option<PathBuf>> {
        for library in libraries
            .iter()
            .filter(|library| library.root() != self.root)
        {
            let there = self.under(library.root());
            if let Some(format) = there.present()? {
                return Ok(Some(there.path(format)));
            }
        }
        Ok(None)
    }

    /// The format of the file that exists at this place, trying `.opus`, `.mp3`, `.m4a` in that
    /// order; `None` when there is none. A song is present when this is `Some`.
    ///
    /// # Errors
    /// A folder that exists but cannot be read. A missing folder or file is `Ok(None)`.
    pub fn present(&self) -> std::io::Result<Option<Format>> {
        existing_format(&self.path(Format::Opus))
    }
}

/// The files of `library` (F7): every `.opus`, `.mp3` and `.m4a` file under the library's folder
/// at any depth, skipping any file or folder whose name starts with a dot, sorted by path.
/// Symbolic links inside the library are not followed; the library's folder itself may be one. A
/// missing library folder is an empty library.
///
/// # Errors
/// A folder that exists but cannot be read.
pub fn library_files(library: &Library) -> std::io::Result<Vec<PathBuf>> {
    // `follow_root_links` is on by default: a linked library folder is still walked.
    let walk = WalkDir::new(library.root())
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0 || !entry.file_name().to_string_lossy().starts_with('.')
        });
    let mut files = Vec::new();
    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                let io = std::io::Error::from(error);
                if io.kind() == ErrorKind::NotFound {
                    continue;
                }
                return Err(io);
            }
        };
        if entry.file_type().is_file() && is_audio_file(entry.path()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
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
    use crate::SongSource;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn lib(root: &Path) -> Library {
        Library::at("Me", root, true)
    }

    fn song(
        title: &str,
        artists: &[&str],
        album: Option<&str>,
        album_artist: Option<&str>,
    ) -> WantedSong {
        WantedSong {
            source: SongSource::Spotify {
                track_id: "sid".into(),
            },
            title: title.into(),
            artists: artists.iter().map(|a| (*a).to_owned()).collect(),
            album: album.map(str::to_owned),
            album_artist: album_artist.map(str::to_owned),
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: None,
            cover_url: None,
        }
    }

    #[test]
    fn layout_is_artist_album_title() {
        let place = Place::of(
            &lib(Path::new("/m")),
            &song("Why?/Not", &["A", "B"], Some("Al"), Some("AA")),
        );
        assert_eq!(
            place.path(Format::Opus),
            PathBuf::from("/m/AA/Al/WhyNot.opus")
        );
        assert_eq!(
            place.relative(Format::M4a),
            PathBuf::from("AA/Al/WhyNot.m4a")
        );
        assert_eq!(place.dir(), PathBuf::from("/m/AA/Al"));
        assert_eq!(place.stem(), "WhyNot");
    }

    #[test]
    fn album_artist_falls_back_to_the_first_artist() {
        let place = Place::of(
            &lib(Path::new("/m")),
            &song("T", &["A", "B"], Some("Al"), None),
        );
        assert_eq!(place.path(Format::Opus), PathBuf::from("/m/A/Al/T.opus"));
        let blank = Place::of(
            &lib(Path::new("/m")),
            &song("T", &["A"], Some("Al"), Some("  ")),
        );
        assert_eq!(blank.path(Format::Opus), PathBuf::from("/m/A/Al/T.opus"));
    }

    #[test]
    fn a_song_with_no_album_goes_in_singles() {
        for album in [None, Some(""), Some("   ")] {
            let place = Place::of(&lib(Path::new("/m")), &song("T", &["A"], album, None));
            assert_eq!(place.path(Format::Mp3), PathBuf::from("/m/A/Singles/T.mp3"));
        }
    }

    #[test]
    fn components_are_fat_safe_and_never_dot_names() {
        assert_eq!(component("a:b*c"), "abc");
        assert_eq!(component(".Spotlight-V100"), "Spotlight-V100");
        assert_eq!(component("..hidden"), "hidden");
        assert_eq!(component("name. "), "name");
        assert_eq!(component("."), "");
        assert_eq!(component(".."), "");
    }

    #[test]
    fn no_component_can_start_a_playlist_comment() {
        assert_eq!(component("#1 Dads"), "1 Dads");
        assert_eq!(component(" # .x"), "x");
        assert_eq!(component("Track #1"), "Track #1");
    }

    #[test]
    fn components_are_capped_at_150_bytes_on_a_character_boundary() {
        let long = component(&"é".repeat(300));
        assert_eq!(long.len(), 150);
        assert!(long.chars().all(|c| c == 'é'));
        assert!(component(&format!("{}x", " ".repeat(149))).len() <= 150);
        // A cut that lands after a space leaves no trailing space.
        let spaced = component(&format!("{} tail", "a".repeat(149)));
        assert_eq!(spaced, "a".repeat(149));
    }

    #[test]
    fn empty_components_fall_back_to_the_id() {
        let mut wanted = song("???", &["..."], Some("***"), None);
        wanted.source = SongSource::Link {
            item_url: "https://y/v".into(),
            video_id: "vid".into(),
            direct: false,
        };
        let place = Place::of(&lib(Path::new("/m")), &wanted);
        assert_eq!(
            place.path(Format::Opus),
            PathBuf::from("/m/vid/vid/vid.opus")
        );
    }

    #[test]
    fn a_song_with_no_artist_at_all_uses_the_id_too() {
        let place = Place::of(&lib(Path::new("/m")), &song("T", &[], None, None));
        assert_eq!(
            place.path(Format::Opus),
            PathBuf::from("/m/sid/Singles/T.opus")
        );
    }

    #[test]
    fn a_playlist_line_stays_well_under_the_disco_buffer() {
        let place = Place::of(
            &lib(Path::new("/m")),
            &song(
                &"t".repeat(400),
                &["a"],
                Some(&"b".repeat(400)),
                Some(&"c".repeat(400)),
            ),
        );
        assert!(place.relative(Format::Opus).to_string_lossy().len() < 700);
    }

    #[test]
    fn present_in_any_format() -> TestResult {
        let root = tempfile::tempdir()?;
        let place = Place::of(&lib(root.path()), &song("Song", &["A"], Some("Al"), None));
        assert_eq!(place.present()?, None, "missing folder");
        std::fs::create_dir_all(place.dir())?;
        for format in [Format::Mp3, Format::M4a, Format::Opus] {
            let file = place.path(format);
            std::fs::write(&file, b"x")?;
            assert_eq!(place.present()?, Some(format));
            std::fs::remove_file(&file)?;
        }
        std::fs::write(place.dir().join("Song.flac"), b"x")?;
        std::fs::write(place.dir().join("Song 2.mp3"), b"x")?;
        assert_eq!(place.present()?, None);
        Ok(())
    }

    #[test]
    fn a_place_can_move_under_another_root() {
        let place = Place::of(&lib(Path::new("/m")), &song("T", &["A"], Some("Al"), None));
        assert_eq!(
            place.under(Path::new("/d/Music")).path(Format::Mp3),
            PathBuf::from("/d/Music/A/Al/T.mp3")
        );
    }

    /// Write `b"x"` at `rel` under `root`, making its folders.
    fn write_under(root: &Path, rel: &str) -> TestResult {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, b"x")?;
        Ok(())
    }

    /// `library_files` of a library at `root`, relative to `root`.
    fn found_under(root: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
        Ok(library_files(&lib(root))?
            .into_iter()
            .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_path_buf))
            .collect())
    }

    #[test]
    fn the_library_is_every_audio_file_at_any_depth_without_dot_names() -> TestResult {
        let root = tempfile::tempdir()?;
        for rel in [
            "A/Al/one.opus",
            "A/Al/two.MP3",
            "B/Singles/three.m4a",
            "A/Al/cover.jpg",
            "A/loose.opus",
            "loose.opus",
            "A/Al/deeper/four.opus",
            "Unsorted/Old/sub/untagged.mp3",
            ".sdl-tmp/x/y.opus",
            "A/.hidden/five.opus",
            "A/Al/.six.opus",
            ".seven.opus",
            "Liked Songs.m3u8",
        ] {
            write_under(root.path(), rel)?;
        }
        assert_eq!(
            found_under(root.path())?,
            [
                "A/Al/deeper/four.opus",
                "A/Al/one.opus",
                "A/Al/two.MP3",
                "A/loose.opus",
                "B/Singles/three.m4a",
                "Unsorted/Old/sub/untagged.mp3",
                "loose.opus",
            ]
            .map(PathBuf::from)
        );
        assert_eq!(
            library_files(&lib(&root.path().join("missing")))?,
            Vec::<PathBuf>::new()
        );
        Ok(())
    }

    #[test]
    fn links_inside_the_library_are_not_followed_but_a_linked_library_folder_is_walked()
    -> TestResult {
        let dir = tempfile::tempdir()?;
        let real = dir.path().join("real");
        let elsewhere = dir.path().join("elsewhere");
        write_under(&real, "A/Al/one.opus")?;
        write_under(&elsewhere, "Al/two.opus")?;
        write_under(&elsewhere, "three.opus")?;
        std::os::unix::fs::symlink(&elsewhere, real.join("Linked"))?;
        std::os::unix::fs::symlink(elsewhere.join("three.opus"), real.join("A/Al/three.opus"))?;
        assert_eq!(found_under(&real)?, [PathBuf::from("A/Al/one.opus")]);
        // The library's own folder may be a link: it is walked, its contents by their link path.
        let linked_root = dir.path().join("Me");
        std::os::unix::fs::symlink(&real, &linked_root)?;
        assert_eq!(found_under(&linked_root)?, [PathBuf::from("A/Al/one.opus")]);
        Ok(())
    }

    #[test]
    fn another_library_holding_the_place_is_found_in_any_format() -> TestResult {
        let root = tempfile::tempdir()?;
        let mine = Library::at("Me", &root.path().join("Me"), true);
        let sam = Library::at("Sam", &root.path().join("Sam"), false);
        let alex = Library::at("Alex", &root.path().join("Alex"), false);
        let all = [mine.clone(), alex.clone(), sam.clone()];
        let wanted = song("Song", &["A"], Some("Al"), None);
        let place = Place::of(&mine, &wanted);
        assert_eq!(place.held_elsewhere(&all)?, None);
        // A file in the place's own library does not count.
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::Opus), b"x")?;
        assert_eq!(place.held_elsewhere(&all)?, None);
        let theirs = Place::of(&sam, &wanted);
        std::fs::create_dir_all(theirs.dir())?;
        std::fs::write(theirs.path(Format::M4a), b"x")?;
        assert_eq!(place.held_elsewhere(&all)?, Some(theirs.path(Format::M4a)));
        // And the other way round: Sam's place finds mine.
        assert_eq!(theirs.held_elsewhere(&all)?, Some(place.path(Format::Opus)));
        Ok(())
    }

    #[test]
    fn device_and_tmp_dirs() {
        assert_eq!(
            device_music_dir(Path::new("/Volumes/X")),
            PathBuf::from("/Volumes/X/Music")
        );
        assert_eq!(tmp_dir(Path::new("/m")), PathBuf::from("/m/.sdl-tmp"));
    }

    #[test]
    fn tmp_dir_is_removed_only_when_empty() -> TestResult {
        let root = tempfile::tempdir()?;
        remove_tmp_dir_if_empty(root.path())?;
        let file = tmp_dir(root.path()).join("x.opus");
        std::fs::create_dir_all(tmp_dir(root.path()))?;
        std::fs::write(&file, b"x")?;
        remove_tmp_dir_if_empty(root.path())?;
        assert!(tmp_dir(root.path()).exists());
        std::fs::remove_file(&file)?;
        remove_tmp_dir_if_empty(root.path())?;
        assert!(!tmp_dir(root.path()).exists());
        Ok(())
    }
}
