//! The outcome of matching one wanted song against `YouTube` Music (F3).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::storage::opt_secs;

/// One `YouTube` Music song result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The video ID. The URL is `https://music.youtube.com/watch?v=<video_id>`
    /// ([`Candidate::url`]).
    pub video_id: String,
    /// Result title.
    pub title: String,
    /// Result artists.
    #[serde(default)]
    pub artists: Vec<String>,
    /// Result album, when it has one.
    #[serde(default)]
    pub album: Option<String>,
    /// Result duration, to whole seconds, when it has one.
    #[serde(default, with = "opt_secs")]
    pub duration: Option<Duration>,
}

impl Candidate {
    /// `https://music.youtube.com/watch?v=<video_id>`, the URL handed to `yt-dlp` and opened by
    /// review's `o`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.video_id)
    }
}

/// How matching one wanted song ended.
///
/// A song is never downloaded on a guess: `Matched` only when a candidate passed the rule, `Held`
/// otherwise. A search that could not be made at all is `Failed`, which is not a held song and is
/// not saved to the review list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// The best candidate that passed the rule.
    Matched(Candidate),
    /// No candidate passed. `candidates` are up to 5 results found (possibly none), shown in
    /// review; `reason` is a plain sentence for the owner.
    Held {
        /// The results found, best-ranked first.
        candidates: Vec<Candidate>,
        /// Why nothing passed, in plain words.
        reason: String,
    },
    /// The search itself failed (network). The reason is plain words.
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_candidate_has_a_music_url() {
        let candidate = Candidate {
            video_id: "abc".into(),
            title: String::new(),
            artists: Vec::new(),
            album: None,
            duration: None,
        };
        assert_eq!(candidate.url(), "https://music.youtube.com/watch?v=abc");
    }
}
