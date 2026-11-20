//! Clean up and eject (F10): remove the macOS items a Mac leaves on a device, then unmount it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use sdl_core::Device;
use tokio::process::Command;
use walkdir::WalkDir;

use crate::DevicesError;
use crate::paths::io_err;

/// Folders macOS makes in a volume's root.
const ROOT_ITEMS: [&str; 4] = [
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    ".TemporaryItems",
];

/// The first bytes of an `AppleDouble` file.
const APPLE_DOUBLE: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// Clean up and eject (the owner has been asked, and was told it empties the device's Trash):
/// remove `.Spotlight-V100`, `.Trashes`, `.fseventsd` and `.TemporaryItems` from the device root
/// and every `.DS_Store`; remove a `._X` file only when `X` is in `written` (this run wrote it)
/// and the file starts with the `AppleDouble` signature (bytes `00 05 16 07`); then
/// `diskutil unmount <mount point>` (`tokio::process::Command`). Removes nothing else. A clean-up
/// that fails stops before the unmount, so the owner is told before anything else happens.
///
/// # Errors
/// [`DevicesError::Io`] when the device cannot be read or an item cannot be removed;
/// [`DevicesError::Diskutil`] when the unmount is refused, with `diskutil`'s own words (for
/// example which program is still using the device).
pub async fn clean_up_and_eject(device: &Device, written: &[PathBuf]) -> Result<(), DevicesError> {
    let (root, written) = (device.mount_point.clone(), written.to_vec());
    tokio::task::spawn_blocking(move || clean_up(&root, &written))
        .await
        .map_err(|error| {
            DevicesError::Library(sdl_library::LibraryError::Task(error.to_string()))
        })??;
    let output = Command::new("diskutil")
        .arg("unmount")
        .arg(&device.mount_point)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| DevicesError::Diskutil {
            action: "unmount",
            message: error.to_string(),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let said = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    Err(DevicesError::Diskutil {
        action: "unmount",
        message: String::from_utf8_lossy(said).trim().to_owned(),
    })
}

/// The file-system half of the clean-up, on the tree under `root`.
pub(crate) fn clean_up(root: &Path, written: &[PathBuf]) -> Result<(), DevicesError> {
    for name in ROOT_ITEMS {
        let path = root.join(name);
        let removed = match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(error) => Err(error),
        };
        match removed {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(io_err(&path)(error));
            }
            _ => {}
        }
    }
    let written: HashSet<&Path> = written.iter().map(PathBuf::as_path).collect();
    let mut doomed = Vec::new();
    for entry in WalkDir::new(root) {
        let entry = entry.map_err(|error| DevicesError::Io {
            path: error.path().unwrap_or(root).to_path_buf(),
            source: error.into(),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let is_ours = name
            .strip_prefix("._")
            .is_some_and(|wrote| written.contains(entry.path().with_file_name(wrote).as_path()))
            && starts_with_apple_double(entry.path()).map_err(io_err(entry.path()))?;
        if name == ".DS_Store" || is_ours {
            doomed.push(entry.into_path());
        }
    }
    for path in doomed {
        std::fs::remove_file(&path).map_err(io_err(&path))?;
    }
    Ok(())
}

fn starts_with_apple_double(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = [0_u8; 4];
    let mut file = std::fs::File::open(path)?;
    match file.read_exact(&mut head) {
        Ok(()) => Ok(head == APPLE_DOUBLE),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn put(path: &Path, content: &[u8]) -> TestResult {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    #[test]
    fn clean_up_removes_the_macos_items_and_only_the_dot_underscore_files_this_run_wrote()
    -> TestResult {
        let dev = tempfile::tempdir()?;
        let root = dev.path();
        for name in ROOT_ITEMS {
            put(&root.join(name).join("inside/x"), b"x")?;
        }
        put(&root.join(".DS_Store"), b"x")?;
        put(&root.join("Music/A/.DS_Store"), b"x")?;
        let mut signature = APPLE_DOUBLE.to_vec();
        signature.extend_from_slice(b"rest of the AppleDouble header");
        // Written by this run, right signature: goes.
        let wrote = root.join("Music/A/Al/new.mp3");
        put(&wrote, b"audio")?;
        put(&root.join("Music/A/Al/._new.mp3"), &signature)?;
        // Not written by this run (an earlier copy, or a player's): stays.
        put(&root.join("Music/A/Al/old.mp3"), b"audio")?;
        put(&root.join("Music/A/Al/._old.mp3"), &signature)?;
        // Written by this run but not an AppleDouble file: stays.
        let wrote_too = root.join("Music/A/Al/other.mp3");
        put(&wrote_too, b"audio")?;
        put(
            &root.join("Music/A/Al/._other.mp3"),
            b"something else entirely",
        )?;
        // Too short to carry a signature: stays.
        let short = root.join("Music/A/Al/short.mp3");
        put(&short, b"audio")?;
        put(&root.join("Music/A/Al/._short.mp3"), &[0, 5])?;
        // A hidden folder that is not on the list: stays, and so does its content.
        put(&root.join(".diskos/keep"), b"x")?;

        clean_up(root, &[wrote.clone(), wrote_too, short])?;

        for name in ROOT_ITEMS {
            assert!(!root.join(name).exists(), "{name}");
        }
        assert!(!root.join(".DS_Store").exists());
        assert!(!root.join("Music/A/.DS_Store").exists());
        assert!(!root.join("Music/A/Al/._new.mp3").exists());
        assert!(wrote.exists());
        assert!(root.join("Music/A/Al/._old.mp3").exists());
        assert!(root.join("Music/A/Al/._other.mp3").exists());
        assert!(root.join("Music/A/Al/._short.mp3").exists());
        assert!(root.join("Music/A/Al/old.mp3").exists());
        assert!(root.join(".diskos/keep").exists());
        Ok(())
    }

    #[test]
    fn a_clean_device_is_left_alone() -> TestResult {
        let dev = tempfile::tempdir()?;
        put(&dev.path().join("Music/A/Al/x.opus"), b"x")?;
        clean_up(dev.path(), &[])?;
        assert!(dev.path().join("Music/A/Al/x.opus").exists());
        Ok(())
    }
}
