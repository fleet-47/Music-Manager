//! Sync Spotify (F2): sign in, offer the lists, take the ticked ones, list the songs, report
//! `N new of M`, then on go handle each song.

use std::path::Path;

use futures::StreamExt;
use sdl_core::paths::{remove_tmp_dir_if_empty, spotify_track_path};
use sdl_core::{Attention, EventKind, ListChoice, Track};
use sdl_library::{ReleaseCache, lookup, write_tags};
use sdl_youtube::{MatchOutcome, Matcher, YoutubeError, download_audio};
use tokio::sync::mpsc::UnboundedReceiver;

use super::progress::{
    Outcome, download_dir, drive, place, summary_line, unless_cancelled, wait_for_choices,
    wait_for_go,
};
use super::runner::{Flow, TaskContext};
use super::{Command, TaskError};

/// Run the whole Spotify flow, waiting on `commands` at the two steps that need the owner.
pub(super) async fn run(
    context: &TaskContext,
    mut commands: UnboundedReceiver<Command>,
) -> Result<Flow, TaskError> {
    let events = &context.events;
    events.send(EventKind::Headline("Signing in to Spotify".to_owned()));
    let sign_in = sdl_spotify::sign_in(&context.config.client_id, events);
    let Some(session) = unless_cancelled(&mut commands, sign_in).await else {
        return Ok(Flow::Cancelled);
    };
    let session = session?;
    events.send(EventKind::Headline("Reading your lists".to_owned()));
    let Some(lists) = unless_cancelled(&mut commands, sdl_spotify::list_choices(&session)).await
    else {
        return Ok(Flow::Cancelled);
    };
    events.send(EventKind::Lists(lists?));

    let Some(chosen) = wait_for_choices(&mut commands).await else {
        return Ok(Flow::Cancelled);
    };
    context.save_ticked(chosen.iter().map(|list| list.id.clone()).collect())?;
    let Some(tracks) = unless_cancelled(&mut commands, plan(context, &session, &chosen)).await
    else {
        return Ok(Flow::Cancelled);
    };
    let tracks = tracks?;

    if !wait_for_go(&mut commands).await {
        return Ok(Flow::Cancelled);
    }
    let counts = sync(context, tracks).await?;
    Ok(Flow::Done {
        line: summary_line(&counts),
    })
}

/// List the songs of the chosen lists and send `NewOfTotal`. Returns every listed song.
async fn plan(
    context: &TaskContext,
    session: &sdl_spotify::Session,
    chosen: &[ListChoice],
) -> Result<Vec<Track>, TaskError> {
    context
        .events
        .send(EventKind::Headline("Listing songs".to_owned()));
    let tracks = sdl_spotify::list_songs(session, chosen, &context.events).await?;
    let mut new = 0_usize;
    for track in &tracks {
        let path = spotify_track_path(&context.config.music_root, track);
        if !tokio::fs::try_exists(&path).await? {
            new = new.saturating_add(1);
        }
    }
    context.events.send(EventKind::NewOfTotal {
        new,
        total: tracks.len(),
    });
    Ok(tracks)
}

/// Handle every song, `download_jobs` at a time.
async fn sync(context: &TaskContext, tracks: Vec<Track>) -> Result<sdl_core::Counts, TaskError> {
    let events = &context.events;
    events.send(EventKind::Headline(
        "Starting the YouTube Music search".to_owned(),
    ));
    let matcher = Matcher::new().await?;
    let cache = ReleaseCache::default();
    events.send(EventKind::Headline("Downloading and tagging".to_owned()));
    let root = &context.config.music_root;
    let total = tracks.len();
    let outcomes = futures::stream::iter(tracks)
        .map(|track| song(root, &matcher, &cache, track))
        .buffer_unordered(context.config.download_jobs.get());
    let result = drive(outcomes, total, events).await;
    // `drive` has dropped every song in flight, so their temp files are gone.
    let cleanup = remove_tmp_dir_if_empty(root);
    let counts = result?;
    cleanup?;
    Ok(counts)
}

/// One song: exists? skip; match; download to a temp file; look up; tag the temp file; rename last.
/// Only a missing `yt-dlp` ends the run; every other failure is the song's.
async fn song(
    root: &Path,
    matcher: &Matcher,
    cache: &ReleaseCache,
    track: Track,
) -> Result<Outcome, TaskError> {
    let attention = |reason: String| Attention {
        title: track.title.clone(),
        artist: track.artists_joined(),
        reason,
    };
    let dest = spotify_track_path(root, &track);
    match tokio::fs::try_exists(&dest).await {
        Ok(true) => return Ok(Outcome::Skipped),
        Ok(false) => {}
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    }
    let candidate = match matcher.find_match(&track).await {
        Ok(MatchOutcome::Matched(candidate)) => candidate,
        Ok(MatchOutcome::Unmatched) => {
            return Ok(Outcome::Unmatched(attention(
                "No match on YouTube Music.".to_owned(),
            )));
        }
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    };
    // `_dir` is removed with everything in it when this function returns or is dropped.
    let (_dir, file) = match download_dir(root) {
        Ok(made) => made,
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    };
    match download_audio(&candidate.url(), &file).await {
        Ok(()) => {}
        Err(YoutubeError::YtDlpNotFound) => {
            return Err(TaskError::Youtube(YoutubeError::YtDlpNotFound));
        }
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    }
    // A failed or empty lookup leaves the genre and totals empty; the song still completes.
    let enrichment = lookup(&track, cache).await.unwrap_or_default();
    if let Err(error) = write_tags(&file, &track, &enrichment).await {
        return Ok(Outcome::Failed(attention(error.to_string())));
    }
    match place(&file, &dest).await {
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(Outcome::Failed(attention(error.to_string()))),
    }
}
