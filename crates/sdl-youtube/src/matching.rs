//! Matching a wanted song against `YouTube` Music (F3): `innertube-rs` `search_music` with the Songs
//! filter, one client per run, and the pass rule.

use std::time::Duration;

use innertube_rs::{Innertube, MusicSearchFilter};
use sdl_core::{Candidate, MatchOutcome, WantedSong};

use crate::YoutubeError;

/// Longest accepted gap between the wanted song's duration and a result's.
const DURATION_TOLERANCE: Duration = Duration::from_secs(3);

/// Two texts match when their Jaro-Winkler similarity is at least this.
const SIMILARITY_MIN: f64 = 0.9;

/// Added to the score when the album titles match.
const ALBUM_BONUS: f64 = 0.5;

/// How many search results are looked at, best-ranked first.
const RESULTS_CHECKED: usize = 5;

/// What the wanted song says about itself.
#[derive(Debug, Clone, Copy)]
struct Wanted<'a> {
    title: &'a str,
    artists: &'a [String],
    album: &'a str,
    duration: Option<Duration>,
}

/// One `YouTube` Music song result (from `MusicTrackItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    video_id: String,
    title: String,
    artists: Vec<String>,
    /// Album title, when the result has one.
    album: Option<String>,
    /// Duration, when the result has one.
    duration: Option<Duration>,
}

/// Whether a bracketed part names a feature or a version: it holds one of the words `feat`,
/// `featuring`, `ft`, `with`, `remaster`, `remastered`, `version` or `edit`.
fn names_feature_or_version(inner: &str) -> bool {
    inner.split(|c: char| !c.is_alphanumeric()).any(|word| {
        matches!(
            word,
            "feat" | "featuring" | "ft" | "with" | "remaster" | "remastered" | "version" | "edit"
        )
    })
}

/// Lower-case `text`, drop bracketed parts that name a feature or version, drop punctuation and
/// collapse whitespace.
fn normalise(text: &str) -> String {
    let mut base = String::new();
    let mut open: Vec<String> = Vec::new();
    for c in text.to_lowercase().chars() {
        match c {
            '(' | '[' | '{' => open.push(String::new()),
            ')' | ']' | '}' => {
                if let Some(inner) = open.pop()
                    && !names_feature_or_version(&inner)
                {
                    let target = open.last_mut().unwrap_or(&mut base);
                    target.push(' ');
                    target.push_str(&inner);
                    target.push(' ');
                }
            }
            _ => open.last_mut().unwrap_or(&mut base).push(c),
        }
    }
    for rest in open {
        base.push(' ');
        base.push_str(&rest);
    }
    let kept: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `short` is a whole-word prefix of `long`: every word of `short` equals the word at the
/// same place in `long`, and `long` has more words.
fn is_whole_word_prefix(short: &str, long: &str) -> bool {
    let mut short_words = short.split_whitespace();
    let mut long_words = long.split_whitespace();
    loop {
        match (short_words.next(), long_words.next()) {
            (Some(a), Some(b)) if a == b => {}
            (None, Some(_)) => return true,
            _ => return false,
        }
    }
}

/// Whether two normalised titles match: similarity at least [`SIMILARITY_MIN`], or the shorter is a
/// whole-word prefix of the longer. An empty text matches nothing.
fn titles_match(a: &str, b: &str) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && (strsim::jaro_winkler(a, b) >= SIMILARITY_MIN
            || is_whole_word_prefix(a, b)
            || is_whole_word_prefix(b, a))
}

/// The similarity of two normalised artist or album names when they match (at least
/// [`SIMILARITY_MIN`], by similarity only: no substring rule), else `None`. An empty text matches
/// nothing.
fn close_match(a: &str, b: &str) -> Option<f64> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let similarity = strsim::jaro_winkler(a, b);
    (similarity >= SIMILARITY_MIN).then_some(similarity)
}

/// The score of one result, or `None` when it does not pass (F3).
fn score(wanted: &Wanted<'_>, found: &Found) -> Option<f64> {
    let duration = found.duration?;
    if duration.abs_diff(wanted.duration?) > DURATION_TOLERANCE {
        return None;
    }
    let want_title = normalise(wanted.title);
    let found_title = normalise(&found.title);
    if !titles_match(&want_title, &found_title) {
        return None;
    }
    let found_artists: Vec<String> = found.artists.iter().map(|a| normalise(a)).collect();
    let mut best_artist: Option<f64> = None;
    for want in wanted.artists.iter().map(|a| normalise(a)) {
        for have in &found_artists {
            if let Some(similarity) = close_match(&want, have) {
                best_artist = Some(best_artist.map_or(similarity, |best| best.max(similarity)));
            }
        }
    }
    let artist = best_artist?;
    let album = found.album.as_deref().map_or(0.0, |album| {
        if close_match(&normalise(wanted.album), &normalise(album)).is_some() {
            ALBUM_BONUS
        } else {
            0.0
        }
    });
    Some(strsim::jaro_winkler(&want_title, &found_title) + artist + album)
}

