//! The per-song pipeline with bounded concurrency (`Config::download_jobs`).

use std::path::Path;
use std::sync::Arc;

use color_eyre::eyre::eyre;
use futures::StreamExt;
use sdl_core::{Config, Event, EventSender, Stage, Status, Track, emit, paths};
use sdl_library::Enrichment;
use sdl_spotify::SpotifyError;
use sdl_youtube::Client;
use tokio::task::JoinHandle;

use crate::app::Action;

/// Start the worker for `action`. The worker reports on `events` and always ends with
/// [`Event::Finished`]; an error ends the run as [`Event::Error`]. Returns `None` for actions that
/// start no work.
#[must_use]
pub fn spawn(action: Action, config: Config, events: EventSender) -> Option<JoinHandle<()>> {
    let handle = match action {
        Action::SyncSpotify => tokio::spawn(async move {
            let result = sync_spotify(&config, &events).await;
            finish(&events, result);
        }),
        Action::DownloadLink(url) => tokio::spawn(async move {
            let result = download_link(&config, &url, &events).await;
            finish(&events, result);
        }),
        Action::SyncCard => tokio::spawn(async move {
            let result = sync_card(&config, &events).await;
            finish(&events, result);
        }),
        Action::FinishCard(mount) => tokio::spawn(async move {
            let result = finish_card(&config, &mount, &events).await;
            finish(&events, result);
        }),
        Action::None | Action::Back => return None,
    };
    Some(handle)
}

/// Report the end of a run. A send fails only when the UI has gone, so the result is ignored.
/// Only the outer message is shown: every error type here already includes its cause in its text.
fn finish(events: &EventSender, result: color_eyre::Result<()>) {
    if let Err(error) = result {
        emit(events, Event::Error(error.to_string()));
    }
    emit(events, Event::Finished);
}

/// Sync Spotify: sign in, list songs, then per song skip if the file exists, match, download, look up
/// genre and write tags. Adds, never deletes. Reports on `events`.
///
/// # Errors
/// Listing ended the run. A failed sign-in is already shown as its sign-in state and returns `Ok`.
/// Per-song failures are events, not errors. A Spotify rate limit ends the run as
/// [`Event::RateLimited`] and returns `Ok`.
async fn sync_spotify(config: &Config, events: &EventSender) -> color_eyre::Result<()> {
    let session = match sdl_spotify::sign_in(&config.client_id, events).await {
        Ok(session) => session,
        Err(error) => {
            // A failed sign-in is already shown as its sign-in state; a 429 also gets its wait line.
            rate_limited(events, error);
            return Ok(());
        }
    };
    let tracks = match sdl_spotify::list_tracks(&session, events).await {
        Ok(tracks) => tracks,
        Err(error) => {
            return rate_limited(events, error).map_or(Ok(()), |error| {
                Err(eyre!("Listing Spotify songs failed: {error}"))
            });
        }
    };
    let total = tracks.len();
    emit(events, Event::Listed(tracks.clone()));
    emit(events, Event::Progress { done: 0, total });
    emit(
        events,
        Event::Activity(format!("{total} songs listed, downloading the new ones")),
    );
    let client = Arc::new(Client::new().await?);

    let mut done = 0usize;
    let mut results = futures::stream::iter(tracks)
        .map(|track| {
            let client = Arc::clone(&client);
            async move { process_track(config, &client, &track, events).await }
        })
        .buffer_unordered(config.download_jobs.get());
    while results.next().await.is_some() {
        done = done.saturating_add(1);
        emit(events, Event::Progress { done, total });
    }
    emit(events, Event::Activity("Spotify sync finished".to_owned()));
    Ok(())
}

/// A Spotify 429 ends the run as [`Event::RateLimited`] (`None` returned); any other error is
/// handed back.
fn rate_limited(events: &EventSender, error: SpotifyError) -> Option<SpotifyError> {
    if let SpotifyError::RateLimited { retry_after } = error {
        emit(events, Event::RateLimited { wait: retry_after });
        None
    } else {
        Some(error)
    }
}

