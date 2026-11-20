//! Running the `yt-dlp` binary: the one place that spawns it, for downloads and for listings.
//!
//! `tokio::process::Command` with `.stdin(Stdio::null())` (yt-dlp must never read the TUI's
//! keyboard), `.kill_on_drop(true)` (a cancelled task kills the child) and stdout and stderr
//! piped.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::Stdio;

use tokio::process::Command;

use crate::YoutubeError;

/// Run `yt-dlp` with `args` and return its standard output.
///
/// # Errors
/// [`YoutubeError::YtDlpNotFound`] when the binary is not on `PATH`; [`YoutubeError::YtDlp`] with
/// the last `ERROR:` line of stderr when it exits non-zero; [`YoutubeError::Io`] for any other
/// failure to run it.
pub(crate) async fn run(args: &[OsString]) -> Result<Vec<u8>, YoutubeError> {
    let output = Command::new("yt-dlp")
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(spawn_error)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(YoutubeError::YtDlp(failure_reason(
            &String::from_utf8_lossy(&output.stderr),
            output.status.code(),
        )))
    }
}

/// `ErrorKind::NotFound` on spawn is "yt-dlp was not found on PATH"; anything else stays an I/O
/// error.
fn spawn_error(error: std::io::Error) -> YoutubeError {
    if error.kind() == ErrorKind::NotFound {
        YoutubeError::YtDlpNotFound
    } else {
        YoutubeError::Io(error)
    }
}

/// Why a run failed: the last `ERROR:` line of `stderr` without its prefix; with none, the last
/// non-empty line; with no output at all, the exit code.
fn failure_reason(stderr: &str, code: Option<i32>) -> String {
    let lines = || {
        stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
    };
    if let Some(line) = lines().rev().find_map(|line| line.strip_prefix("ERROR:")) {
        return line.trim().to_owned();
    }
    lines().next_back().map_or_else(
        || {
            code.map_or_else(
                || "it was stopped before it finished".to_owned(),
                |code| format!("it exited with code {code} and printed nothing"),
            )
        },
        str::to_owned,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_error_line_is_the_reason() {
        let stderr =
            "WARNING: x\nERROR: first\n[youtube] more\nERROR: [youtube] abc: Video unavailable\n";
        assert_eq!(
            failure_reason(stderr, Some(1)),
            "[youtube] abc: Video unavailable"
        );
    }

    #[test]
    fn without_an_error_line_the_last_line_is_used() {
        assert_eq!(failure_reason("one\ntwo\n\n", Some(1)), "two");
    }

    #[test]
    fn with_no_output_the_exit_code_is_named() {
        assert_eq!(
            failure_reason("", Some(2)),
            "it exited with code 2 and printed nothing"
        );
        assert_eq!(
            failure_reason("  \n", None),
            "it was stopped before it finished"
        );
    }

    #[test]
    fn a_missing_binary_is_named_plainly() {
        let missing = spawn_error(std::io::Error::from(ErrorKind::NotFound));
        assert_eq!(missing.to_string(), "yt-dlp was not found on PATH");
        let other = spawn_error(std::io::Error::from(ErrorKind::PermissionDenied));
        assert!(matches!(other, YoutubeError::Io(_)));
    }
}
