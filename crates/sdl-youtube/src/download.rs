//! Downloads (F3) with the `yt-dlp` binary: `tokio::process::Command`, `.stdin(Stdio::null())`,
//! `.kill_on_drop(true)`, stdout and stderr piped.
//!
//! Each download has its own temp directory (`tempfile`) inside `<music_root>/.sdl-tmp/`
//! (`sdl_core::place::tmp_dir`), so landing the file in the library is a same-volume rename. The
//! directory is removed when the [`Downloaded`] is dropped, on success, failure or cancel.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::YoutubeError;

/// One finished download: the single audio file `yt-dlp` left in its own temp directory.
///
/// Invariant: `file()` exists while this value lives; dropping the value removes the directory and
/// the file with it, so a file that is to be kept must be moved out first.
#[derive(Debug)]
pub struct Downloaded {
    file: PathBuf,
    _dir: TempDir,
}

impl Downloaded {
    /// The audio file. Its extension is its real type.
    #[must_use]
    pub fn file(&self) -> &Path {
        &self.file
    }
}

/// Download `url` (a candidate's URL, or a `music.youtube.com` item URL) as Opus into a new temp
/// directory under `music_root`'s `.sdl-tmp`.
///
/// Arguments: `-f bestaudio[acodec=opus]/bestaudio -x --audio-format opus --no-playlist
/// --no-progress -o <temp dir>/audio.%(ext)s -- <url>`. An Opus stream is copied, not re-encoded
/// (to be confirmed in yt-dlp's docs by the builder). The result is the single audio file left in
/// the directory.
///
/// # Errors
/// [`YoutubeError::NotBuilt`] until built. Then: `ErrorKind::NotFound` on spawn is
/// [`YoutubeError::YtDlpNotFound`] ("yt-dlp was not found on PATH"); a non-zero exit is
/// [`YoutubeError::YtDlp`] with the last `ERROR:` line of stderr.
pub async fn download_opus(url: &str, music_root: &Path) -> Result<Downloaded, YoutubeError> {
    let _ = (url, music_root);
    std::future::ready(Err(YoutubeError::NotBuilt("downloading as Opus"))).await
}

/// Download a link's own audio, kept as served (only chosen in review, F8): `-f bestaudio -x` with
/// no `--audio-format`, otherwise as [`download_opus`]. The caller keeps an Opus, MP3 or M4A file
/// as it is and converts any other type to Opus with `sdl_library::converter` (this crate does not
/// depend on it).
///
/// # Errors
/// As [`download_opus`].
pub async fn download_own_audio(url: &str, music_root: &Path) -> Result<Downloaded, YoutubeError> {
    let _ = (url, music_root);
    std::future::ready(Err(YoutubeError::NotBuilt(
        "downloading a link's own audio",
    )))
    .await
}
