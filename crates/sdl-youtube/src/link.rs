//! What a pasted link contains (F5), as wanted songs.

use sdl_core::WantedSong;
use url::Url;

use crate::YoutubeError;

/// A playlist link's name and id as `yt-dlp` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInfo {
    /// Playlist title; the playlist file is named after it (F7).
    pub name: String,
    /// Playlist id, the file-name fallback.
    pub id: String,
}

/// One item of a flat listing. A flat listing can lack fields, so only the URL is relied on;
/// [`read_item`] reads the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkItem {
    /// The item's own URL.
    pub url: String,
    /// Title in the flat listing (may be empty).
    pub title: String,
    /// Video id (may be empty).
    pub id: String,
}

/// What a link contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkListing {
    /// Set for a playlist link; its items are in list order.
    pub playlist: Option<PlaylistInfo>,
    /// The items, in order.
    pub items: Vec<LinkItem>,
}

/// List a link: `yt-dlp --dump-single-json --flat-playlist -- <url>` gives the items and, for a
/// playlist, its name and order. A link that is a single video is a listing of one item and no
/// playlist.
///
/// # Errors
/// [`YoutubeError::NotBuilt`] until built; then [`YoutubeError::YtDlpNotFound`],
/// [`YoutubeError::YtDlp`] or [`YoutubeError::Listing`].
pub async fn list_link(url: &str) -> Result<LinkListing, YoutubeError> {
    let _ = url;
    std::future::ready(Err(YoutubeError::NotBuilt("listing a link"))).await
}

/// Read one item's own metadata at its turn, with
/// `yt-dlp --dump-single-json --no-playlist -- <item url>`, as a wanted song: title is `track`,
/// else `title`; artist is `artist`, else `uploader`; `album`; `duration`; cover is `thumbnail`.
/// The song's source is a link item with `direct` set by [`is_music_link`].
///
/// # Errors
/// [`YoutubeError::NotBuilt`] until built; then as [`list_link`].
pub async fn read_item(item: &LinkItem) -> Result<WantedSong, YoutubeError> {
    let _ = item;
    std::future::ready(Err(YoutubeError::NotBuilt("reading a link item"))).await
}

/// Whether `url` is a `music.youtube.com` link: such an item is its own source and downloads
/// without matching. Every other link, plain `YouTube` included, is matched (F3).
#[must_use]
pub fn is_music_link(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host == "music.youtube.com"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_music_youtube_is_its_own_source() {
        assert!(is_music_link("https://music.youtube.com/watch?v=abc"));
        assert!(!is_music_link("https://www.youtube.com/watch?v=abc"));
        assert!(!is_music_link("https://youtu.be/abc"));
        assert!(!is_music_link("https://evil.example/music.youtube.com"));
        assert!(!is_music_link("not a url"));
    }
}
