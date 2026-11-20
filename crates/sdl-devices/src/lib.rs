//! Devices (F10): the mounted external or removable volumes, plans for copying To and From a
//! device, the copy runs, device playlists, and clean-up and eject.
//!
//! Uses `sdl-library` through its public interface: the converter (`converter::convert_file`,
//! `converter::parallelism`, `converter::output_size`), playlist writing
//! (`playlists::write_for_destination`) and tag reading (`tags::place_from_tags`). Drives are found and ejected with macOS `diskutil`; nothing is
//! mounted or formatted, and nothing is deleted except the macOS items named in
//! [`clean_up_and_eject`].
//!
//! Names that start with a dot are never copied, in either direction.

use std::path::PathBuf;

mod eject;
mod paths;
mod plan;
#[cfg(test)]
mod reality;
mod run;
mod volumes;

pub use eject::clean_up_and_eject;
pub use plan::{DevicePlan, place_incoming, plan};
pub use run::run;
pub use volumes::{DeviceWatch, folders, list, watch};

/// Errors from the devices crate. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum DevicesError {
    /// `diskutil` could not be run or reported failure.
    #[error("diskutil {action} failed: {message}")]
    Diskutil {
        /// Sub-command.
        action: &'static str,
        /// Reason.
        message: String,
    },
    /// Watching `/Volumes` failed.
    #[error("cannot watch for devices: {0}")]
    Watch(String),
    /// The device has no room for the next file. The files already complete stay.
    #[error("{device} is full")]
    Full {
        /// Device name.
        device: String,
    },
    /// The device went away during the run. The files already complete stay.
    #[error("{device} was removed during the copy")]
    Gone {
        /// Device name.
        device: String,
    },
    /// File system failure.
    #[error("{path}: {source}")]
    Io {
        /// Path concerned.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// A failure in `sdl-library` (converter, playlists, tags).
    #[error(transparent)]
    Library(#[from] sdl_library::LibraryError),
}

/// Test helpers shared by the modules' tests.
#[cfg(test)]
mod testing {
    use std::path::Path;

    /// A small, valid MP3 file: an `ID3v2.3` tag with the given text frames (none when all are
    /// `None`), then 20 silent MPEG-1 layer 3 frames.
    pub(crate) fn mp3(title: Option<&str>, artist: Option<&str>, album: Option<&str>) -> Vec<u8> {
        let mut frames = Vec::new();
        for (id, text) in [("TIT2", title), ("TPE1", artist), ("TALB", album)] {
            if let Some(text) = text {
                let body_len = u32::try_from(text.len().saturating_add(1)).unwrap_or(0);
                frames.extend_from_slice(id.as_bytes());
                frames.extend_from_slice(&body_len.to_be_bytes());
                frames.extend_from_slice(&[0, 0, 0]);
                frames.extend_from_slice(text.as_bytes());
            }
        }
        let mut file = Vec::new();
        if !frames.is_empty() {
            let size = u32::try_from(frames.len()).unwrap_or(0);
            let syncsafe = [
                u8::try_from((size >> 21) & 0x7f).unwrap_or(0),
                u8::try_from((size >> 14) & 0x7f).unwrap_or(0),
                u8::try_from((size >> 7) & 0x7f).unwrap_or(0),
                u8::try_from(size & 0x7f).unwrap_or(0),
            ];
            file.extend_from_slice(b"ID3\x03\x00\x00");
            file.extend_from_slice(&syncsafe);
            file.extend_from_slice(&frames);
        }
        for _ in 0..20 {
            file.extend_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
            file.extend_from_slice(&[0_u8; 413]);
        }
        file
    }

    /// Write `content` at `path`, making its folders.
    pub(crate) fn put(path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }
}
