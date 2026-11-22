//! Running an outside tool (spec H1): the one place a process is started. `yt-dlp`, `ffmpeg`,
//! `diskutil` and `mise` all go through [`run`]; the tools check asks [`on_path`].
//!
//! Interface, in full:
//! * [`run`] starts `program` with `args`, stdin closed (a tool must never read the TUI's
//!   keyboard) and its output captured, and returns what it printed on stdout.
//! * The program leads its own process group. Dropping the future (a stopped task, the app
//!   closing) kills the whole group: the program and every helper it started. A run that ended by
//!   itself signals nothing, because its group number may already belong to another process.
//! * Errors are [`ToolError`]: not found (naming the program), failed (one plain reason), or an
//!   I/O error.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::Stdio;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::Command;

/// Why an outside tool gave no result. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The program is not on `PATH`.
    #[error("{0} was not found on PATH")]
    NotFound(String),
    /// The program ran and ended with a failure. `reason` is the last `ERROR:` line of its stderr
    /// without that prefix; with none, the last non-empty line of stderr; with no stderr, the
    /// last non-empty line of stdout; with no output at all, `it exited with code N and printed
    /// nothing`, or `it was stopped before it finished` when there is no exit code.
    #[error("{program} failed: {reason}")]
    Failed {
        /// The program's name.
        program: String,
        /// Why, in one line.
        reason: String,
    },
    /// The program could not be started or waited for.
    #[error("{program} could not be run: {source}")]
    Io {
        /// The program's name.
        program: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

/// Kills a whole process group when dropped.
///
/// A tool may start helpers of its own, and `kill_on_drop` only reaches the tool itself. The
/// program is started as the leader of its own group, its helpers inherit the group, and this
/// guard ends them all together.
struct Group(Option<Pid>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(group) = self.0.take() {
            // A group that has already gone is exactly what is wanted: nothing to report.
            let _ = kill_process_group(group, Signal::KILL);
        }
    }
}

/// Run `program` with `args` and return what it printed on stdout.
///
/// # Errors
/// [`ToolError`], as described on each variant.
pub async fn run(program: &str, args: &[OsString]) -> Result<Vec<u8>, ToolError> {
    let io_error = |source: std::io::Error| {
        if source.kind() == ErrorKind::NotFound {
            ToolError::NotFound(program.to_owned())
        } else {
            ToolError::Io {
                program: program.to_owned(),
                source,
            }
        }
    };
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(io_error)?;
    // The child leads its own group, so the group's number is the child's.
    let mut group = Group(
        child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw),
    );
    let output = child.wait_with_output().await.map_err(io_error)?;
    // It ended by itself and its helpers with it. The group's number can now be given to another
    // process, so it must not be signalled.
    group.0 = None;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(ToolError::Failed {
            program: program.to_owned(),
            reason: failure_reason(
                &String::from_utf8_lossy(&output.stderr),
                &String::from_utf8_lossy(&output.stdout),
                output.status.code(),
            ),
        })
    }
}

/// Why a run failed: the last `ERROR:` line of `stderr` without its prefix; with none, the last
/// non-empty line of `stderr`; with no stderr, the last non-empty line of `stdout`; with no
/// output at all, the exit code.
fn failure_reason(stderr: &str, stdout: &str, code: Option<i32>) -> String {
    let lines = |text: &'_ str| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let err_lines = lines(stderr);
    if let Some(line) = err_lines
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("ERROR:"))
    {
        return line.trim().to_owned();
    }
    if let Some(line) = err_lines.last() {
        return line.clone();
    }
    if let Some(line) = lines(stdout).pop() {
        return line;
    }
    code.map_or_else(
        || "it was stopped before it finished".to_owned(),
        |code| format!("it exited with code {code} and printed nothing"),
    )
}

/// Whether `program` is found along `PATH` (the `which` crate).
#[must_use]
pub fn on_path(program: &str) -> bool {
    which::which(program).is_ok()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rustix::process::{Pid, test_kill_process};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn sh(script: &str) -> [OsString; 2] {
        [OsString::from("-c"), OsString::from(script)]
    }

    #[tokio::test]
    async fn a_run_returns_what_the_program_printed() -> TestResult {
        assert_eq!(run("sh", &sh("printf hello")).await?, b"hello");
        Ok(())
    }

    #[tokio::test]
    async fn a_program_never_reads_the_keyboard() -> TestResult {
        // `cat` with stdin closed ends at once with nothing; with the terminal it would wait.
        let printed = tokio::time::timeout(Duration::from_secs(5), run("cat", &[])).await??;
        assert_eq!(printed, b"");
        Ok(())
    }

    #[tokio::test]
    async fn a_missing_program_is_named() {
        let error = run("sdl-no-such-tool", &[]).await;
        assert!(matches!(&error, Err(ToolError::NotFound(name)) if name == "sdl-no-such-tool"));
        assert_eq!(
            error.err().map(|error| error.to_string()).as_deref(),
            Some("sdl-no-such-tool was not found on PATH")
        );
    }

    async fn reason_of(script: &str) -> Option<String> {
        match run("sh", &sh(script)).await {
            Err(ToolError::Failed { program, reason }) if program == "sh" => Some(reason),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_failure_gives_one_plain_reason() {
        // The last `ERROR:` line of stderr, without the prefix.
        let script = "echo 'WARNING: x' >&2; echo 'ERROR: first' >&2; echo '[y] more' >&2; \
                      echo 'ERROR: [y] abc: Video unavailable' >&2; exit 1";
        assert_eq!(
            reason_of(script).await.as_deref(),
            Some("[y] abc: Video unavailable")
        );
        // With none, the last non-empty line of stderr.
        assert_eq!(
            reason_of("echo one >&2; echo two >&2; echo >&2; exit 1")
                .await
                .as_deref(),
            Some("two")
        );
        // With no stderr, the last non-empty line of stdout.
        assert_eq!(
            reason_of("echo 'Volume failed to unmount'; exit 1")
                .await
                .as_deref(),
            Some("Volume failed to unmount")
        );
        // With nothing at all, the exit code.
        assert_eq!(
            reason_of("exit 2").await.as_deref(),
            Some("it exited with code 2 and printed nothing")
        );
    }

    #[test]
    fn the_tools_check_finds_what_is_on_path() {
        assert!(on_path("sh"));
        assert!(!on_path("sdl-no-such-tool"));
    }

    /// The defect this guards against: a stopped download killed yt-dlp but left what yt-dlp had
    /// started (deno, ffmpeg) running. A shell stands in for the tool and `sleep` for its helper.
    #[tokio::test]
    async fn stopping_a_run_kills_every_process_the_program_started() -> TestResult {
        let dir = tempfile::tempdir()?;
        let pid_file = dir.path().join("helper.pid");
        let script = format!("sleep 300 & echo $! > '{}'; wait", pid_file.display());
        let args = sh(&script);
        let running = tokio::spawn(async move { run("sh", &args).await });

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
        running.abort();
        let _ = running.await;

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
