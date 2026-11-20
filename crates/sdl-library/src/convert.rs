//! The Convert function (F9): converted copies of the library (or ticked playlists) written to a
//! separate destination, with the library's layout. Never changes or deletes the library.

use std::path::{Path, PathBuf};

use sdl_core::{ConvertChoice, Counts, EventSender, PlanSummary, StopFlag};

use crate::LibraryError;

/// What a Convert run would do: the files to convert and the summary shown before Start.
#[derive(Debug, Clone)]
pub struct ConvertPlan {
    summary: PlanSummary,
    /// `(source in the library, destination file)`.
    jobs: Vec<(PathBuf, PathBuf)>,
}

impl ConvertPlan {
    /// The summary line's numbers: `N files to convert · M already there`. `free_bytes` is the
    /// free space at the destination when known. A destination inside `music_root` is a plan with
    /// a `refusal` in plain words (not an error), so the form stays open.
    #[must_use]
    pub const fn summary(&self) -> &PlanSummary {
        &self.summary
    }
}

/// Plan a Convert: the files of the ticked selection ([`sdl_core::Selection`]: the whole library
/// via `sdl_core::library_files`, and each ticked playlist's songs that exist), each mapped to
/// `<destination>/<same relative path>` with the preset's extension. Files whose stem is already
/// there in any format count as "already there". Sizes are estimated from each file's duration
/// and [`sdl_core::Preset::nominal_kbps`] (`estimated` is set).
///
/// # Errors
/// [`LibraryError::NotBuilt`] until built; then a library folder or a tag that cannot be read.
pub async fn plan(music_root: &Path, choice: &ConvertChoice) -> Result<ConvertPlan, LibraryError> {
    let _ = (music_root, choice);
    std::future::ready(Err(LibraryError::NotBuilt("planning Convert"))).await
}

/// Run a plan: convert each file with [`crate::converter::convert_file`],
/// [`crate::converter::parallelism`] at a time, sending [`sdl_core::EventKind::Counts`] after each
/// file and an `Attention` for each failed one. Then write each ticked playlist to the
/// destination with `playlists::write_for_destination`. `stop` is checked between files: the file
/// in hand finishes, then the run ends with the counts so far.
///
/// # Errors
/// [`LibraryError::NotBuilt`] until built; then a failure that ends the whole run (`ffmpeg` not
/// found, the destination not writable). A single file that fails is counted and listed, not an
/// error.
pub async fn run(
    plan: &ConvertPlan,
    events: &EventSender,
    stop: &StopFlag,
) -> Result<Counts, LibraryError> {
    let _ = (&plan.jobs, events, stop);
    std::future::ready(Err(LibraryError::NotBuilt("running Convert"))).await
}
