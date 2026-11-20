//! Spotify sign-in (PKCE) and list/song listing, via `rspotify`.
//!
//! # Web API calls
//!
//! Every page holds 50 entries (rspotify's default `pagination_chunks`). Calls per step, for `P`
//! playlists in the account and `n` songs in a ticked list:
//!
//! - Sign-in: 0 Web API calls (one request to Spotify's accounts service for the token).
//! - Choosing lists: `1` (`me`) `+ 1` (first saved-tracks page, limit 1, for the Liked Songs count)
//!   `+ max(1, ceil(P/50))` (`current_user_playlists`; each entry already carries its track total).
//! - Listing songs: `max(1, ceil(n/50))` per ticked list.
//!
//! No per-song call is made: track, artist and album endpoints are never called.

use std::time::{Duration, SystemTime};

use futures::TryStreamExt;
use rspotify::clients::pagination::Paginator;
use rspotify::clients::{BaseClient, OAuthClient};
use rspotify::http::HttpError;
use rspotify::model::{FullTrack, PlayableItem, PlaylistId};
use rspotify::prelude::Id;
use rspotify::scopes;
use rspotify::{AuthCodePkceSpotify, ClientError, ClientResult};
use rspotify::{Config, Credentials, OAuth};
use sdl_core::event::LIKED_SONGS_ID;
use sdl_core::paths::SAVED_TRACKS;
use sdl_core::{EventKind, EventSender, ListChoice, SignInState, Track};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use url::Url;

/// Redirect URI registered on the Spotify app; the local listener binds its port.
const REDIRECT_URI: &str = "http://127.0.0.1:9900/";
/// Items per page and per headline update; rspotify's default `pagination_chunks`.
const PAGE: usize = 50;
/// HTTP status for rate limiting.
const TOO_MANY_REQUESTS: u16 = 429;
/// HTTP status for a refused request.
const FORBIDDEN: u16 = 403;
/// Where the sign-in listener binds; the port of [`REDIRECT_URI`].
const LISTEN_ADDR: &str = "127.0.0.1:9900";
/// Scopes requested: the library and private playlists, read only.
const SCOPES: &str = "user-library-read playlist-read-private";
/// The most of a request the listener reads; the request line is all it needs.
const MAX_REQUEST: usize = 8192;
/// The page shown in the browser after a successful redirect.
const DONE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>sdl</title>\
<p>Signed in. You can close this tab and return to sdl.</p>";
/// The page shown in the browser after a redirect carrying `error=`.
const REFUSED_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>sdl</title>\
<p>Sign-in did not complete. You can close this tab and return to sdl.</p>";

/// Errors from the Spotify crate.
#[derive(Debug, thiserror::Error)]
pub enum SpotifyError {
    /// Spotify answered HTTP 429 (`ClientError::Http(HttpError::StatusCode)`). The task ends; there is
    /// no retry. `retry_after` is the `Retry-After` header (seconds or an HTTP date), `None` when absent.
    #[error("Spotify rate limit reached")]
    RateLimited {
        /// How long Spotify asks the client to wait.
        retry_after: Option<Duration>,
    },
    /// Spotify answered HTTP 403; `reason` is the reason in the response body.
    #[error("Spotify refused the request: {reason}")]
    Forbidden {
        /// Spotify's reason.
        reason: String,
    },
    /// A list id could not be used as a Spotify playlist id.
    #[error("not a Spotify playlist id: {0}")]
    BadListId(String),
    /// The browser could not be opened for sign-in.
    #[error("could not open the browser: {0}")]
    Browser(std::io::Error),
    /// The browser redirect carried `error=` (for example `access_denied` when access is refused).
    #[error("Spotify sign-in did not complete: {0}")]
    SignInRefused(String),
    /// The sign-in listener could not bind or accept on `127.0.0.1:9900`.
    #[error("could not listen on {LISTEN_ADDR}: {0}")]
    Listen(std::io::Error),
    /// Any other `rspotify` failure.
    #[error("Spotify error: {0}")]
    Api(ClientError),
}

