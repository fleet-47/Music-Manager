//! Downloads (F3) with the `yt-dlp` binary, run by [`crate::ytdlp`].
//!
//! Each download has its own temp directory (`tempfile`) inside `<music_root>/.sdl-tmp/`
//! (`sdl_core::place::tmp_dir`), so landing the file in the library is a same-volume rename. The
//! directory is removed when the [`Downloaded`] is dropped, on success, failure or cancel: the
//! [`TempDir`] lives across the whole call, so dropping the future (a stopped task) removes it too.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use sdl_core::{Format, place};
use tempfile::TempDir;

use crate::{YoutubeError, ytdlp};

/// What the file is named inside its temp directory; `%(ext)s` is filled in by `yt-dlp` with the
/// file's real type.
const FILE_TEMPLATE: &str = "audio.%(ext)s";

/// Which audio to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Audio {
    /// A match's audio: the Opus stream when there is one, else the best stream. Nothing is
    /// re-encoded (F3). This is the one place the command is chosen.
    Opus,
    /// A link's own audio, kept as served: the best stream, whatever its codec.
    AsServed,
}

/// The `yt-dlp` arguments for one download (pure). `-f bestaudio[acodec=opus]/bestaudio -x
/// --no-playlist --no-progress -o <dir>/audio.%(ext)s -- <url>` for [`Audio::Opus`]; `-f bestaudio -x --no-playlist --no-progress -o <dir>/audio.%(ext)s -- <url>`
/// for [`Audio::AsServed`].
fn args(audio: Audio, dir: &Path, url: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = match audio {
        Audio::Opus => ["-f", "bestaudio[acodec=opus]/bestaudio", "-x"]
            .map(OsString::from)
            .to_vec(),
        Audio::AsServed => ["-f", "bestaudio", "-x"].map(OsString::from).to_vec(),
    };
    args.extend(["--no-playlist", "--no-progress", "-o"].map(OsString::from));
    args.push(dir.join(FILE_TEMPLATE).into_os_string());
    args.push(OsString::from("--"));
    args.push(OsString::from(url));
    args
}

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

    /// The file's real type when it is one the library holds (Opus, MP3 or M4A), which enters as
    /// it is; `None` for any other type, which the caller converts to Opus with the converter.
    #[must_use]
    pub fn format(&self) -> Option<Format> {
        Format::of(&self.file)
    }
}

/// Download `url` (a candidate's URL, or a `music.youtube.com` item URL), preferring its Opus
/// stream, into a new temp directory under `music_root`'s `.sdl-tmp`.
///
/// Arguments: `-f bestaudio[acodec=opus]/bestaudio -x --no-playlist --no-progress
/// -o <temp dir>/audio.%(ext)s -- <url>`. Nothing is re-encoded: an Opus stream stays `.opus`; a
/// source with no Opus stream keeps the best stream as served (usually `.m4a`), so the caller
/// reads [`Downloaded::format`]. The result is the single audio file left in the directory.
///
/// # Errors
/// `ErrorKind::NotFound` on spawn is [`YoutubeError::YtDlpNotFound`] ("yt-dlp was not found on
/// PATH"); a non-zero exit is [`YoutubeError::YtDlp`] with the last `ERROR:` line of stderr; the
/// temp directory could not be made or read is [`YoutubeError::Io`].
pub async fn download_opus(url: &str, music_root: &Path) -> Result<Downloaded, YoutubeError> {
    download(Audio::Opus, url, music_root).await
}

/// Download a link's own audio, kept as served (only chosen in review, F8): `-f bestaudio -x` with
/// no `--audio-format`, otherwise as [`download_opus`]. [`Downloaded::format`] says whether the
/// file is Opus, MP3 or M4A (kept as it is) or another type (the caller converts it to Opus with
/// `sdl_library::converter`; this crate does not depend on it).
///
/// # Errors
/// As [`download_opus`].
pub async fn download_own_audio(url: &str, music_root: &Path) -> Result<Downloaded, YoutubeError> {
    download(Audio::AsServed, url, music_root).await
}

async fn download(audio: Audio, url: &str, music_root: &Path) -> Result<Downloaded, YoutubeError> {
    let tmp = place::tmp_dir(music_root);
    tokio::fs::create_dir_all(&tmp).await?;
    let dir = tempfile::Builder::new().prefix("dl-").tempdir_in(&tmp)?;
    ytdlp::run(&args(audio, dir.path(), url)).await?;
    finish(dir)
}

