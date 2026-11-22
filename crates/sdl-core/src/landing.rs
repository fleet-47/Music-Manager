//! Putting a finished file at its place (spec H2): the one place a file lands, and the one place
//! a small text file is written whole.
//!
//! Interface, in full:
//! * A file is made complete under a temp name first ([`part_beside`], or any `tempfile` on the
//!   same volume), then [`land`] moves it to its place. **Nothing is ever replaced**: when a file
//!   is already there the answer is [`Landed::Existed`], which is not an error, the file that
//!   was there is untouched and the temp file is removed.
//! * [`Landing`] says where the place is. In the library the move itself refuses to replace
//!   (`tempfile`'s `persist_noclobber`). On a device or a Convert destination the drive may be
//!   FAT32, where macOS refuses that call (os error 45), so the place is checked free and the
//!   file renamed; only one task writes to a given device or destination at a time (F3).
//! * [`write_atomic`] is for the files the app rewrites whole (`state.toml`, `review.toml`,
//!   playlist files): a crash leaves the old file or the new one, never half of one.
//!
//! All three are plain blocking file calls.

use std::path::Path;

use tempfile::{NamedTempFile, TempPath};

/// Where a file is landing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// A place in a library, on the Mac's own disk.
    Library,
    /// A place on a device or in a Convert destination, which may be FAT32.
    Destination,
}

/// How a landing ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landed {
    /// The file is at its place.
    Written,
    /// A file was already there; nothing was replaced and the temp file is gone.
    Existed,
}

/// A new, empty temp file in `dest`'s folder (made when missing), named `<dest's name><random>.part`:
/// on the same volume as `dest`, so [`land`] is a rename. It is removed when dropped unless landed.
///
/// # Errors
/// `dest` has no folder or file name, or the folder or file could not be made.
pub fn part_beside(dest: &Path) -> std::io::Result<NamedTempFile> {
    let (folder, name) = split(dest)?;
    std::fs::create_dir_all(folder)?;
    tempfile::Builder::new()
        .prefix(name)
        .suffix(".part")
        .tempfile_in(folder)
}

/// Move the finished temp file `part` to `dest` (making `dest`'s folder when missing), never
/// replacing a file.
///
/// # Errors
/// The folder could not be made or the file could not be moved. A file already at `dest` is
/// `Ok(Landed::Existed)`, not an error.
pub fn land(part: TempPath, dest: &Path, landing: Landing) -> std::io::Result<Landed> {
    let (folder, _) = split(dest)?;
    std::fs::create_dir_all(folder)?;
    match landing {
        Landing::Library => match part.persist_noclobber(dest) {
            Ok(()) => Ok(Landed::Written),
            Err(failure) if failure.error.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(Landed::Existed)
            }
            Err(failure) => Err(failure.error),
        },
        Landing::Destination => {
            if dest.try_exists()? {
                return Ok(Landed::Existed);
            }
            part.persist(dest).map_err(|failure| failure.error)?;
            Ok(Landed::Written)
        }
    }
}

/// Write `contents` as the whole of the file at `path`, replacing what was there in one step
/// (`atomic-write-file`: a temp file in the same folder, synced, then renamed over `path`). The
/// folder is made when missing.
///
/// # Errors
/// The folder could not be made or the file could not be written.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let (folder, _) = split(path)?;
    std::fs::create_dir_all(folder)?;
    let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
    file.write_all(contents)?;
    file.commit()
}

/// The folder and file name of `path`; a bare file name sits in the current folder.
fn split(path: &Path) -> std::io::Result<(&Path, &std::ffi::OsStr)> {
    let no_place =
        || std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name");
    let name = path.file_name().ok_or_else(no_place)?;
    let folder = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok((folder, name))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Result<Vec<String>, std::io::Error> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            found.push(entry?.file_name().to_string_lossy().into_owned());
        }
        found.sort();
        Ok(found)
    }

    #[test]
    fn a_part_is_made_beside_its_place_and_goes_when_dropped() -> TestResult {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("Artist").join("Album").join("Song.opus");
        let part = part_beside(&dest)?;
        let folder = dest.parent().ok_or("no folder")?;
        assert_eq!(part.path().parent(), Some(folder));
        let name = part
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(name.starts_with("Song.opus"), "{name}");
        assert_eq!(
            part.path().extension().and_then(|ext| ext.to_str()),
            Some("part")
        );
        drop(part);
        assert_eq!(names(folder)?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn a_file_lands_at_a_free_place_in_either_mode_making_its_folder() -> TestResult {
        for landing in [Landing::Library, Landing::Destination] {
            let dir = tempfile::tempdir()?;
            // The temp file is elsewhere on the same volume, as a download's is.
            let mut part = NamedTempFile::new_in(dir.path())?;
            part.write_all(b"audio")?;
            let dest = dir.path().join("Artist").join("Album").join("Song.opus");
            assert_eq!(
                land(part.into_temp_path(), &dest, landing)?,
                Landed::Written
            );
            assert_eq!(std::fs::read(&dest)?, b"audio");
            assert_eq!(names(dir.path())?, ["Artist"], "the temp name is gone");
        }
        Ok(())
    }

    #[test]
    fn a_taken_place_is_never_replaced_and_is_not_an_error() -> TestResult {
        for landing in [Landing::Library, Landing::Destination] {
            let dir = tempfile::tempdir()?;
            let dest = dir.path().join("Song.opus");
            std::fs::write(&dest, b"first")?;
            let mut part = part_beside(&dest)?;
            part.write_all(b"second")?;
            assert_eq!(
                land(part.into_temp_path(), &dest, landing)?,
                Landed::Existed
            );
            assert_eq!(std::fs::read(&dest)?, b"first");
            assert_eq!(names(dir.path())?, ["Song.opus"], "the part is removed");
        }
        Ok(())
    }

    #[test]
    fn a_whole_file_write_replaces_in_one_step_and_leaves_nothing_else() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("lists").join("Mix.m3u8");
        write_atomic(&path, b"#EXTM3U\none\n")?;
        assert_eq!(std::fs::read(&path)?, b"#EXTM3U\none\n");
        write_atomic(&path, b"#EXTM3U\n")?;
        assert_eq!(std::fs::read(&path)?, b"#EXTM3U\n", "shorter content too");
        assert_eq!(names(&dir.path().join("lists"))?, ["Mix.m3u8"]);
        Ok(())
    }
}