/// Turn a failed rspotify call into the crate's error. A 429 reads `Retry-After`; a 403 reads
/// Spotify's reason from the response body (`{"error":{"message":..}}`, else the body text).
async fn classify(error: ClientError) -> SpotifyError {
    let ClientError::Http(http) = error else {
        return SpotifyError::Api(error);
    };
    let HttpError::StatusCode(response) = *http else {
        return SpotifyError::Api(ClientError::Http(http));
    };
    match response.status().as_u16() {
        TOO_MANY_REQUESTS => {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, SystemTime::now()));
            SpotifyError::RateLimited { retry_after }
        }
        FORBIDDEN => {
            let body = response.text().await.unwrap_or_default();
            SpotifyError::Forbidden {
                reason: reason_from_body(&body),
            }
        }
        _ => SpotifyError::Api(ClientError::Http(Box::new(HttpError::StatusCode(response)))),
    }
}

/// Spotify's reason in an error body: `error.message`, else the trimmed body text.
fn reason_from_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| json.pointer("/error/message")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| body.trim().to_owned())
}

/// Await an rspotify call and classify its failure.
async fn api<T>(call: impl Future<Output = ClientResult<T>>) -> Result<T, SpotifyError> {
    match call.await {
        Ok(value) => Ok(value),
        Err(error) => Err(classify(error).await),
    }
}

/// `Retry-After` as delay-seconds or as an HTTP date (parsed with `httpdate`) relative to `now`.
/// A date in the past is a zero wait; anything unreadable is `None`.
fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(now).unwrap_or_default())
}

/// A signed-in Spotify session (`rspotify::AuthCodePkceSpotify`, `token_cached = false`). The token
/// lives in memory only.
#[derive(Debug)]
pub struct Session {
    client: AuthCodePkceSpotify,
}

/// Sign in with PKCE using the config's client ID (F2): authorize URL opened with `open`, code
/// received on our own `tokio::net::TcpListener` at `127.0.0.1:9900`, then `request_token`.
/// Reports [`SignInState`] changes as [`EventKind::SignIn`] on `events`. The listener is dropped
/// on every way out (success, failure, and the task being aborted), so the port is always freed.
///
/// # Errors
/// Sign-in failed; the cause is also sent as [`SignInState::Failed`].
pub async fn sign_in(client_id: &str, events: &EventSender) -> Result<Session, SpotifyError> {
    events.send(EventKind::SignIn(SignInState::OpeningBrowser));
    match authorize(client_id, events).await {
        Ok(client) => {
            events.send(EventKind::SignIn(SignInState::SignedIn));
            Ok(Session { client })
        }
        Err(error) => {
            events.send(EventKind::SignIn(SignInState::Failed(error.to_string())));
            Err(error)
        }
    }
}

/// The PKCE flow: authorize URL, browser, code on the listener, token.
async fn authorize(
    client_id: &str,
    events: &EventSender,
) -> Result<AuthCodePkceSpotify, SpotifyError> {
    let oauth = OAuth {
        redirect_uri: REDIRECT_URI.to_owned(),
        scopes: scopes!(SCOPES),
        ..OAuth::default()
    };
    let config = Config {
        token_cached: false,
        ..Config::default()
    };
    let mut client =
        AuthCodePkceSpotify::with_config(Credentials::new_pkce(client_id), oauth, config);
    let url = client.get_authorize_url(None).map_err(SpotifyError::Api)?;
    let listener = TcpListener::bind(LISTEN_ADDR)
        .await
        .map_err(SpotifyError::Listen)?;
    open::that_detached(&url).map_err(SpotifyError::Browser)?;
    events.send(EventKind::SignIn(SignInState::WaitingForBrowser));
    let code = wait_for_code(&listener, &client).await?;
    drop(listener);
    api(client.request_token(&code)).await?;
    Ok(client)
}

