//! Per-song pipeline position and outcome.

/// Where a song is in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Listed by Spotify, nothing done yet.
    Listed,
    /// Searching `YouTube` Music for a match.
    Matching,
    /// Downloading the matched audio.
    Downloading,
    /// Looking up genre and disc total.
    Enriching,
    /// Writing tags and cover.
    Tagging,
}

/// Outcome of the song so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The song is still moving through the pipeline.
    InProgress,
    /// The file already exists; skipped before any network call.
    SkippedExists,
    /// No `YouTube` Music candidate passed the match rules; nothing was downloaded.
    Unmatched,
    /// Downloaded and tagged.
    Done,
    /// The song failed, with the reason.
    Failed(String),
}
