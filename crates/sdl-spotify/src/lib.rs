//! Spotify sign-in (PKCE) and track listing, via `rspotify`.
//!
//! # Web API calls in a full listing
//!
//! With `L` Liked Songs, `P` playlists the user can see and `n_i` items in the i-th playlist the
//! user owns, every page holds 50 entries (rspotify's default `pagination_chunks`) and a listing makes
//!
//! `1 (current user) + max(1, ceil(L/50)) + max(1, ceil(P/50)) + sum over owned playlists of max(1, ceil(n_i/50))`
//!
//! calls. The sign-in itself makes one more, to Spotify's token endpoint (accounts, not the Web API).
//! Example: 2,117 liked songs (43 pages), 30 playlists of which 20 are owned and each fits one page:
//! 1 + 43 + 1 + 20 = 65 calls.

use std::net::SocketAddr;
use std::time::Duration;

use futures::TryStreamExt;
use rspotify::clients::pagination::Paginator;
use rspotify::clients::{BaseClient, OAuthClient};
use rspotify::http::HttpError;
use rspotify::model::{FullTrack, PlayableItem};
use rspotify::{
    AuthCodePkceSpotify, ClientError, ClientResult, Config, Credentials, OAuth, scopes,
};
use sdl_core::paths::SAVED_TRACKS;
use sdl_core::{Event, EventSender, SignInState, Track, emit};

/// Redirect URI registered on the Spotify app; the local listener binds its port.
const REDIRECT_URI: &str = "http://127.0.0.1:9900/";
/// Local listener address matching [`REDIRECT_URI`].
const LISTENER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 9900);
/// Items per activity update; matches rspotify's default page size (`pagination_chunks`, 50).
const PAGE: usize = 50;
/// HTTP status for rate limiting.
const TOO_MANY_REQUESTS: u16 = 429;

/// Errors from the Spotify crate.
#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    /// Spotify answered HTTP 429 (surfaced by `rspotify` as `ClientError::Http(HttpError::StatusCode)`).
    /// The run ends; there is no retry. `retry_after` is the `Retry-After` header, `None` when absent.
    #[error("Spotify rate limit reached")]
    RateLimited {
        /// How long Spotify asks the client to wait.
        retry_after: Option<Duration>,
    },
    /// The browser could not be opened for sign-in.
    #[error("could not open the browser: {0}")]
    Browser(#[from] std::io::Error),
    /// The sign-in listener task failed to run.
    #[error("sign-in listener task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    /// Any other `rspotify` failure.
    #[error("Spotify error: {0}")]
    Api(ClientError),
}

impl From<ClientError> for SpotifyError {
    fn from(error: ClientError) -> Self {
        if let ClientError::Http(http) = &error
            && let HttpError::StatusCode(response) = &**http
            && response.status().as_u16() == TOO_MANY_REQUESTS
        {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_retry_after);
            return Self::RateLimited { retry_after };
        }
        Self::Api(error)
    }
}

/// `Retry-After` as delay-seconds. The HTTP-date form is not parsed and yields `None`.
fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// A signed-in Spotify session (`rspotify::AuthCodePkceSpotify`, `token_cached = false`). The token
/// lives in memory only.
#[derive(Debug)]
pub struct Session {
    client: AuthCodePkceSpotify,
}

/// Sign in with PKCE using the config's client ID: open the authorize URL in the browser and receive
/// the code on rspotify's local listener at `http://127.0.0.1:9900/` (inside `spawn_blocking`).
/// Reports [`sdl_core::SignInState`] changes on `events`.
///
/// # Errors
/// Sign-in failed or was cancelled.
pub async fn sign_in(client_id: &str, events: &EventSender) -> Result<Session, SpotifyError> {
    let result = sign_in_inner(client_id, events).await;
    let state = match &result {
        Ok(_) => SignInState::SignedIn,
        Err(error) => SignInState::Failed(error.to_string()),
    };
    emit(events, Event::SignIn(state));
    result
}