/// Accept connections until one carries a request with `code=` that rspotify's
/// `parse_response_code` accepts (state included), answer it with the "you can close this tab"
/// page and return the code. A request carrying `error=` ends sign-in as
/// [`SpotifyError::SignInRefused`] with that reason. Each connection is read on its own task, so an idle speculative
/// connection from the browser never blocks the real one; the tasks end with this future.
async fn wait_for_code(
    listener: &TcpListener,
    client: &AuthCodePkceSpotify,
) -> Result<String, SpotifyError> {
    let mut readers: JoinSet<Option<(TcpStream, String)>> = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(SpotifyError::Listen)?;
                readers.spawn(read_request_target(stream));
            }
            Some(Ok(Some((mut stream, target)))) = readers.join_next() => {
                let url = format!("http://{LISTEN_ADDR}{target}");
                // A failed reply below only means the tab went away; the outcome is already known.
                if let Some(code) = client.parse_response_code(&url) {
                    let _ = respond(&mut stream, "200 OK", DONE_PAGE).await;
                    return Ok(code);
                }
                if let Some(reason) = redirect_error(&url) {
                    let _ = respond(&mut stream, "200 OK", REFUSED_PAGE).await;
                    return Err(SpotifyError::SignInRefused(reason));
                }
                let _ = respond(&mut stream, "400 Bad Request", "").await;
            }
        }
    }
}

/// The `error` query parameter of a redirect URL, when present.
fn redirect_error(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "error")
        .map(|(_, value)| value.into_owned())
}

/// Read the request line of one connection. A request whose target carries `code=` or `error=` is
/// returned with its stream for the caller to answer; any other request is answered 404 here.
async fn read_request_target(mut stream: TcpStream) -> Option<(TcpStream, String)> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buffer.contains(&b'\n') && buffer.len() < MAX_REQUEST {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(chunk.get(..read)?);
    }
    let text = String::from_utf8_lossy(&buffer);
    let target = text.lines().next()?.split_whitespace().nth(1)?.to_owned();
    if target.contains("code=") || target.contains("error=") {
        return Some((stream, target));
    }
    let _ = respond(&mut stream, "404 Not Found", "").await;
    None
}

/// Write a complete HTTP/1.1 response and close the connection.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

/// The lists offered for ticking: Liked Songs (count from the first saved-tracks page) and each
/// playlist the signed-in user owns (`me()`), with its song count.
///
/// # Errors
/// An API or network failure, or [`SpotifyError::RateLimited`].
pub async fn list_choices(session: &Session) -> Result<Vec<ListChoice>, SpotifyError> {
    let client = &session.client;
    let me = api(client.current_user()).await?;
    let first = api(client.current_user_saved_tracks_manual(None, Some(1), Some(0))).await?;
    let mut choices = vec![ListChoice {
        name: "Liked Songs".to_owned(),
        id: LIKED_SONGS_ID.to_owned(),
        count: usize::try_from(first.total).unwrap_or(usize::MAX),
    }];
    let owned: Vec<_> = match client
        .current_user_playlists()
        .try_filter(|playlist| std::future::ready(playlist.owner.id == me.id))
        .try_collect()
        .await
    {
        Ok(owned) => owned,
        Err(error) => return Err(classify(error).await),
    };
    choices.extend(owned.into_iter().map(|playlist| ListChoice {
        name: playlist.name,
        id: playlist.id.id().to_owned(),
        count: usize::try_from(playlist.items.total).unwrap_or(usize::MAX),
    }));
    Ok(choices)
}

