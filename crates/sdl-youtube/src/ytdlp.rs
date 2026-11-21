//! Running the `yt-dlp` binary: the one place that spawns it, for downloads and for listings.
//!
//! `tokio::process::Command` with `.stdin(Stdio::null())` (yt-dlp must never read the TUI's
//! keyboard), `.kill_on_drop(true)` (a cancelled task kills the child) and stdout and stderr
//! piped.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::Stdio;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::Command;

use crate::YoutubeError;

/// Kills a whole process group when dropped.
///
/// yt-dlp starts helpers of its own (`deno` to solve `YouTube`'s challenge, `ffmpeg` to fetch or
/// repackage a stream). `kill_on_drop` only reaches yt-dlp itself, so a stopped download used to
/// leave those helpers running. The program is started as the leader of its own group, its
/// helpers inherit the group, and this guard ends them all together.
struct Group(Option<Pid>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(group) = self.0.take() {
            // A group that has already gone is exactly what is wanted: nothing to report.
            let _ = kill_process_group(group, Signal::KILL);
        }
    }
}

/// Run yt-dlp with `args` and return what it printed. Dropping the future (a stopped or
/// cancelled task, or the app closing) kills yt-dlp and every process it started.
pub(crate) async fn run(args: &[OsString]) -> Result<Vec<u8>, YoutubeError> {
    run_program("yt-dlp", args).await
}

async fn run_program(program: &str, args: &[OsString]) -> Result<Vec<u8>, YoutubeError> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(spawn_error)?;
    // The child leads its own group, so the group's number is the child's.
    let mut group = Group(
        child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw),
    );
    let output = child.wait_with_output().await.map_err(spawn_error)?;
    // It ended by itself and its helpers with it. The group's number can now be given to another
    // process, so it must not be signalled.
    group.0 = None;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(YoutubeError::YtDlp(failure_reason(
            &String::from_utf8_lossy(&output.stderr),
            output.status.code(),
        )))
    }
}

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

    /// The defect this guards against: a stopped download killed yt-dlp but left what yt-dlp had
    /// started (deno, ffmpeg) running. A shell stands in for yt-dlp and `sleep` for its helper.
    #[tokio::test]
    async fn stopping_a_run_kills_every_process_the_program_started()
    -> Result<(), Box<dyn std::error::Error>> {
        use rustix::process::test_kill_process;
        use std::time::Duration;

        let dir = tempfile::tempdir()?;
        let pid_file = dir.path().join("helper.pid");
        let script = format!("sleep 300 & echo $! > '{}'; wait", pid_file.display());
        let args = [OsString::from("-c"), OsString::from(script)];
        let run = tokio::spawn(async move { run_program("sh", &args).await });

        let mut helper = None;
        for _ in 0..100 {
            let read = std::fs::read_to_string(&pid_file).ok();
            if let Some(pid) = read.and_then(|text| text.trim().parse::<i32>().ok()) {
                helper = Pid::from_raw(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let helper = helper.ok_or("the helper never started")?;
        assert!(
            test_kill_process(helper).is_ok(),
            "the helper is not running"
        );

        // What Stop does: abort the task, which drops the run.
        run.abort();
        let _ = run.await;

        let mut gone = false;
        for _ in 0..100 {
            if test_kill_process(helper).is_err() {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(gone, "the helper outlived the stop");
        Ok(())
    }
}
