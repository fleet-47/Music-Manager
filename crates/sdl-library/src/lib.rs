//! What the library does to files and tags (F6, F7, F9): genre and totals from `MusicBrainz`, tag
//! writing with `lofty`, playlist files, the `ffmpeg` converter and the Convert function.
//!
//! Modules: [`tags`] (F6), [`playlists`] (F7), [`converter`] and [`convert`] (F9). Devices (F10)
//! are the `sdl-devices` crate, which uses this crate's converter and playlist writer.

pub mod convert;
pub mod converter;
pub mod playlists;
pub mod tags;

#[cfg(test)]
mod testkit;

use std::path::PathBuf;

/// Errors from the library crate. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// `MusicBrainz` request failed.
    #[error("MusicBrainz lookup failed: {0}")]
    MusicBrainz(String),
    /// Cover download failed.
    #[error("cover download failed: {0}")]
    Cover(#[from] reqwest::Error),
    /// Tag read or write failed.
    #[error("tag error on {path}: {message}")]
    Tag {
        /// File concerned.
        path: PathBuf,
        /// Reason.
        message: String,
    },
    /// File system failure.
    #[error("{path}: {source}")]
    Io {
        /// Path concerned.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// A background task failed to run to completion.
    #[error("background task failed: {0}")]
    Task(String),
    /// The `ffmpeg` binary could not be started because it is not on `PATH`.
    #[error("ffmpeg was not found on PATH")]
    FfmpegNotFound,
    /// `ffmpeg` ran and failed; carries its last error line.
    #[error("ffmpeg could not convert {}: {message}", path.display())]
    Ffmpeg {
        /// The file being converted.
        path: PathBuf,
        /// Reason.
        message: String,
    },
}

pub(crate) fn io_err(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> LibraryError {
    let path = path.to_path_buf();
    move |source| LibraryError::Io { path, source }
}