/// The single file `yt-dlp` left in `dir`, keeping the directory alive with it.
fn finish(dir: TempDir) -> Result<Downloaded, YoutubeError> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir.path())? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    match files.len() {
        1 => {
            let file = files.swap_remove(0);
            Ok(Downloaded { file, _dir: dir })
        }
        0 => Err(YoutubeError::YtDlp(
            "it finished but left no audio file".to_owned(),
        )),
        _ => Err(YoutubeError::YtDlp(
            "it left more than one file, so the audio could not be told apart".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_opus_arguments_are_the_specified_command_and_name_no_audio_format() {
        let args = args(
            Audio::Opus,
            Path::new("/m/.sdl-tmp/dl-x"),
            "https://music.youtube.com/watch?v=abc",
        );
        assert_eq!(
            strings(&args),
            [
                "-f",
                "bestaudio[acodec=opus]/bestaudio",
                "-x",
                "--no-playlist",
                "--no-progress",
                "-o",
                "/m/.sdl-tmp/dl-x/audio.%(ext)s",
                "--",
                "https://music.youtube.com/watch?v=abc",
            ]
        );
    }

    #[test]
    fn the_own_audio_arguments_name_no_audio_format() {
        let args = strings(&args(Audio::AsServed, Path::new("/d"), "https://x/y"));
        assert!(!args.iter().any(|a| a == "--audio-format"));
        assert_eq!(args.first().map(String::as_str), Some("-f"));
        assert_eq!(args.get(1).map(String::as_str), Some("bestaudio"));
        assert!(args.iter().any(|a| a == "-x"));
        assert_eq!(args.last().map(String::as_str), Some("https://x/y"));
        assert_eq!(
            args.get(args.len().saturating_sub(2)).map(String::as_str),
            Some("--")
        );
    }

    #[test]
    fn a_url_that_looks_like_a_flag_stays_after_the_separator() {
        let args = strings(&args(Audio::Opus, Path::new("/d"), "--exec=bad"));
        let separator = args.iter().position(|a| a == "--");
        let url = args.iter().position(|a| a == "--exec=bad");
        assert!(separator < url);
    }

    #[test]
    fn the_single_file_is_the_result_and_its_type_is_reported() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("audio.webm"), b"x")?;
        let done = finish(dir)?;
        assert_eq!(
            done.file().file_name().and_then(|n| n.to_str()),
            Some("audio.webm")
        );
        assert_eq!(done.format(), None);
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("audio.m4a"), b"x")?;
        assert_eq!(finish(dir)?.format(), Some(Format::M4a));
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("audio.opus"), b"x")?;
        assert_eq!(finish(dir)?.format(), Some(Format::Opus));
        Ok(())
    }

    #[test]
    fn a_download_with_no_opus_stream_lands_with_its_own_extension() -> TestResult {
        let root = tempfile::tempdir()?;
        let song = sdl_core::WantedSong {
            source: sdl_core::SongSource::Spotify {
                track_id: "t".into(),
            },
            title: "Title".into(),
            artists: vec!["Artist".into()],
            album: Some("Album".into()),
            album_artist: None,
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: None,
            cover_url: None,
        };
        let place = sdl_core::Place::of(&sdl_core::Library::at("Me", root.path(), true), &song);
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("audio.m4a"), b"x")?;
        let format = finish(dir)?.format().ok_or("m4a is held as it is")?;
        let landed = place.path(format);
        assert_eq!(landed.extension().and_then(|e| e.to_str()), Some("m4a"));
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(&landed, b"x")?;
        assert_eq!(place.present()?, Some(Format::M4a));
        Ok(())
    }

    #[test]
    fn no_file_or_two_files_is_a_plain_error() -> TestResult {
        let empty = tempfile::tempdir()?;
        assert!(matches!(finish(empty), Err(YoutubeError::YtDlp(_))));
        let two = tempfile::tempdir()?;
        std::fs::write(two.path().join("a.opus"), b"x")?;
        std::fs::write(two.path().join("b.opus"), b"x")?;
        assert!(matches!(finish(two), Err(YoutubeError::YtDlp(_))));
        Ok(())
    }

    #[test]
    fn dropping_a_download_removes_its_directory() -> TestResult {
        let root = tempfile::tempdir()?;
        let dir = tempfile::Builder::new()
            .prefix("dl-")
            .tempdir_in(root.path())?;
        let path = dir.path().to_owned();
        std::fs::write(path.join("audio.opus"), b"x")?;
        let done = finish(dir)?;
        assert!(path.exists());
        drop(done);
        assert!(!path.exists());
        Ok(())
    }
}
