//! Path rules shared by planning and running: dot names, and a song's file in any format.

use std::path::{Component, Path, PathBuf};

use sdl_core::format::existing_format;

use crate::DevicesError;

/// An `Io` error naming `path`, for `map_err`.
pub(crate) fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> DevicesError {
    let path = path.to_path_buf();
    move |source| DevicesError::Io { path, source }
}

/// Whether any component of `relative` starts with a dot. Such names are never copied.
pub(crate) fn has_dot_name(relative: &Path) -> bool {
    relative
        .components()
        .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
}

/// Whether `relative` stays inside its root: only plain names, none starting with a dot. Playlist
/// lines are text from a file, so a line that is absolute or climbs out is not followed.
pub(crate) fn stays_inside(relative: &Path) -> bool {
    relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
        && !has_dot_name(relative)
}

/// The file that exists with the same name as `path` in any audio format (`.opus`, `.mp3` or
/// `.m4a`; `path` itself counts), by `sdl_core::format::existing_format`. A missing folder is
/// `None`.
///
/// # Errors
/// A folder that exists but cannot be read.
pub(crate) fn existing_in_any_format(path: &Path) -> Result<Option<PathBuf>, DevicesError> {
    Ok(existing_format(path)
        .map_err(io_err(path))?
        .map(|format| path.with_extension(format.extension())))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn dot_names_and_escapes_are_refused() {
        assert!(has_dot_name(Path::new("A/.hidden/x.mp3")));
        assert!(has_dot_name(Path::new("._x.mp3")));
        assert!(!has_dot_name(Path::new("A/Mr. Brightside.mp3")));
        assert!(stays_inside(Path::new("A/B/c.opus")));
        assert!(!stays_inside(Path::new("/etc/x.opus")));
        assert!(!stays_inside(Path::new("../x.opus")));
    }

    #[test]
    fn a_song_is_found_in_any_format_and_a_dotted_title_keeps_its_name() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Mr. Brightside.m4a"), b"x")?;
        let found = existing_in_any_format(&dir.path().join("Mr. Brightside.opus"))?;
        assert_eq!(found, Some(dir.path().join("Mr. Brightside.m4a")));
        assert_eq!(
            existing_in_any_format(&dir.path().join("Other.opus"))?,
            None
        );
        assert_eq!(
            existing_in_any_format(&dir.path().join("nope/Other.opus"))?,
            None
        );
        Ok(())
    }
}
