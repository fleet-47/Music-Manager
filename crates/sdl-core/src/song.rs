//! The wanted song (F3): what the library is asked to hold, whatever its source.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::storage::opt_secs;

/// Where a wanted song came from. It is also the song's identity in the review list (F8) and the
/// "never download" set (F11): [`WantedSong::key`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SongSource {
    /// A Spotify listing entry (F4). Local files and episodes never become wanted songs.
    Spotify {
        /// The Spotify track ID. Never empty.
        track_id: String,
    },
    /// An item of a pasted link (F5).
    Link {
        /// The item's own URL (`yt-dlp`'s `webpage_url`, else the entry URL). Never empty.
        item_url: String,
        /// The video ID, the path fallback when a name is empty. May be empty when `yt-dlp` gave
        /// none; the item URL is used then.
        video_id: String,
        /// `true` for a `music.youtube.com` item: it is its own source and downloads without
        /// matching (F3). Every other item, plain `YouTube` included, is matched.
        direct: bool,
    },
}

/// Whether `url` is an address on `YouTube` or `YouTube Music`.
fn is_youtube(url: &str) -> bool {
    let host = Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned));
    matches!(
        host.as_deref(),
        Some(
            "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be"
        )
    )
}

/// One song the library should hold, with as much as its source knows (F3). Every field except
/// the title and the source may be missing.
///
/// Invariants: the title is not empty; `artists` may be empty (a link item with no artist);
/// text fields are stored as the source gave them. File names are made from these fields by
/// [`crate::Place::of`], never from `MusicBrainz` (F7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WantedSong {
    /// Where it came from.
    pub source: SongSource,
    /// Song title.
    pub title: String,
    /// Artists, in the source's order.
    #[serde(default)]
    pub artists: Vec<String>,
    /// Album title.
    #[serde(default)]
    pub album: Option<String>,
    /// Album artist(s) joined with `, `.
    #[serde(default)]
    pub album_artist: Option<String>,
    /// Track number on its disc.
    #[serde(default)]
    pub track_number: Option<u32>,
    /// Disc number.
    #[serde(default)]
    pub disc_number: Option<u32>,
    /// Release date as the source reports it (`YYYY`, `YYYY-MM` or `YYYY-MM-DD`).
    #[serde(default)]
    pub date: Option<String>,
    /// ISRC.
    #[serde(default)]
    pub isrc: Option<String>,
    /// Duration, to whole seconds.
    #[serde(default, with = "opt_secs")]
    pub duration: Option<Duration>,
    /// Cover image URL.
    #[serde(default)]
    pub cover_url: Option<String>,
}

impl WantedSong {
    /// The song's identity (F8): the Spotify track ID; for a `YouTube` or `YouTube Music` item its
    /// video ID, so the same video pasted with either address is one song; for any other link item
    /// its URL.
    #[must_use]
    pub fn key(&self) -> &str {
        match &self.source {
            SongSource::Spotify { track_id } => track_id,
            SongSource::Link {
                item_url, video_id, ..
            } if video_id.is_empty() || !is_youtube(item_url) => item_url,
            SongSource::Link { video_id, .. } => video_id,
        }
    }

    /// The ID a path component falls back to when its name is empty: the Spotify track ID, or the
    /// video ID (the item URL when there is none).
    #[must_use]
    pub fn fallback_id(&self) -> &str {
        match &self.source {
            SongSource::Spotify { track_id } => track_id,
            SongSource::Link {
                video_id, item_url, ..
            } => {
                if video_id.is_empty() {
                    item_url
                } else {
                    video_id
                }
            }
        }
    }

    /// Whether the song came from a pasted link (so review offers "Use the link's own audio").
    #[must_use]
    pub const fn is_from_link(&self) -> bool {
        matches!(self.source, SongSource::Link { .. })
    }

    /// Whether the song is its own source and downloads without matching.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        matches!(self.source, SongSource::Link { direct: true, .. })
    }

    /// Artists joined with `, `, for display.
    #[must_use]
    pub fn artists_joined(&self) -> String {
        self.artists.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(source: SongSource) -> WantedSong {
        WantedSong {
            source,
            title: "T".into(),
            artists: vec!["A".into(), "B".into()],
            album: None,
            album_artist: None,
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: None,
            cover_url: None,
        }
    }

    #[test]
    fn identity_and_fallback_per_source() {
        let spotify = song(SongSource::Spotify {
            track_id: "sid".into(),
        });
        assert_eq!(spotify.key(), "sid");
        assert_eq!(spotify.fallback_id(), "sid");
        assert!(!spotify.is_from_link());
        let link = song(SongSource::Link {
            item_url: "https://x/y".into(),
            video_id: String::new(),
            direct: true,
        });
        assert_eq!(link.key(), "https://x/y");
        assert_eq!(link.fallback_id(), "https://x/y");
        assert!(link.is_from_link() && link.is_direct());
        assert_eq!(link.artists_joined(), "A, B");
    }

    #[test]
    fn a_youtube_video_has_one_identity_whichever_address_it_came_with() {
        let link = |item_url: &str, video_id: &str| {
            song(SongSource::Link {
                item_url: item_url.into(),
                video_id: video_id.into(),
                direct: false,
            })
        };
        for url in [
            "https://music.youtube.com/watch?v=abc123",
            "https://www.youtube.com/watch?v=abc123&list=PL1",
            "https://youtube.com/watch?v=abc123",
            "https://m.youtube.com/watch?v=abc123",
            "https://youtu.be/abc123",
        ] {
            assert_eq!(link(url, "abc123").key(), "abc123", "{url}");
        }
        // Any other site, and any address that only mentions YouTube, is its URL.
        for url in [
            "https://soundcloud.com/a/b",
            "https://evil.example/music.youtube.com",
            "https://notyoutube.com/watch?v=abc123",
            "not a url",
        ] {
            assert_eq!(link(url, "abc123").key(), url, "{url}");
        }
        // A YouTube item with no video ID falls back to its URL.
        let bare = "https://www.youtube.com/watch?v=x";
        assert_eq!(link(bare, "").key(), bare);
    }
}