/// The match rule (F3), a pure function: among the first [`RESULTS_CHECKED`] of `found`, score each
/// and return the index of the best that passes, or `None` (unmatched). Ties go to the earlier
/// result.
///
/// A result passes when its duration is within 3 s of the wanted song's, its title matches and at least
/// one artist matches. Text is compared after [`normalise`]; titles match when
/// `strsim::jaro_winkler` is at least 0.9 or the shorter is a whole-word prefix of the longer;
/// artists match by similarity alone; empty text matches nothing. Score = title similarity +
/// best artist similarity + 0.5 when the album titles match.
fn best_match(wanted: &Wanted<'_>, found: &[Found]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, candidate) in found.iter().take(RESULTS_CHECKED).enumerate() {
        if let Some(value) = score(wanted, candidate)
            && best.is_none_or(|(_, top)| value > top)
        {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index)
}

impl Found {
    fn to_candidate(&self) -> Candidate {
        Candidate {
            video_id: self.video_id.clone(),
            title: self.title.clone(),
            artists: self.artists.clone(),
            album: self.album.clone(),
            duration: self.duration,
        }
    }
}

/// One `innertube-rs` client, built once per run and shared by every song.
pub struct Matcher {
    client: Innertube,
}

impl std::fmt::Debug for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matcher").finish_non_exhaustive()
    }
}

impl Matcher {
    /// Build the client (`Innertube::new`).
    ///
    /// # Errors
    /// [`YoutubeError::Search`] when the client cannot start up.
    pub async fn new() -> Result<Self, YoutubeError> {
        let client = Innertube::new()
            .await
            .map_err(|e| YoutubeError::Search(e.to_string()))?;
        Ok(Self { client })
    }

    /// Match `song`: query the ISRC first when there is one; if no result passes, query
    /// `<first artist> <title>`. A result passes by [`best_match`]; the best passing result of the
    /// first query that has one is [`MatchOutcome::Matched`].
    ///
    /// When nothing passes the song is [`MatchOutcome::Held`], carrying up to 5 results found
    /// (the ISRC query's first, then the text query's, each once) and a plain reason. A song with
    /// no known duration cannot be checked, so it is held too (the reason says so). A search that
    /// fails is [`MatchOutcome::Failed`], never held.
    pub async fn find(&self, song: &WantedSong) -> MatchOutcome {
        let wanted = Wanted {
            title: &song.title,
            artists: &song.artists,
            album: song.album.as_deref().unwrap_or_default(),
            duration: song.duration,
        };
        let mut seen: Vec<Found> = Vec::new();
        let first_artist = song.artists.first().map_or("", String::as_str);
        let text_query = format!("{first_artist} {}", song.title);
        let queries = song
            .isrc
            .iter()
            .map(String::as_str)
            .chain([text_query.as_str()]);
        for query in queries {
            match self.search(query).await {
                Ok(results) => {
                    if let Some(hit) = settle(&wanted, &mut seen, results) {
                        return MatchOutcome::Matched(hit);
                    }
                }
                Err(error) => return MatchOutcome::Failed(error.to_string()),
            }
        }
        held(&wanted, &seen)
    }

    async fn search(&self, query: &str) -> Result<Vec<Found>, YoutubeError> {
        let results = self
            .client
            .search_music(query, Some(MusicSearchFilter::Songs))
            .await
            .map_err(|e| YoutubeError::Search(e.to_string()))?;
        Ok(results
            .songs
            .into_iter()
            .take(RESULTS_CHECKED)
            .map(|song| Found {
                video_id: song.video_id,
                title: song.title,
                artists: song.artists.into_iter().map(|a| a.name).collect(),
                album: song.album.map(|a| a.title),
                duration: song.duration_ms.map(Duration::from_millis),
            })
            .collect())
    }
}