async fn sign_in_inner(client_id: &str, events: &EventSender) -> Result<Session, SpotifyError> {
    let oauth = OAuth {
        redirect_uri: REDIRECT_URI.to_owned(),
        scopes: scopes!("user-library-read", "playlist-read-private"),
        ..OAuth::default()
    };
    let config = Config {
        token_cached: false,
        ..Config::default()
    };
    let mut client =
        AuthCodePkceSpotify::with_config(Credentials::new_pkce(client_id), oauth, config);
    let url = client.get_authorize_url(None)?;

    emit(events, Event::SignIn(SignInState::OpeningBrowser));
    open::that_detached(&url)?;

    emit(events, Event::SignIn(SignInState::WaitingForBrowser));
    let listener = client.clone();
    let code = tokio::task::spawn_blocking(move || listener.get_authcode_listener(LISTENER_ADDR))
        .await??;
    client.request_token(&code).await?;
    Ok(Session { client })
}

/// Map a full track to [`Track`]; local files (no Spotify id) are skipped.
fn to_track(track: &FullTrack, list_name: &str) -> Option<Track> {
    if track.is_local {
        return None;
    }
    let id = track.id.as_ref()?;
    Some(Track {
        title: track.name.clone(),
        artists: track.artists.iter().map(|a| a.name.clone()).collect(),
        album: track.album.name.clone(),
        album_artist: track
            .album
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        track_number: track.track_number,
        disc_number: u32::try_from(track.disc_number).unwrap_or(1),
        release_date: track.album.release_date.clone(),
        isrc: track.external_ids.get("isrc").cloned(),
        duration: track.duration.to_std().unwrap_or_default(),
        cover_url: track.album.images.first().map(|image| image.url.clone()),
        spotify_id: rspotify::prelude::Id::id(id).to_owned(),
        list_name: list_name.to_owned(),
    })
}

/// Drain one paginated listing into `tracks`, sending an activity line per page received.
async fn drain<T>(
    items: Paginator<'_, ClientResult<T>>,
    name: &str,
    events: &EventSender,
    tracks: &mut Vec<Track>,
    to_row: impl Fn(T) -> Option<Track>,
) -> Result<(), SpotifyError> {
    let mut pages = items.try_chunks(PAGE);
    emit(
        events,
        Event::Activity(format!("Listing {name} ({} so far)", tracks.len())),
    );
    while let Some(page) = pages.try_next().await.map_err(|error| error.1)? {
        tracks.extend(page.into_iter().filter_map(&to_row));
        emit(
            events,
            Event::Activity(format!("Listing {name} ({} so far)", tracks.len())),
        );
    }
    Ok(())
}

/// List Liked Songs (list name [`sdl_core::paths::SAVED_TRACKS`]) and the playlists the user owns,
/// one [`Track`] per list entry, through rspotify's paginators (50 per page, its default
/// `pagination_chunks`). Sends [`sdl_core::Event::Activity`] while listing.
///
/// # Errors
/// An API or network failure, or [`SpotifyError::RateLimited`].
pub async fn list_tracks(
    session: &Session,
    events: &EventSender,
) -> Result<Vec<Track>, SpotifyError> {
    let client = &session.client;
    let me = client.current_user().await?;
    let mut tracks = Vec::new();

    drain(
        client.current_user_saved_tracks(None),
        SAVED_TRACKS,
        events,
        &mut tracks,
        |saved| to_track(&saved.track, SAVED_TRACKS),
    )
    .await?;

    emit(events, Event::Activity("Listing playlists".to_owned()));
    let owned: Vec<_> = client
        .current_user_playlists()
        .try_filter(|playlist| std::future::ready(playlist.owner.id == me.id))
        .try_collect()
        .await?;

    for playlist in owned {
        drain(
            client.playlist_items(playlist.id.as_ref(), None, None),
            &playlist.name,
            events,
            &mut tracks,
            |entry| match entry.item {
                Some(PlayableItem::Track(track)) => to_track(&track, &playlist.name),
                _ => None,
            },
        )
        .await?;
    }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds() -> Result<(), String> {
        let wait = parse_retry_after(" 42 ").ok_or("expected seconds")?;
        assert_eq!(wait, Duration::from_secs(42));
        Ok(())
    }

    #[test]
    fn retry_after_http_date_is_none() {
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }
}