/// Run one song through the pipeline. A failure is reported as the song's status and never stops
/// the run.
async fn process_track(config: &Config, client: &Client, track: &Track, events: &EventSender) {
    let key = track.key();
    let report = |stage: Stage, status: Status| {
        emit(
            events,
            Event::Song {
                key: key.clone(),
                stage,
                status,
            },
        );
    };
    let dest = paths::spotify_track_path(&config.music_root, track);
    if matches!(tokio::fs::try_exists(&dest).await, Ok(true)) {
        report(Stage::Listed, Status::SkippedExists);
        return;
    }

    report(Stage::Matching, Status::InProgress);
    let candidate = match client.find_match(track).await {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            report(Stage::Matching, Status::Unmatched);
            return;
        }
        Err(error) => {
            report(Stage::Matching, Status::Failed(error.to_string()));
            return;
        }
    };

    report(Stage::Downloading, Status::InProgress);
    if let Err(error) = client.download(&candidate, &dest).await {
        report(Stage::Downloading, Status::Failed(error.to_string()));
        return;
    }

    report(Stage::Enriching, Status::InProgress);
    // A failed lookup never fails the song: its tags stay empty, and the reason is shown.
    let enrichment = match sdl_library::lookup(track).await {
        Ok(enrichment) => enrichment,
        Err(error) => {
            emit(events, Event::Activity(format!("{}: {error}", track.title)));
            Enrichment::default()
        }
    };

    report(Stage::Tagging, Status::InProgress);
    if let Err(error) = sdl_library::write_tags(&dest, track, &enrichment).await {
        report(Stage::Tagging, Status::Failed(error.to_string()));
        return;
    }
    report(Stage::Tagging, Status::Done);
}

/// Download one pasted link (track or playlist) into the library.
///
/// # Errors
/// The downloader could not start, or the download failed.
async fn download_link(config: &Config, url: &str, events: &EventSender) -> color_eyre::Result<()> {
    let client = Client::new().await?;
    let files = client
        .download_link(url, &config.music_root, events)
        .await
        .map_err(|error| eyre!("Downloading the link failed: {error}"))?;
    emit(
        events,
        Event::Activity(format!("Downloaded {} file(s)", files.len())),
    );
    Ok(())
}

/// Sync the card: mount, copy add-only, then send [`Event::CardSynced`] with the mount point and
/// return. Cleanup and unmount wait for the user's confirmation ([`finish_card`]).
///
/// # Errors
/// Mounting or copying failed.
async fn sync_card(config: &Config, events: &EventSender) -> color_eyre::Result<()> {
    emit(events, Event::Activity("Mounting the card".to_owned()));
    let uuid = config.card_uuid.clone();
    let mount = tokio::task::spawn_blocking(move || sdl_library::mount_card(&uuid)).await??;

    emit(
        events,
        Event::Activity(format!(
            "Copying to {}",
            paths::card_music_dir(&mount).display()
        )),
    );
    let music_root = config.music_root.clone();
    let copy_mount = mount.clone();
    let sender = events.clone();
    let report = tokio::task::spawn_blocking(move || {
        sdl_library::sync_card(&music_root, &copy_mount, &sender)
    })
    .await??;
    emit(
        events,
        Event::Activity(format!(
            "Copied {} file(s), {} already on the card",
            report.copied, report.skipped
        )),
    );
    emit(events, Event::CardSynced { mount });
    Ok(())
}

/// After the user confirms: remove the macOS dot-folders from the card root, then unmount it.
///
/// # Errors
/// Removing a dot-folder or unmounting failed.
async fn finish_card(
    config: &Config,
    card_mount: &Path,
    events: &EventSender,
) -> color_eyre::Result<()> {
    let mount = card_mount.to_path_buf();
    let uuid = config.card_uuid.clone();
    tokio::task::spawn_blocking(move || {
        sdl_library::clean_dot_folders(&mount)?;
        sdl_library::unmount_card(&uuid)
    })
    .await??;
    emit(
        events,
        Event::Activity("Card cleaned and unmounted".to_owned()),
    );
    Ok(())
}
