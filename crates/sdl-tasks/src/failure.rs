//! A failed task as the event the UI shows, and as the last-run line.

use sdl_core::{ErrorInfo, EventKind};
use sdl_devices::DevicesError;
use sdl_library::LibraryError;
use sdl_spotify::SpotifyError;
use sdl_youtube::YoutubeError;

use crate::TaskError;

/// The terminal event for a task that ended on `error`: `RateLimited` for a Spotify 429, otherwise
/// `Error` with what went wrong and what to do.
pub(crate) fn failure_event(error: &TaskError) -> EventKind {
    if let TaskError::Spotify(SpotifyError::RateLimited { retry_after }) = error {
        return EventKind::RateLimited { wait: *retry_after };
    }
    EventKind::Error(error_info(error))
}

/// What went wrong and what to do about it, in plain words.
pub(crate) fn error_info(error: &TaskError) -> ErrorInfo {
    const NETWORK: &str = "Check the internet connection, then try again.";
    const DISK: &str =
        "Check that the folder named above exists and has free space, then try again.";
    let advice = match error {
        TaskError::Youtube(YoutubeError::YtDlpNotFound)
        | TaskError::Library(LibraryError::FfmpegNotFound)
        | TaskError::Devices(DevicesError::Library(LibraryError::FfmpegNotFound)) => {
            "Start sdl with `mise run tui` so it is on PATH."
        }
        TaskError::Youtube(YoutubeError::YtDlp(_) | YoutubeError::Listing(_)) => {
            "Check the link, then try again. If it keeps failing, run `mise upgrade` to update yt-dlp."
        }
        TaskError::Youtube(YoutubeError::Search(_))
        | TaskError::Spotify(SpotifyError::Api(_))
        | TaskError::Library(LibraryError::MusicBrainz(_)) => NETWORK,
        TaskError::Spotify(SpotifyError::Forbidden { .. }) => {
            "Check the app's settings on the Spotify developer dashboard, then try again."
        }
        TaskError::Spotify(SpotifyError::RateLimited { .. }) => {
            "Run Sync Spotify again after the wait."
        }
        TaskError::Spotify(SpotifyError::Browser(_)) => {
            "Check that macOS has a default web browser, then try again."
        }
        TaskError::Spotify(SpotifyError::Listen(_)) => {
            "Another program is using port 9900. Close it, then try again."
        }
        TaskError::Spotify(SpotifyError::SignInRefused(_)) => {
            "Run Sync Spotify again and allow access in the browser."
        }
        TaskError::Spotify(SpotifyError::BadListId(_)) => "Untick that list and try again.",
        TaskError::Devices(DevicesError::Diskutil { .. } | DevicesError::Watch(_)) => {
            "Check that the device is still plugged in, press r to look again, then try again."
        }
        TaskError::Devices(DevicesError::Full { .. }) => {
            "Free some space on the device or pick fewer songs. The files already copied stay."
        }
        TaskError::Devices(DevicesError::Gone { .. }) => {
            "Plug the device back in and run Sync device again. The files already copied stay."
        }
        TaskError::Library(LibraryError::Ffmpeg { .. }) => {
            "Check that the file plays, then try again."
        }
        TaskError::State(_) => crate::runner::STATE_ADVICE,
        TaskError::Youtube(YoutubeError::Io(_))
        | TaskError::Io(_)
        | TaskError::Devices(DevicesError::Io { .. } | DevicesError::Library(_))
        | TaskError::Library(
            LibraryError::Io { .. } | LibraryError::Tag { .. } | LibraryError::Task(_),
        ) => DISK,
    };
    ErrorInfo {
        message: error.to_string(),
        advice: advice.to_owned(),
    }
}

/// The last-run line for a terminal failure event.
pub(crate) fn failure_line(event: &EventKind) -> String {
    match event {
        EventKind::RateLimited { .. } => "stopped: Spotify rate limit reached".to_owned(),
        EventKind::Error(info) => format!("failed: {}", info.message),
        _ => "failed".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_429_is_rate_limited_with_its_wait() {
        let error = TaskError::Spotify(SpotifyError::RateLimited {
            retry_after: Some(Duration::from_secs(30)),
        });
        assert_eq!(
            failure_event(&error),
            EventKind::RateLimited {
                wait: Some(Duration::from_secs(30))
            }
        );
    }

    #[test]
    fn a_403_is_a_plain_error_with_spotifys_reason() -> Result<(), String> {
        let error = TaskError::Spotify(SpotifyError::Forbidden {
            reason: "user not registered".into(),
        });
        let EventKind::Error(info) = failure_event(&error) else {
            return Err("expected an Error event".to_owned());
        };
        assert!(info.message.contains("user not registered"));
        assert_ne!(info.advice, "");
        Ok(())
    }

    #[test]
    fn missing_yt_dlp_says_how_to_start() {
        let error = TaskError::Youtube(YoutubeError::YtDlpNotFound);
        let event = failure_event(&error);
        assert_eq!(failure_line(&event), "failed: yt-dlp was not found on PATH");
        assert!(matches!(&event, EventKind::Error(info) if info.advice.contains("mise run tui")));
    }
}
