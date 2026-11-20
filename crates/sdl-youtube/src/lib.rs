//! `YouTube` Music matching (`innertube-rs`), link listing and downloading (the `yt-dlp` binary,
//! run directly with `tokio::process::Command`).
//!
//! - [`matching`] (F3): a wanted song against `YouTube` Music; built from the earlier design and
//!   adapted to return candidates.
//! - [`download`] (F3): a match's audio as Opus, or a link's own audio as served.
//! - [`link`] (F5): what a pasted link contains, as wanted songs.
//!
//! Bodies marked "not built yet" return [`YoutubeError::NotBuilt`]; their doc comments are the
//! contract.

pub mod download;
pub mod link;
pub mod matching;

pub use download::{Downloaded, download_opus, download_own_audio};
pub use link::{LinkItem, LinkListing, PlaylistInfo, is_music_link, list_link, read_item};
pub use matching::Matcher;

/// Errors from the `YouTube` crate. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum YoutubeError {
    /// This part of the contract has a signature but no body yet.
    #[error("{0} is not built yet")]
    NotBuilt(&'static str),
    /// The `YouTube` Music search client could not be made.
    #[error("YouTube Music search failed: {0}")]
    Search(String),
    /// The `yt-dlp` binary could not be started because it is not on `PATH`.
    #[error("yt-dlp was not found on PATH")]
    YtDlpNotFound,
    /// `yt-dlp` ran and failed; carries the last `ERROR:` line of its stderr.
    #[error("yt-dlp failed: {0}")]
    YtDlp(String),
    /// `yt-dlp`'s JSON listing could not be read.
    #[error("could not read yt-dlp's listing: {0}")]
    Listing(String),
    /// File system or process failure.
    #[error("file or process error: {0}")]
    Io(#[from] std::io::Error),
}