/// Map a full track to [`Track`]; local files (no Spotify id) are skipped.
fn to_track(track: &FullTrack, list: &ListChoice) -> Option<Track> {
    if track.is_local {
        return None;
    }
    let id = track.id.as_ref()?;
    let list_name = if list.id == LIKED_SONGS_ID {
        SAVED_TRACKS
    } else {
        &list.name
    };
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
        spotify_id: id.id().to_owned(),
        list_name: list_name.to_owned(),
        list_id: list.id.clone(),
    })
}

/// Drain one paginated listing into `tracks`, sending a headline per page received.
async fn drain<T>(
    items: Paginator<'_, ClientResult<T>>,
    list: &ListChoice,
    events: &EventSender,
    tracks: &mut Vec<Track>,
    to_row: impl Fn(T) -> Option<Track>,
) -> Result<(), SpotifyError> {
    let mut pages = items.try_chunks(PAGE);
    events.send(EventKind::Headline(format!(
        "Listing {} ({} so far)",
        list.name,
        tracks.len()
    )));
    loop {
        let page = match pages.try_next().await {
            Ok(Some(page)) => page,
            Ok(None) => break,
            Err(error) => return Err(classify(error.1).await),
        };
        tracks.extend(page.into_iter().filter_map(&to_row));
        events.send(EventKind::Headline(format!(
            "Listing {} ({} so far)",
            list.name,
            tracks.len()
        )));
    }
    Ok(())
}

/// The songs of the `chosen` lists, one [`Track`] per list entry, through rspotify's paginators
/// (50 per page). Local files and episodes are skipped. Sends [`EventKind::Headline`] while listing.
///
/// # Errors
/// An API or network failure, [`SpotifyError::RateLimited`], or [`SpotifyError::BadListId`].
pub async fn list_songs(
    session: &Session,
    chosen: &[ListChoice],
    events: &EventSender,
) -> Result<Vec<Track>, SpotifyError> {
    let client = &session.client;
    let mut tracks = Vec::new();
    for list in chosen {
        if list.id == LIKED_SONGS_ID {
            drain(
                client.current_user_saved_tracks(None),
                list,
                events,
                &mut tracks,
                |saved| to_track(&saved.track, list),
            )
            .await?;
        } else {
            let id = PlaylistId::from_id(list.id.as_str())
                .map_err(|_| SpotifyError::BadListId(list.id.clone()))?;
            drain(
                client.playlist_items(id, None, None),
                list,
                events,
                &mut tracks,
                |entry| match entry.item {
                    Some(PlayableItem::Track(track)) => to_track(&track, list),
                    _ => None,
                },
            )
            .await?;
        }
    }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds() {
        assert_eq!(
            parse_retry_after(" 42 ", SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(42))
        );
    }

    #[test]
    fn retry_after_http_date_is_relative_to_now() {
        // Thu, 01 Jan 1970 00:01:40 GMT is 100 s after the epoch.
        let date = "Thu, 01 Jan 1970 00:01:40 GMT";
        assert_eq!(
            parse_retry_after(date, SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(100))
        );
    }

    #[test]
    fn retry_after_date_in_the_past_is_zero() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(500);
        assert_eq!(
            parse_retry_after("Thu, 01 Jan 1970 00:01:40 GMT", now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn retry_after_garbage_is_none() {
        assert_eq!(parse_retry_after("soon", SystemTime::UNIX_EPOCH), None);
    }

    #[test]
    fn forbidden_reason_comes_from_the_body() {
        let body =
            r#"{"error":{"status":403,"message":"Check settings on developer.spotify.com"}}"#;
        assert_eq!(
            reason_from_body(body),
            "Check settings on developer.spotify.com"
        );
        assert_eq!(reason_from_body(" plain text \n"), "plain text");
    }

    #[test]
    fn redirect_error_is_read_from_the_query() {
        assert_eq!(
            redirect_error("http://127.0.0.1:9900/?error=access_denied&state=s"),
            Some("access_denied".to_owned())
        );
        assert_eq!(
            redirect_error("http://127.0.0.1:9900/?code=c&state=s"),
            None
        );
    }
}
