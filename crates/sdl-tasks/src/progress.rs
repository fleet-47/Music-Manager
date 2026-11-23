//! Count bookkeeping, the time estimate, landing files and the run-line wording shared by the
//! functions' tasks.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use sdl_core::Counts;
use sdl_core::landing::{Landed, Landing, land};
use sdl_core::place::tmp_dir;
use sdl_core::run::{Outcome, Tally};
use sdl_core::words::{need_attention, thousands, x_of_y};
use tempfile::{TempDir, TempPath};

/// A new scratch directory inside `<music_root>/.sdl-tmp` (same volume as the library, so the
/// final rename is atomic), for a conversion's output. Downloads make their own
/// (`sdl_youtube::Downloaded`). The directory and anything in it go when the [`TempDir`] is
/// dropped.
///
/// # Errors
/// The directory could not be created.
pub fn scratch_dir(root: &Path) -> std::io::Result<TempDir> {
    let parent = tmp_dir(root);
    std::fs::create_dir_all(&parent)?;
    tempfile::Builder::new().prefix("sdl-").tempdir_in(parent)
}

/// Move a finished download to `dest` in a library (creating its folder). A file already there is
/// never replaced: that is [`Outcome::Skipped`].
///
/// # Errors
/// The folder could not be made or the file could not be moved.
pub fn land_in_library(file: &Path, dest: &Path) -> std::io::Result<Outcome> {
    Ok(
        match land(TempPath::try_from_path(file)?, dest, Landing::Library)? {
            Landed::Written => Outcome::Added,
            Landed::Existed => Outcome::Skipped,
        },
    )
}

/// Time left: `elapsed / done * remaining`. `None` until something is done, and on overflow.
#[must_use]
pub fn estimate_remaining(elapsed: Duration, done: usize, total: usize) -> Option<Duration> {
    let done = u32::try_from(done).ok().filter(|done| *done > 0)?;
    let remaining = u32::try_from(total.saturating_sub(usize::try_from(done).ok()?)).ok()?;
    elapsed.checked_mul(remaining)?.checked_div(done)
}

/// The last-run line of a stopped run, with what had finished: `stopped after 412 of 2,117`, or
/// `stopped after 3` when the total was not known yet.
#[must_use]
pub fn stopped_line(counts: &Counts) -> String {
    match counts.total {
        Some(total) => format!("stopped after {}", x_of_y(counts.done, total)),
        None => format!("stopped after {}", thousands(counts.done)),
    }
}

/// The last-run line of a run that writes files: `12 copied`, `3 converted, 1 needs attention`.
#[must_use]
pub fn written_line(counts: &Counts, verb: &str) -> String {
    let mut line = format!("{} {verb}", thousands(counts.added));
    push_attention(&mut line, counts);
    line
}

/// The last-run line of a download run: `412 added, 12 already there, 2 without a cover, 3 need
/// attention`. A part that is zero is left out, except the number added.
#[must_use]
pub fn summary_line(tally: &Tally) -> String {
    let counts = tally.counts();
    let mut line = format!("{} added", thousands(counts.added));
    if counts.skipped > 0 {
        let _ = write!(line, ", {} already there", thousands(counts.skipped));
    }
    if tally.without_cover() > 0 {
        let _ = write!(
            line,
            ", {} without a cover",
            thousands(tally.without_cover())
        );
    }
    push_attention(&mut line, &counts);
    line
}

fn push_attention(line: &mut String, counts: &Counts) {
    let attention = counts.need_attention();
    if attention > 0 {
        let _ = write!(line, ", {}", need_attention(attention));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl_core::Attention;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn item(title: &str) -> Attention {
        Attention::failed(title.into(), "A".into(), "why".into())
    }

    #[test]
    fn estimate_is_elapsed_over_done_times_remaining() {
        let estimate = estimate_remaining(Duration::from_secs(60), 10, 40);
        assert_eq!(estimate, Some(Duration::from_secs(180)));
        assert_eq!(
            estimate_remaining(Duration::from_secs(60), 40, 40),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn estimate_needs_something_done() {
        assert_eq!(estimate_remaining(Duration::from_secs(60), 0, 40), None);
    }

    #[test]
    fn written_line_names_the_verb() {
        let counts = Counts {
            added: 3,
            failed: 1,
            ..Counts::default()
        };
        assert_eq!(
            written_line(&counts, "converted"),
            "3 converted, 1 needs attention"
        );
        assert_eq!(written_line(&Counts::default(), "copied"), "0 copied");
    }

    #[test]
    fn songs_left_out_before_the_run_count_as_skipped_in_its_line() {
        let mut tally = Tally::new(2);
        tally.record(Outcome::Added);
        tally.record(Outcome::AddedWithoutCover);
        let line = summary_line(&tally.with_already(10));
        assert_eq!(line, "2 added, 10 already there, 1 without a cover");
    }

    #[test]
    fn songs_without_a_cover_are_counted_in_the_line_only_when_there_are_some() {
        let mut tally = Tally::new(3);
        tally.record(Outcome::Added);
        assert_eq!(summary_line(&tally), "1 added");
        tally.record(Outcome::AddedWithoutCover);
        tally.record(Outcome::AddedWithoutCover);
        assert_eq!(summary_line(&tally), "3 added, 2 without a cover");
    }

    #[test]
    fn summary_line_mentions_attention_only_when_present() {
        let mut tally = Tally::new(4);
        tally.record(Outcome::Added);
        assert_eq!(summary_line(&tally), "1 added");
        tally.record(Outcome::Skipped);
        assert_eq!(summary_line(&tally), "1 added, 1 already there");
        tally.record(Outcome::Held(item("h")));
        tally.record(Outcome::Failed(item("f")));
        assert_eq!(
            summary_line(&tally),
            "1 added, 1 already there, 2 need attention"
        );
    }

    #[test]
    fn a_stopped_run_says_how_far_it_got() {
        let counts = Counts {
            total: Some(2117),
            done: 412,
            ..Counts::default()
        };
        assert_eq!(stopped_line(&counts), "stopped after 412 of 2,117");
        let unknown = Counts {
            total: None,
            done: 3,
            ..Counts::default()
        };
        assert_eq!(stopped_line(&unknown), "stopped after 3");
    }

    #[test]
    fn scratch_dirs_are_separate_and_landing_never_replaces_a_file() -> TestResult {
        let root = tempfile::tempdir()?;
        let dest = root.path().join("a/b.opus");
        let first_dir = scratch_dir(root.path())?;
        let second_dir = scratch_dir(root.path())?;
        assert_ne!(first_dir.path(), second_dir.path());
        let first = first_dir.path().join("audio.opus");
        let second = second_dir.path().join("audio.opus");
        assert!(!first.exists());
        std::fs::write(&first, b"new")?;
        assert_eq!(land_in_library(&first, &dest)?, Outcome::Added);
        std::fs::write(&second, b"other")?;
        assert_eq!(land_in_library(&second, &dest)?, Outcome::Skipped);
        assert_eq!(std::fs::read(&dest)?, b"new");
        drop((first_dir, second_dir));
        sdl_core::place::remove_tmp_dir_if_empty(root.path())?;
        assert!(!tmp_dir(root.path()).exists());
        Ok(())
    }
}