/// One round of the rule (pure): the best passing result of `results` as a candidate, or `None`
/// after adding them to `seen` (the candidates [`held`] shows when no round passes).
fn settle(wanted: &Wanted<'_>, seen: &mut Vec<Found>, results: Vec<Found>) -> Option<Candidate> {
    if let Some(hit) = best_match(wanted, &results).and_then(|index| results.get(index)) {
        return Some(hit.to_candidate());
    }
    merge_found(seen, results);
    None
}

/// The outcome when no round passed (pure): [`MatchOutcome::Held`] with the results seen (the ISRC
/// query's first, then the text query's, each once, at most 5) and the plain reason.
fn held(wanted: &Wanted<'_>, seen: &[Found]) -> MatchOutcome {
    MatchOutcome::Held {
        reason: held_reason(wanted, seen),
        candidates: seen.iter().map(Found::to_candidate).collect(),
    }
}

/// Add `results` to `seen`, skipping video IDs already there, up to [`RESULTS_CHECKED`] in all.
fn merge_found(seen: &mut Vec<Found>, results: Vec<Found>) {
    for result in results {
        if seen.len() >= RESULTS_CHECKED {
            break;
        }
        if !seen.iter().any(|have| have.video_id == result.video_id) {
            seen.push(result);
        }
    }
}

