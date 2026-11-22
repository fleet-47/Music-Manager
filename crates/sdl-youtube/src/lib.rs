//! `YouTube` Music matching (`innertube-rs`), link listing and downloading (the `yt-dlp` binary,
//! run through `sdl_core::process`).
//!
//! - [`matching`] (F3): a wanted song against `YouTube` Music, returning a match or the candidates
//!   held for review.
//! - [`download`] (F3): a match's audio as Opus, or a link's own audio as served.
//! - [`link`] (F5): what a pasted link contains, as wanted songs.
//!
//! `yt-dlp` is run through `sdl_core::process::run`.

mod download;
mod link;
mod matching;

pub use download::{Downloaded, download_opus, download_own_audio};
pub use link::{LinkItem, LinkListing, PlaylistInfo, list_link, read_item};
pub use matching::Matcher;

/// Errors from the `YouTube` crate. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum YoutubeError {
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

impl From<sdl_core::process::ToolError> for YoutubeError {
    fn from(error: sdl_core::process::ToolError) -> Self {
        use sdl_core::process::ToolError;
        match error {
            ToolError::NotFound(_) => Self::YtDlpNotFound,
            ToolError::Failed { reason, .. } => Self::YtDlp(reason),
            ToolError::Io { source, .. } => Self::Io(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_tool_reads_as_not_on_path() {
        let error = YoutubeError::from(sdl_core::process::ToolError::NotFound("yt-dlp".to_owned()));
        assert_eq!(error.to_string(), "yt-dlp was not found on PATH");
    }
}
