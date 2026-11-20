//! The converter (F9): one module used by F3 (a link's own audio that is not Opus, MP3 or M4A),
//! Convert (F9) and Sync device (F10).

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::Stdio;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::tag::TagExt;
use sdl_core::{Format, Preset, Quality};
use tokio::process::Command;

use crate::{LibraryError, io_err};

/// Convert `src` to `dest` with `ffmpeg`, audio only, sample rate kept.
///
/// - runs the `ffmpeg` binary through `tokio::process::Command` with `.stdin(Stdio::null())` and
///   `.kill_on_drop(true)`; the arguments come from [`ffmpeg_args`];
/// - writes to a `<name>.part` temp file beside `dest` (same folder, so the rename cannot cross
///   volumes), then renames it to `dest` when complete. A plain rename after checking `dest` is
///   free: `tempfile`'s `persist_noclobber` fails with os error 45 on FAT32 on macOS;
/// - then copies the tags from `src` with `lofty`, and the cover only when `preset.keep_cover`;
/// - a `src` already in the target format is copied as it is;
/// - `ErrorKind::NotFound` on spawn is [`LibraryError::FfmpegNotFound`]; a non-zero exit is
///   [`LibraryError::Ffmpeg`] with ffmpeg's last error line.
///
/// # Errors
/// The errors above; [`LibraryError::Io`] with `ErrorKind::AlreadyExists` when `dest` exists
/// (it is never replaced); [`LibraryError::Tag`] when the tags cannot be copied.
pub async fn convert_file(src: &Path, dest: &Path, preset: Preset) -> Result<(), LibraryError> {
    ensure_free(dest)?;
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name()) else {
        return Err(io_err(dest)(std::io::Error::other("not a file path")));
    };
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let temp = tempfile::Builder::new()
        .prefix(name)
        .suffix(".part")
        .tempfile_in(dir)
        .map_err(io_err(dir))?;
    if preset.format.holds(src) {
        std::fs::copy(src, temp.path()).map_err(io_err(src))?;
    } else {
        run_ffmpeg(src, temp.path(), preset).await?;
        copy_tags(src, temp.path(), preset)?;
    }
    ensure_free(dest)?;
    temp.persist(dest).map_err(|e| io_err(dest)(e.error))?;
    Ok(())
}

/// `dest` must not exist: a file is never replaced.
fn ensure_free(dest: &Path) -> Result<(), LibraryError> {
    if dest.try_exists().map_err(io_err(dest))? {
        return Err(io_err(dest)(std::io::ErrorKind::AlreadyExists.into()));
    }
    Ok(())
}

/// The `ffmpeg` arguments that write `src` as `out` in the preset's format: audio only (`-vn`,
/// first audio stream), no metadata (tags are copied afterwards with `lofty`), no sample-rate or
/// channel option, so both are kept. `-y` because `out` is the empty temp file. Presets:
/// MP3 `-c:a libmp3lame -q:a 2|4|6`, M4A `-c:a aac -b:a 192k|160k|128k`, Opus
/// `-c:a libopus -b:a 160k`. The muxer is named with `-f` because the temp file ends in `.part`.
#[must_use]
pub fn ffmpeg_args(src: &Path, out: &Path, preset: Preset) -> Vec<OsString> {
    let (codec, quality, muxer): (&str, [&str; 2], &str) = match (preset.format, preset.quality) {
        (Format::Mp3, Quality::High) => ("libmp3lame", ["-q:a", "2"], "mp3"),
        (Format::Mp3, Quality::Medium) => ("libmp3lame", ["-q:a", "4"], "mp3"),
        (Format::Mp3, Quality::Small) => ("libmp3lame", ["-q:a", "6"], "mp3"),
        (Format::M4a, Quality::High) => ("aac", ["-b:a", "192k"], "ipod"),
        (Format::M4a, Quality::Medium) => ("aac", ["-b:a", "160k"], "ipod"),
        (Format::M4a, Quality::Small) => ("aac", ["-b:a", "128k"], "ipod"),
        (Format::Opus, _) => ("libopus", ["-b:a", "160k"], "opus"),
    };
    let mut args: Vec<OsString> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"]
        .map(OsString::from)
        .into();
    args.push(src.as_os_str().to_owned());
    args.extend(
        ["-vn", "-map", "0:a:0", "-map_metadata", "-1", "-c:a", codec]
            .into_iter()
            .chain(quality)
            .chain(["-f", muxer])
            .map(OsString::from),
    );
    args.push(out.as_os_str().to_owned());
    args
}

async fn run_ffmpeg(src: &Path, out: &Path, preset: Preset) -> Result<(), LibraryError> {
    let output = Command::new("ffmpeg")
        .args(ffmpeg_args(src, out, preset))
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => LibraryError::FfmpegNotFound,
            _ => io_err(src)(error),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("ffmpeg stopped without saying why")
        .to_owned();
    Err(LibraryError::Ffmpeg {
        path: src.to_path_buf(),
        message,
    })
}

