//! The song as listed by Spotify.

use std::time::Duration;

/// One Spotify song, as listed from Liked Songs or an owned playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Song title.
    pub title: String,
    /// Artists, in Spotify's order.
    pub artists: Vec<String>,
    /// Album title.
    pub album: String,
    /// Album artist(s), joined with `, `.
    pub album_artist: String,
    /// Track number on its disc.
    pub track_number: u32,
    /// Disc number.
    pub disc_number: u32,
    /// Release date as Spotify reports it (`YYYY`, `YYYY-MM` or `YYYY-MM-DD`), when present.
    pub release_date: Option<String>,
    /// ISRC, when Spotify provides one.
    pub isrc: Option<String>,
    /// Duration according to Spotify.
    pub duration: Duration,
    /// Album cover image URL, when Spotify provides one.
    pub cover_url: Option<String>,
    /// Spotify track id.
    pub spotify_id: String,
    /// Name of the list the song came from (`Saved tracks` for Liked Songs).
    pub list_name: String,
    /// Id of that list ([`crate::event::LIKED_SONGS_ID`] for Liked Songs); the path fallback when
    /// the name is empty after sanitising.
    pub list_id: String,
}

impl Track {
    /// Artists joined with `, `, as used in file names.
    #[must_use]
    pub fn artists_joined(&self) -> String {
        self.artists.join(", ")
    }
}
