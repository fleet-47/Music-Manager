//! Sync Spotify (F2): sign in, offer the lists, take the ticked ones, list the songs, report
//! `N new of M`, then on go handle each new song.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use sdl_core::paths::spotify_target;
use sdl_core::{Attention, Counts, EventKind, Format, ListChoice, Track};
use sdl_library::{ReleaseCache, lookup, write_tags};
use sdl_spotify::Session;
use sdl_youtube::{MatchOutcome, Matcher, TagsFrom, YoutubeError, download_audio};

use super::TaskError;
use super::form::{Answered, Answers, Step};
use super::progress::{Outcome, download_dir, drive, place, summary_line, with_already};
use super::runner::{Flow, TaskContext};

/// Sync Spotify as a step form.
pub(super) struct Spotify<'a> {
    context: &'a TaskContext,
    session: Option<Session>,
    /// Songs of the ticked lists that are not in the library yet.
    new: Vec<Track>,
    /// Songs of the ticked lists that already are, in any format.
    already: usize,
    format: Format,
}

impl<'a> Spotify<'a> {
    pub(super) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            session: None,
            new: Vec::new(),
            already: 0,
            format: Format::default(),
        }
    }

    /// List the songs of the chosen lists, set aside those already present in any format (before
    /// any network call about them), and send `NewOfTotal`.
    async fn plan(&mut self, session: &Session, chosen: &[ListChoice]) -> Result<(), TaskError> {
        let events = &self.context.events;
        events.send(EventKind::Headline("Listing your songs".to_owned()));
        let tracks = sdl_spotify::list_songs(session, chosen, events).await?;
        let total = tracks.len();
        let root = &self.context.config.music_root;
        let mut new = Vec::new();
        for track in tracks {
            if !spotify_target(root, &track).is_present()? {
                new.push(track);
            }
        }
        self.already = total.saturating_sub(new.len());
        events.send(EventKind::NewOfTotal {
            new: new.len(),
            total,
        });
        self.new = new;
        Ok(())
    }
}

impl Step for Spotify<'_> {
    async fn open(&mut self) -> Result<(), TaskError> {
        let events = &self.context.events;
        events.send(EventKind::Headline("Signing in to Spotify".to_owned()));
        let session = sdl_spotify::sign_in(&self.context.config.client_id, events).await?;
        events.send(EventKind::Headline("Reading your lists".to_owned()));
        events.send(EventKind::Lists(sdl_spotify::list_choices(&session).await?));
        self.session = Some(session);
        Ok(())
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::Spotify { lists, format } = answers else {
            return Ok(Answered::Ignored);
        };
        let Some(session) = self.session.take() else {
            return Ok(Answered::Ignored);
        };
        self.context
            .save_ticked(lists.iter().map(|list| list.id.clone()).collect())?;
        self.context.remember_format(format)?;
        self.format = format.format;
        let planned = self.plan(&session, &lists).await;
        self.session = Some(session);
        planned?;
        Ok(Answered::Waiting { can_start: true })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let tracks = std::mem::take(&mut self.new);
        let no_genre = AtomicUsize::new(0);
        let counts = sync(self.context, tracks, self.format, &no_genre).await?;
        let line = summary_line(&with_already(counts, self.already));
        Ok(Flow::done(
            with_no_genre(line, no_genre.into_inner()),
            &counts,
        ))
    }
}

/// The last-run line with the songs added without a genre (F7), when there are any.
fn with_no_genre(line: String, no_genre: usize) -> String {
    if no_genre == 0 {
        line
    } else {
        format!("{line}, {} without a genre", crate::fmt::thousands(no_genre))
    }
}

/// Handle every new song, `download_jobs` at a time. Songs added without a genre are counted in
/// `no_genre`.
async fn sync(
    context: &TaskContext,
    tracks: Vec<Track>,
    format: Format,
    no_genre: &AtomicUsize,
) -> Result<Counts, TaskError> {
    let events = &context.events;
    if tracks.is_empty() {
        events.send(EventKind::Counts(Counts::default()));
        return Ok(Counts::default());
    }
    events.send(EventKind::Headline(
        "Finding songs on YouTube Music".to_owned(),
    ));
    let matcher = Matcher::new().await?;
    let cache = ReleaseCache::default();
    events.send(EventKind::Headline(
        "Downloading and tagging songs".to_owned(),
    ));
    let root = &context.config.music_root;
    let total = tracks.len();
    let outcomes = futures::stream::iter(tracks)
        .map(|track| song(root, &matcher, &cache, track, format, no_genre))
        .buffer_unordered(context.config.download_jobs.get());
    drive(outcomes, total, events).await
}

/// One song: match; download to a temp file; look up; tag the temp file; rename last. Only a
/// missing `yt-dlp` ends the run; every other failure is the song's.
async fn song(
    root: &Path,
    matcher: &Matcher,
    cache: &ReleaseCache,
    track: Track,
    format: Format,
    no_genre: &AtomicUsize,
) -> Result<Outcome, TaskError> {
    let failed =
        |reason: String| Attention::failed(track.title.clone(), track.artists_joined(), reason);
    let target = spotify_target(root, &track);
    let candidate = match matcher.find_match(&track).await {
        Ok(MatchOutcome::Matched(candidate)) => candidate,
        Ok(MatchOutcome::Unmatched) => {
            return Ok(Outcome::Unmatched(Attention::unmatched(
                track.title.clone(),
                track.artists_joined(),
                "No match on YouTube Music.".to_owned(),
            )));
        }
        Err(error) => return Ok(Outcome::Failed(failed(error.to_string()))),
    };
    // `_dir` is removed with everything in it when this function returns or is dropped.
    let (_dir, file) = match download_dir(root, format) {
        Ok(made) => made,
        Err(error) => return Ok(Outcome::Failed(failed(error.to_string()))),
    };
    match download_audio(&candidate.url(), &file, format, TagsFrom::Caller).await {
        Ok(()) => {}
        Err(YoutubeError::YtDlpNotFound) => {
            return Err(TaskError::Youtube(YoutubeError::YtDlpNotFound));
        }
        Err(error) => return Ok(Outcome::Failed(failed(error.to_string()))),
    }
    // A failed or empty lookup leaves the genre and totals empty; the song still completes.
    let enrichment = lookup(&track, cache).await.unwrap_or_default();
    if let Err(error) = write_tags(&file, &track, &enrichment).await {
        return Ok(Outcome::Failed(failed(error.to_string())));
    }
    match place(&file, &target.path(format)).await {
        Ok(Outcome::Added) => {
            if enrichment.genre.is_none() {
                no_genre.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Outcome::Added)
        }
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(Outcome::Failed(failed(error.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_without_a_genre_are_named_only_when_there_are_some() {
        assert_eq!(
            with_no_genre("2 added".into(), 0),
            "2 added"
        );
        assert_eq!(
            with_no_genre("2 added".into(), 1),
            "2 added, 1 without a genre"
        );
    }
}