/// Copy `src`'s tags (and its pictures when the preset keeps the cover) onto `out`, mapped to the
/// tag type of the output's format. A source with no tag leaves `out` untagged.
fn copy_tags(src: &Path, out: &Path, preset: Preset) -> Result<(), LibraryError> {
    let tag_err = |path: &Path, message: String| LibraryError::Tag {
        path: path.to_path_buf(),
        message,
    };
    let source = lofty::read_from_path(src).map_err(|e| tag_err(src, e.to_string()))?;
    let Some(tag) = source.primary_tag().or_else(|| source.first_tag()) else {
        return Ok(());
    };
    let target = match preset.format {
        Format::Mp3 => FileType::Mpeg,
        Format::M4a => FileType::Mp4,
        Format::Opus => FileType::Opus,
    }
    .primary_tag_type();
    let mut tag = tag.clone();
    tag.re_map(target);
    if !preset.keep_cover {
        while tag.picture_count() > 0 {
            tag.remove_picture(0);
        }
    }
    tag.save_to_path(out, WriteOptions::default())
        .map_err(|e| tag_err(out, e.to_string()))
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
/// times [`Preset::nominal_kbps`] ([`Preset::estimate_bytes`]). When `lofty` cannot read the
/// duration the estimate is the file's own size, so one odd file never blocks a plan (Convert or
/// Sync device); the conversion itself then says what is wrong with it.
///
/// # Errors
/// [`LibraryError::Io`] when the file cannot be read.
pub fn output_size(src: &Path, preset: Option<Preset>) -> Result<OutputSize, LibraryError> {
    let own_size = || Ok::<u64, LibraryError>(std::fs::metadata(src).map_err(io_err(src))?.len());
    match preset {
        Some(preset) if !preset.format.holds(src) => {
            let bytes = match lofty::read_from_path(src) {
                Ok(tagged) => preset.estimate_bytes(tagged.properties().duration()),
                Err(_) => own_size()?,
            };
            Ok(OutputSize {
                bytes,
                estimated: true,
            })
        }
        _ => Ok(OutputSize {
            bytes: own_size()?,
            estimated: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{Format, Quality};

    use super::*;
    use crate::testkit::{TestResult, make_cover, make_song, probe};

    fn preset(format: Format, quality: Quality, keep_cover: bool) -> Preset {
        Preset {
            format,
            quality,
            keep_cover,
        }
    }

    fn args(format: Format, quality: Quality) -> Vec<String> {
        ffmpeg_args(
            Path::new("in.opus"),
            Path::new("out.part"),
            preset(format, quality, true),
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    fn has_pair(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == key && w[1] == value)
    }

    #[test]
    fn the_presets_are_the_spec_arguments() {
        for (quality, q) in [
            (Quality::High, "2"),
            (Quality::Medium, "4"),
            (Quality::Small, "6"),
        ] {
            let a = args(Format::Mp3, quality);
            assert!(
                has_pair(&a, "-c:a", "libmp3lame") && has_pair(&a, "-q:a", q),
                "{a:?}"
            );
        }
        for (quality, b) in [
            (Quality::High, "192k"),
            (Quality::Medium, "160k"),
            (Quality::Small, "128k"),
        ] {
            let a = args(Format::M4a, quality);
            assert!(
                has_pair(&a, "-c:a", "aac") && has_pair(&a, "-b:a", b),
                "{a:?}"
            );
        }
        let a = args(Format::Opus, Quality::Small);
        assert!(
            has_pair(&a, "-c:a", "libopus") && has_pair(&a, "-b:a", "160k"),
            "{a:?}"
        );
    }

    #[test]
    fn audio_only_and_rate_and_channels_are_left_alone() {
        let a = args(Format::Mp3, Quality::High);
        assert!(a.contains(&"-vn".to_owned()) && a.contains(&"-nostdin".to_owned()));
        assert!(!a.iter().any(|x| x == "-ar" || x == "-ac"), "{a:?}");
        assert_eq!(a.last().map(String::as_str), Some("out.part"));
    }

    #[tokio::test]
    async fn every_preset_makes_the_right_codec_bitrate_and_tags() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("in/Song.opus");
        make_song(&src, 10, "Song")?;
        // CBR AAC lands on its target; MP3 and Opus are VBR, so their rate depends on the music:
        // check it is plausible and that High > Medium > Small.
        let cases = [
            (Format::Mp3, Quality::High, "mp3", 60..=260),
            (Format::Mp3, Quality::Medium, "mp3", 50..=240),
            (Format::Mp3, Quality::Small, "mp3", 40..=200),
            (Format::M4a, Quality::High, "aac", 172..=212),
            (Format::M4a, Quality::Medium, "aac", 144..=176),
            (Format::M4a, Quality::Small, "aac", 115..=141),
            (Format::Opus, Quality::High, "opus", 60..=200),
        ];
        let mut mp3_rates = Vec::new();
        for (format, quality, codec, range) in cases {
            let dest = dir.path().join(format!(
                "out/{}-{}.{}",
                format.label(),
                quality.name(),
                format.extension()
            ));
            // An Opus source is already Opus, so make the Opus case from an MP3 source.
            let from = if format == Format::Opus {
                let mp3 = dir.path().join("in/Song.mp3");
                convert_file(&src, &mp3, preset(Format::Mp3, Quality::High, true)).await?;
                mp3
            } else {
                src.clone()
            };
            convert_file(&from, &dest, preset(format, quality, true)).await?;
            let found = probe(&dest)?;
            assert_eq!(
                found.get("codec_name").map(String::as_str),
                Some(codec),
                "{dest:?}"
            );
            assert_eq!(found.get("sample_rate").map(String::as_str), Some("48000"));
            assert_eq!(found.get("channels").map(String::as_str), Some("2"));
            let kbps = found.get("bit_rate").ok_or("no bit rate")?.parse::<i64>()? / 1000;
            assert!(range.contains(&kbps), "{dest:?}: {kbps} kbps");
            if format == Format::Mp3 {
                mp3_rates.push(kbps);
            }
            assert_eq!(
                found.get("tag:title").map(String::as_str),
                Some("Song"),
                "{found:?}"
            );
            assert_eq!(
                found.get("tag:artist").map(String::as_str),
                Some("The Artist")
            );
            assert_eq!(
                found.get("tag:album").map(String::as_str),
                Some("The Album")
            );
            assert_eq!(found.get("tag:genre").map(String::as_str), Some("Jazz"));
            let track = found.get("tag:track").map(String::as_str);
            assert!(matches!(track, Some("3/12" | "3")), "{dest:?}: {track:?}");
        }
        assert!(
            mp3_rates.windows(2).all(|pair| pair[0] > pair[1]),
            "{mp3_rates:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_file_already_in_the_format_is_copied_as_it_is() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("Song.mp3");
        make_song(&src, 2, "Song")?;
        let dest = dir.path().join("copy/Song.mp3");
        convert_file(&src, &dest, preset(Format::Mp3, Quality::Small, true)).await?;
        assert_eq!(std::fs::read(&src)?, std::fs::read(&dest)?);
        Ok(())
    }

    #[tokio::test]
    async fn an_existing_file_is_never_replaced_and_no_temp_file_is_left() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("Song.opus");
        make_song(&src, 2, "Song")?;
        let dest = dir.path().join("out/Song.mp3");
        std::fs::create_dir_all(dir.path().join("out"))?;
        std::fs::write(&dest, b"mine")?;
        let error = convert_file(&src, &dest, preset(Format::Mp3, Quality::High, true)).await;
        assert!(matches!(
            error,
            Err(LibraryError::Io { source, .. }) if source.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(std::fs::read(&dest)?, b"mine");
        assert_eq!(std::fs::read_dir(dir.path().join("out"))?.count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_conversion_says_why_and_leaves_nothing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("Broken.opus");
        std::fs::write(&src, b"this is not audio")?;
        let dest = dir.path().join("out/Broken.mp3");
        let error = convert_file(&src, &dest, preset(Format::Mp3, Quality::High, true)).await;
        assert!(
            matches!(error, Err(LibraryError::Ffmpeg { .. })),
            "{error:?}"
        );
        assert!(!dest.exists());
        assert_eq!(std::fs::read_dir(dir.path().join("out"))?.count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn the_cover_is_kept_or_dropped() -> TestResult {
        use lofty::file::TaggedFileExt;
        use lofty::picture::{Picture, PictureType};
        use lofty::tag::{Tag, TagExt};
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("Song.opus");
        make_song(&src, 2, "Song")?;
        let tagged = lofty::read_from_path(&src)?;
        let mut tag = tagged.primary_tag().ok_or("no tag")?.clone();
        let png = make_cover(&dir.path().join("cover.png"))?;
        let mut picture = Picture::from_reader(&mut std::io::Cursor::new(png))?;
        picture.set_pic_type(PictureType::CoverFront);
        tag.push_picture(picture);
        tag.save_to_path(&src, lofty::config::WriteOptions::default())?;
        for (format, keep) in [
            (Format::Mp3, true),
            (Format::Mp3, false),
            (Format::M4a, true),
            (Format::M4a, false),
        ] {
            let dest = dir
                .path()
                .join(format!("{}-{keep}.{}", format.label(), format.extension()));
            convert_file(&src, &dest, preset(format, Quality::High, keep)).await?;
            let out = lofty::read_from_path(&dest)?;
            let pictures = out.primary_tag().map_or(0, Tag::picture_count);
            assert_eq!(pictures, u32::from(keep), "{dest:?}");
        }
        Ok(())
    }

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
        assert_eq!(
            output_size(&file, Some(preset(Format::Mp3, Quality::Small, true)))?,
            exact
        );
        Ok(())
    }

    #[test]
    fn a_conversion_is_estimated_from_duration_and_nominal_bitrate() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("song.opus");
        make_song(&file, 10, "Song")?;
        let size = output_size(&file, Some(preset(Format::M4a, Quality::High, true)))?;
        assert_eq!(
            size,
            OutputSize {
                bytes: 10 * 192 * 125,
                estimated: true
            }
        );
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
