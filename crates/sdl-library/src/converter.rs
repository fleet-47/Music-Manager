//! The converter (F9): one module used by F3 (a link's own audio that is not Opus, MP3 or M4A),
//! Convert (F9) and Sync device (F10).

use std::num::NonZeroUsize;
use std::path::Path;

use lofty::file::AudioFile;
use sdl_core::Preset;

use crate::{LibraryError, io_err};

/// Convert `src` to `dest` with `ffmpeg`, audio only, sample rate kept.
///
/// Contract:
/// - runs the `ffmpeg` binary through `tokio::process::Command` with `.stdin(Stdio::null())` and
///   `.kill_on_drop(true)`;
/// - presets: MP3 `-c:a libmp3lame -q:a 2` (High) / `4` (Medium) / `6` (Small); M4A
///   `-c:a aac -b:a 192k` / `160k` / `128k`; Opus `-c:a libopus -b:a 160k`
///   (see [`sdl_core::Preset::nominal_kbps`]). The library is 48 kHz Opus, and `libmp3lame`,
///   `aac` and `libopus` all accept 48 kHz (`ffmpeg -h encoder=<name>`), so no rate is passed;
/// - writes to a temp file beside `dest` (same folder, so the rename cannot cross volumes) and
///   renames it to `dest` when complete. A plain rename: `tempfile`'s `persist_noclobber` fails
///   with "Operation not supported" (os error 45) on a FAT32 volume on macOS (measured on a FAT32
///   disk image), and a device is FAT32. Callers plan only files that are not there yet;
/// - then copies the tags from `src` with `lofty`, and the cover only when `preset.keep_cover`;
/// - a `src` already in the target format is copied as it is (no re-encoding);
/// - `ErrorKind::NotFound` on spawn is [`LibraryError::FfmpegNotFound`]; a non-zero exit is
///   [`LibraryError::Ffmpeg`] with ffmpeg's last error line.
///
/// # Errors
/// [`LibraryError::NotBuilt`] until built; then the errors above.
pub async fn convert_file(src: &Path, dest: &Path, preset: Preset) -> Result<(), LibraryError> {
    let _ = (src, dest, preset);
    std::future::ready(Err(LibraryError::NotBuilt("the converter"))).await
}

/// How many conversions run at once: `std::thread::available_parallelism()`, 1 when unknown.
#[must_use]
pub fn parallelism() -> NonZeroUsize {
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

/// The size a file will take where it is going, for a plan's summary (F9, F10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSize {
    /// Bytes.
    pub bytes: u64,
    /// Whether `bytes` is an estimate (a conversion), so the summary says "about".
    pub estimated: bool,
}

/// The size `src` will take once copied (`preset` of `None`) or converted with `preset`. A copy,
/// or a file already in the preset's format (which [`convert_file`] copies as it is), is its own
/// exact size. A conversion is estimated: the file's duration (`lofty`, its audio properties)
/// times [`Preset::nominal_kbps`] ([`Preset::estimate_bytes`]).
///
/// # Errors
/// [`LibraryError::Io`] when the file cannot be read; [`LibraryError::Tag`] when `lofty` cannot
/// read its duration.
pub fn output_size(src: &Path, preset: Option<Preset>) -> Result<OutputSize, LibraryError> {
    match preset {
        Some(preset) if !preset.format.holds(src) => {
            let tagged = lofty::read_from_path(src).map_err(|error| LibraryError::Tag {
                path: src.to_path_buf(),
                message: error.to_string(),
            })?;
            Ok(OutputSize {
                bytes: preset.estimate_bytes(tagged.properties().duration()),
                estimated: true,
            })
        }
        _ => Ok(OutputSize {
            bytes: std::fs::metadata(src).map_err(io_err(src))?.len(),
            estimated: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{Format, Quality};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_copy_or_a_file_already_in_the_format_is_its_exact_size() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("song.mp3");
        std::fs::write(&file, [0_u8; 1234])?;
        let exact = OutputSize {
            bytes: 1234,
            estimated: false,
        };
        assert_eq!(output_size(&file, None)?, exact);
        let mp3 = Preset {
            format: Format::Mp3,
            quality: Quality::Small,
            keep_cover: true,
        };
        assert_eq!(output_size(&file, Some(mp3))?, exact);
        Ok(())
    }

    #[test]
    fn an_unreadable_file_is_an_error_naming_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let missing = dir.path().join("gone.opus");
        let message = output_size(&missing, None)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(message.contains("gone.opus"), "{message}");
        Ok(())
    }
}