/// The plain reason a song is held.
fn held_reason(wanted: &Wanted<'_>, seen: &[Found]) -> String {
    if seen.is_empty() {
        "YouTube Music found no songs for this search.".to_owned()
    } else if wanted.duration.is_none() {
        "The song's length is not known, so no result could be checked against it.".to_owned()
    } else {
        "No result matched the title, the artist and the length closely enough.".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|&n| n.to_owned()).collect()
    }

    fn found(title: &str, artists: &[&str], album: Option<&str>, secs: u64) -> Found {
        Found {
            video_id: "id".into(),
            title: title.into(),
            artists: names(artists),
            album: album.map(str::to_owned),
            duration: Some(Duration::from_secs(secs)),
        }
    }

    fn wanted(artists: &[String]) -> Wanted<'_> {
        Wanted {
            title: "Get Lucky",
            artists,
            album: "Random Access Memories",
            duration: Some(Duration::from_secs(369)),
        }
    }

    #[test]
    fn normalise_drops_feature_and_version_brackets_only() {
        assert_eq!(
            normalise("Get Lucky (feat. Pharrell Williams and Nile Rodgers)"),
            "get lucky"
        );
        assert_eq!(normalise("Song [Remastered 2011]"), "song");
        assert_eq!(normalise("Song (Radio Edit)"), "song");
        assert_eq!(normalise("Song (ft. X) (Live)"), "song live");
        assert_eq!(normalise("Don't Stop!"), "dont stop");
        assert_eq!(normalise("Song (Featuring X)"), "song");
        assert_eq!(normalise("Song (2009 Remaster)"), "song");
        assert_eq!(normalise("Song (Live [feat. X])"), "song live");
        assert_eq!(normalise("Song (Unclosed"), "song unclosed");
    }

    #[test]
    fn brackets_that_only_start_like_a_keyword_are_kept() {
        assert_eq!(normalise("Birds (Feather Mix)"), "birds feather mix");
        assert_eq!(normalise("Night (Editor's Cut)"), "night editors cut");
        assert_eq!(normalise("Love (Without You)"), "love without you");
    }

    #[test]
    fn a_bracketed_part_that_is_not_a_feature_still_counts() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Get Lucky (Without You)", &["Daft Punk"], None, 369)];
        let mut spotify = wanted(&artists);
        spotify.title = "Get Lucky (Daft Punk Remix)";
        assert_eq!(best_match(&spotify, &results), None);
    }

    #[test]
    fn get_lucky_matches_its_featuring_title() {
        let artists = names(&["Daft Punk"]);
        let results = [found(
            "Get Lucky (feat. Pharrell Williams and Nile Rodgers)",
            &["Daft Punk", "Pharrell Williams", "Nile Rodgers"],
            Some("Random Access Memories"),
            369,
        )];
        assert_eq!(best_match(&wanted(&artists), &results), Some(0));
    }

    #[test]
    fn live_version_with_another_length_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Get Lucky (Live Version)", &["Daft Punk"], None, 452)];
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn cover_by_another_artist_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Get Lucky", &["Midnight Cover Band"], None, 369)];
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn wrong_duration_is_rejected_and_three_seconds_passes() {
        let artists = names(&["Daft Punk"]);
        let far = [found("Get Lucky", &["Daft Punk"], None, 373)];
        let near = [found("Get Lucky", &["Daft Punk"], None, 372)];
        assert_eq!(best_match(&wanted(&artists), &far), None);
        assert_eq!(best_match(&wanted(&artists), &near), Some(0));
    }

    #[test]
    fn missing_duration_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let mut result = found("Get Lucky", &["Daft Punk"], None, 369);
        result.duration = None;
        assert_eq!(best_match(&wanted(&artists), &[result]), None);
    }

    #[test]
    fn unrelated_title_is_rejected() {
        let artists = names(&["Daft Punk"]);
        let results = [found("Around the World", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn one_matching_artist_is_enough() {
        let artists = names(&["Pharrell Williams", "Daft Punk"]);
        let results = [found("Get Lucky", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&wanted(&artists), &results), Some(0));
    }

    #[test]
    fn album_match_breaks_a_tie() {
        let artists = names(&["Daft Punk"]);
        let results = [
            found("Get Lucky", &["Daft Punk"], Some("Greatest Hits"), 369),
            found(
                "Get Lucky",
                &["Daft Punk"],
                Some("Random Access Memories"),
                369,
            ),
        ];
        assert_eq!(best_match(&wanted(&artists), &results), Some(1));
    }

    #[test]
    fn equal_scores_keep_the_earlier_result() {
        let artists = names(&["Daft Punk"]);
        let results = [
            found("Get Lucky", &["Daft Punk"], None, 369),
            found("Get Lucky", &["Daft Punk"], None, 369),
        ];
        assert_eq!(best_match(&wanted(&artists), &results), Some(0));
    }

    #[test]
    fn only_the_first_five_results_are_checked() {
        let artists = names(&["Daft Punk"]);
        let mut results: Vec<Found> = (0..RESULTS_CHECKED)
            .map(|_| found("Other Song", &["Someone"], None, 100))
            .collect();
        results.push(found("Get Lucky", &["Daft Punk"], None, 369));
        assert_eq!(best_match(&wanted(&artists), &results), None);
    }

    #[test]
    fn empty_normalised_text_matches_nothing() {
        let artists = names(&["Daft Punk"]);
        let mut spotify = wanted(&artists);
        spotify.title = "(feat. X)";
        let results = [found("Get Lucky", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&spotify, &results), None);
    }

    fn matches(title: &str, artist: &str, result: &Found) -> bool {
        let artists = names(&[artist]);
        let mut spotify = wanted(&artists);
        spotify.title = title;
        best_match(&spotify, std::slice::from_ref(result)).is_some()
    }

    fn plain_get_lucky() -> Found {
        found("Get Lucky", &["Daft Punk"], None, 369)
    }

    #[test]
    fn tribute_band_is_not_the_artist() {
        let tribute = found("Get Lucky", &["Daft Punk Tribute Band"], None, 369);
        assert!(!matches("Get Lucky", "Daft Punk", &tribute));
        assert!(!matches(
            "Get Lucky",
            "Daft Punk Tribute Band",
            &plain_get_lucky()
        ));
    }

    #[test]
    fn artist_inside_a_longer_name_is_not_a_match() {
        let result = found("Get Lucky", &["The Daft Punk Experience"], None, 369);
        assert!(!matches("Get Lucky", "Daft Punk", &result));
    }

    #[test]
    fn remaster_suffix_matches_bracketed_or_dashed_on_either_side() {
        let dashed = found("Get Lucky - 2013 Remaster", &["Daft Punk"], None, 369);
        assert!(matches("Get Lucky", "Daft Punk", &dashed));
        let bracketed = found("Get Lucky (2013 Remaster)", &["Daft Punk"], None, 369);
        assert!(matches("Get Lucky", "Daft Punk", &bracketed));
        assert!(matches(
            "Get Lucky [Remastered 2013]",
            "Daft Punk",
            &plain_get_lucky()
        ));
    }

    #[test]
    fn featured_artist_suffix_matches_on_either_side() {
        let result = found(
            "Get Lucky (feat. Pharrell Williams)",
            &["Daft Punk"],
            None,
            369,
        );
        assert!(matches("Get Lucky", "Daft Punk", &result));
        assert!(matches(
            "Get Lucky (ft. Pharrell Williams)",
            "Daft Punk",
            &plain_get_lucky()
        ));
    }

    #[test]
    fn a_short_title_inside_a_long_one_needs_a_whole_word_prefix() {
        let mid = found("Crazy in Love", &["Beyonce"], None, 369);
        assert!(!matches("Love", "Beyonce", &mid));
        let partial_word = found("Heyday", &["Beyonce"], None, 369);
        assert!(!matches("Hey", "Beyonce", &partial_word));
        let prefix = found("Love Me Do", &["Beyonce"], None, 369);
        assert!(matches("Love", "Beyonce", &prefix));
        let short = found("Love", &["Beyonce"], None, 369);
        assert!(matches("Love Me Do", "Beyonce", &short));
    }

    #[test]
    fn a_whole_word_prefix_title_with_the_wrong_length_is_rejected() {
        let artists = names(&["Beyonce"]);
        let mut spotify = wanted(&artists);
        spotify.title = "Love";
        let results = [found("Love Me Do", &["Beyonce"], None, 150)];
        assert_eq!(best_match(&spotify, &results), None);
    }

    #[test]
    fn live_version_is_left_to_the_duration_check() {
        let live = found("Get Lucky (Live)", &["Daft Punk"], None, 369);
        assert!(matches("Get Lucky", "Daft Punk", &live));
        let long_live = found("Get Lucky (Live)", &["Daft Punk"], None, 452);
        assert!(!matches("Get Lucky", "Daft Punk", &long_live));
    }

    #[test]
    fn empty_artist_text_matches_nothing() {
        let blank = found("Get Lucky", &["", "(feat. X)"], None, 369);
        assert!(!matches("Get Lucky", "", &blank));
        assert!(!matches("Get Lucky", "Daft Punk", &blank));
    }

    #[test]
    fn merging_dedupes_and_stops_at_five() {
        let make = |id: &str| Found {
            video_id: id.into(),
            ..found("t", &["a"], None, 1)
        };
        let mut seen = vec![make("a"), make("b")];
        merge_found(&mut seen, ["b", "c", "d", "e", "f"].map(make).to_vec());
        let ids: Vec<&str> = seen.iter().map(|f| f.video_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn an_unknown_wanted_length_passes_nothing() {
        let artists = names(&["Daft Punk"]);
        let mut spotify = wanted(&artists);
        spotify.duration = None;
        let results = [found("Get Lucky", &["Daft Punk"], None, 369)];
        assert_eq!(best_match(&spotify, &results), None);
        assert!(held_reason(&spotify, &results).contains("not known"));
        assert!(held_reason(&spotify, &[]).contains("no songs"));
    }

    fn with_id(id: &str, mut result: Found) -> Found {
        result.video_id = id.into();
        result
    }

    #[test]
    fn a_passing_first_round_matches_without_a_second() {
        let artists = names(&["Daft Punk"]);
        let mut seen = Vec::new();
        let hit = settle(
            &wanted(&artists),
            &mut seen,
            vec![with_id("isrc-hit", plain_get_lucky())],
        );
        assert_eq!(hit.map(|c| c.video_id), Some("isrc-hit".to_owned()));
        assert_eq!(seen, Vec::new());
    }

    #[test]
    fn nothing_passing_in_two_rounds_is_held_with_both_rounds_once_each() -> Result<(), String> {
        let artists = names(&["Daft Punk"]);
        let wanted = wanted(&artists);
        let off = |id: &str| with_id(id, found("Other", &["X"], None, 10));
        let mut seen = Vec::new();
        assert!(settle(&wanted, &mut seen, vec![off("a"), off("b")]).is_none());
        assert!(settle(&wanted, &mut seen, vec![off("b"), off("c")]).is_none());
        let (candidates, reason) = held_parts(held(&wanted, &seen))?;
        let ids: Vec<&str> = candidates.iter().map(|c| c.video_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(reason.contains("closely enough"));
        Ok(())
    }

    #[test]
    fn a_text_round_can_rescue_an_isrc_round_that_found_nothing_usable() {
        let artists = names(&["Daft Punk"]);
        let wanted = wanted(&artists);
        let mut seen = Vec::new();
        assert!(settle(&wanted, &mut seen, Vec::new()).is_none());
        let hit = settle(&wanted, &mut seen, vec![plain_get_lucky()]);
        assert!(hit.is_some());
    }

    #[test]
    fn nothing_found_at_all_is_held_with_no_candidates() -> Result<(), String> {
        let artists = names(&["Daft Punk"]);
        let (candidates, reason) = held_parts(held(&wanted(&artists), &[]))?;
        assert_eq!(candidates, Vec::new());
        assert!(reason.contains("no songs"));
        Ok(())
    }

    fn held_parts(outcome: MatchOutcome) -> Result<(Vec<Candidate>, String), String> {
        match outcome {
            MatchOutcome::Held { candidates, reason } => Ok((candidates, reason)),
            other => Err(format!("expected held, got {other:?}")),
        }
    }
}
