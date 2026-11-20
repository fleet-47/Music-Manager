//! The converter (F10): the `ffmpeg` binary, run directly, with tags and cover carried over with
//! `lofty`, and the Convert plan. [`convert_file`] is the one place a file is converted; device sync
//! (F4) reaches it through [`crate::files::run`].

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use lofty::tag::{Tag, TagExt};
use sdl_core::{ConvertChoice, Format};
use tokio::process::Command;

use crate::files::{Plan, plan_jobs};
use crate::{LibraryError, blocking, io_err};

/// Convert `src` to `format`, writing `dest`. `dest`'s folder is created; `src` is never changed.
///
/// `ffmpeg` through `tokio::process::Command` (`.stdin(Stdio::null())`, `.kill_on_drop(true)`,
/// arguments passed separately, stderr piped), audio only: MP3 `-vn -c:a libmp3lame -q:a 0`, M4A
/// `-vn -c:a aac -b:a 256k`, Opus `-vn -c:a libopus -b:a 160k`. `-vn` drops a cover-art stream
/// (ffmpeg shows an embedded cover as a video stream), so the cover comes back through `lofty`.
/// Written to a temp file in `dest`'s folder (a `tempfile` with a dot prefix, removed on drop) and
/// renamed to `dest` when complete. Because the temp name is not `dest`'s, name the muxer with
/// `-f` (MP3 `mp3`, M4A `ipod`, Opus `opus`, as `ffmpeg -h muxer=<name>` lists their extensions)
/// and pass `-y`, since the temp file already exists. Then the source's primary tag, with its
/// pictures, is read with `lofty`, re-mapped to the new file's primary tag type
/// (`Tag::re_map`) and saved to the temp file before the rename.
///
/// # Errors
/// [`LibraryError::FfmpegNotFound`] when spawning hits `ErrorKind::NotFound`,
/// [`LibraryError::Ffmpeg`] with ffmpeg's last stderr line on a non-zero exit, or a tag or file
/// failure. On any error no `dest` is left.
pub async fn convert_file(src: &Path, dest: &Path, format: Format) -> Result<(), LibraryError> {
    let folder = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(folder).map_err(io_err(folder))?;
    // Dot-prefixed, in the destination's folder (same volume, so the rename is atomic); removed
    // on drop, so a failure or a stop leaves nothing behind. Created with 0o666 less the umask,
    // as `File::create` would (tempfile's documented `permissions` use), not tempfile's
    // owner-only 0o600, so a converted file is readable like a copied or downloaded one.
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o666))
        .prefix(".sdl-convert-")
        .suffix(&format!(".{}", format.extension()))
        .tempfile_in(folder)
        .map_err(io_err(folder))?;
    run_ffmpeg(src, temp.path(), format).await?;
    let from = src.to_path_buf();
    let to = temp.path().to_path_buf();
    blocking(move || carry_tags(&from, &to)).await?;
    temp.persist(dest)
        .map_err(|error| io_err(dest)(error.error))?;
    Ok(())
}

/// The `-f` muxer name and encoder arguments for `format`, as `ffmpeg -h muxer=<name>` and
/// `ffmpeg -encoders` list them.
const fn encoder_args(format: Format) -> (&'static str, [&'static str; 5]) {
    match format {
        Format::Mp3 => ("mp3", ["-vn", "-c:a", "libmp3lame", "-q:a", "0"]),
        Format::M4a => ("ipod", ["-vn", "-c:a", "aac", "-b:a", "256k"]),
        Format::Opus => ("opus", ["-vn", "-c:a", "libopus", "-b:a", "160k"]),
    }
}

/// Run `ffmpeg` to write `out` (an existing temp file, hence `-y`) from `src`.
async fn run_ffmpeg(src: &Path, out: &Path, format: Format) -> Result<(), LibraryError> {
    let (muxer, encoder) = encoder_args(format);
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(encoder)
        .args(["-f", muxer])
        .arg(out)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    let output = match result {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(LibraryError::FfmpegNotFound);
        }
        Err(error) => {
            return Err(LibraryError::Ffmpeg {
                path: src.to_path_buf(),
                message: error.to_string(),
            });
        }
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(|| output.status.to_string(), str::to_owned);
    Err(LibraryError::Ffmpeg {
        path: src.to_path_buf(),
        message,
    })
}

/// Copy the source's tags and pictures onto `dest` with `lofty`: the source's primary tag (else
/// its first) is re-mapped to the type `dest` uses ([`Tag::re_map`], which drops items the new
/// type cannot hold), its cover marked as the front cover ([`mark_front_cover`]) and saved,
/// replacing whatever `ffmpeg` wrote. A source with no tag leaves `dest` as `ffmpeg` made it.
fn carry_tags(src: &Path, dest: &Path) -> Result<(), LibraryError> {
    let tag_err = |path: &Path, message: String| LibraryError::Tag {
        path: path.to_path_buf(),
        message,
    };
    let source = lofty::read_from_path(src).map_err(|e| tag_err(src, e.to_string()))?;
    let Some(mut tag) = source.primary_tag().or_else(|| source.first_tag()).cloned() else {
        return Ok(());
    };
    let target = lofty::read_from_path(dest).map_err(|e| tag_err(dest, e.to_string()))?;
    tag.re_map(target.primary_tag_type());
    mark_front_cover(&mut tag);
    tag.save_to_path(dest, WriteOptions::default())
        .map_err(|e| tag_err(dest, e.to_string()))
}

/// When no picture is the front cover, make the first one the front cover. An M4A cover has no
/// picture type (lofty reads it as `Other`), so without this an M4A source would give an MP3 or
/// Opus file whose cover players do not show as the cover.
fn mark_front_cover(tag: &mut Tag) {
    if tag
        .pictures()
        .iter()
        .any(|picture| picture.pic_type() == PictureType::CoverFront)
    {
        return;
    }
    if let Some(mut cover) = tag.pictures().first().cloned() {
        cover.set_pic_type(PictureType::CoverFront);
        tag.set_picture(0, cover);
    }
}

/// Plan Convert. The destination is [`ConvertChoice::destination_path`] (a leading `~` expanded).
///
/// Refused, with a plain reason in the summary and no files, when the destination is the music
/// root or lies inside a ticked folder (the converted files would land in the folders being read),
/// or when it starts with `~` and the home folder is unknown. Paths are compared as the file
/// system resolves them ([`resolve`]), so a symlink, `..` or a different letter case on macOS
/// cannot slip a destination into the library.
///
/// Otherwise the jobs are [`crate::files::plan_jobs`] of the ticked `choice.folders` of
/// `music_root` into the destination with `Some(choice.format)`: a file already in that format is
/// copied as is, a song already present there in any format is counted as already there. The
/// summary's `free_bytes` is `None`. Run the result with [`crate::files::run`]; conversions are
/// written beside their destination (no staging).
///
/// # Errors
/// A read or walk failure.
pub async fn plan(music_root: &Path, choice: &ConvertChoice) -> Result<Plan, LibraryError> {
    let Some(destination) = choice.destination_path() else {
        return Ok(Plan::refused(
            "sdl cannot find your home folder to expand ~. Type the destination in full."
                .to_owned(),
        ));
    };
    let root = music_root.to_path_buf();
    let folders = choice.folders.clone();
    let format = choice.format;
    blocking(move || {
        if let Some(reason) = refusal(&root, &folders, &destination) {
            return Ok(Plan::refused(reason));
        }
        plan_jobs(&root, &folders, &destination, Some(format))
            .map(|planned| Plan::new(planned, None, None))
    })
    .await
}

/// Why `destination` cannot take the converted files of the ticked `folders` of `music_root`, in
/// plain words; `None` when it can. Blocks (it asks the file system to [`resolve`] paths).
fn refusal(music_root: &Path, folders: &[String], destination: &Path) -> Option<String> {
    let root = resolve(music_root);
    let target = resolve(destination);
    if target == root {
        return Some(format!(
            "The destination {} is your music folder. Choose a folder outside it.",
            destination.display()
        ));
    }
    let folder = folders
        .iter()
        .find(|folder| target.starts_with(root.join(folder)))?;
    Some(format!(
        "The destination {} is inside {folder}, which is being converted. Choose a folder outside the ticked folders.",
        destination.display()
    ))
}

/// `path` as the file system sees it: each existing part canonicalised (`std::fs::canonicalize`,
/// which on macOS also follows symlinks and corrects letter case), `.` dropped and `..` applied to
/// what is left, so a folder that does not exist yet still compares correctly.
fn resolve(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
        if let Ok(real) = std::fs::canonicalize(&resolved) {
            resolved = real;
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lofty::tag::{ItemKey, TagType};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(path: &Path) -> TestResult {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, b"audio")?;
        Ok(())
    }

    fn choice(folders: &[&str], format: Format, destination: PathBuf) -> ConvertChoice {
        ConvertChoice {
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
            format,
            destination,
        }
    }

    #[tokio::test]
    async fn a_destination_in_the_folders_being_read_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let lib = dir.path().join("lib");
        write(&lib.join("Spotify/L/a.opus"))?;
        for destination in [lib.clone(), lib.join("Spotify/out")] {
            let plan = plan(&lib, &choice(&["Spotify"], Format::Mp3, destination)).await?;
            assert!(plan.summary.refusal.is_some());
            assert!(!plan.summary.can_start());
            assert_eq!(plan.summary.files, 0);
        }
        let elsewhere = plan(
            &lib,
            &choice(&["Spotify"], Format::Mp3, lib.join("Youtube")),
        )
        .await?;
        assert!(elsewhere.summary.refusal.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_symlink_or_dot_dot_cannot_slip_the_destination_into_the_library() -> TestResult {
        let dir = tempfile::tempdir()?;
        let lib = dir.path().join("lib");
        write(&lib.join("Spotify/L/a.opus"))?;
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&lib, &link)?;
        for destination in [
            link.join("Spotify/new"),
            dir.path().join("gone/../lib/Spotify/new"),
            lib.join("Spotify/./L"),
        ] {
            let plan = plan(&lib, &choice(&["Spotify"], Format::Mp3, destination)).await?;
            assert!(plan.summary.refusal.is_some());
        }
        Ok(())
    }

    #[test]
    fn a_cover_without_a_type_becomes_the_front_cover() {
        use lofty::picture::{MimeType, Picture};
        let mut tag = Tag::new(TagType::Mp4Ilst);
        let other = Picture::unchecked(vec![1, 2, 3])
            .pic_type(PictureType::Other)
            .mime_type(MimeType::Png)
            .build();
        tag.push_picture(other.clone());
        tag.re_map(TagType::Id3v2);
        mark_front_cover(&mut tag);
        let types: Vec<_> = tag.pictures().iter().map(Picture::pic_type).collect();
        assert_eq!(types, [PictureType::CoverFront]);
        let mut both = Tag::new(TagType::Id3v2);
        both.push_picture(other);
        let mut front = Picture::unchecked(vec![4]).build();
        front.set_pic_type(PictureType::CoverFront);
        both.push_picture(front);
        mark_front_cover(&mut both);
        let types: Vec<_> = both.pictures().iter().map(Picture::pic_type).collect();
        assert_eq!(types, [PictureType::Other, PictureType::CoverFront]);
    }

    #[tokio::test]
    async fn plans_new_files_and_counts_songs_already_there_in_any_format() -> TestResult {
        let dir = tempfile::tempdir()?;
        let lib = dir.path().join("lib");
        let out = dir.path().join("out");
        write(&lib.join("A/one.opus"))?;
        write(&lib.join("A/two.mp3"))?;
        write(&lib.join("A/three.m4a"))?;
        write(&lib.join("A/three.opus"))?;
        write(&lib.join("B/skip.opus"))?;
        write(&out.join("A/one.m4a"))?;
        let plan = plan(&lib, &choice(&["A"], Format::Mp3, out.clone())).await?;
        // one: present as m4a. two: copied as is. three: once only, from the first file.
        assert_eq!(plan.summary.files, 2);
        assert_eq!(plan.summary.already, 2);
        assert_eq!(plan.summary.bytes, 10);
        assert_eq!(plan.summary.free_bytes, None);
        assert!(plan.summary.can_start());
        Ok(())
    }

    /// Every field F7 writes, in a Vorbis comment (what an Opus source carries).
    fn opus_tag() -> Tag {
        let mut tag = Tag::new(TagType::VorbisComments);
        for (key, value) in [
            (ItemKey::TrackTitle, "Title"),
            (ItemKey::TrackArtist, "Artist"),
            (ItemKey::AlbumTitle, "Album"),
            (ItemKey::AlbumArtist, "Album Artist"),
            (ItemKey::TrackNumber, "3"),
            (ItemKey::TrackTotal, "12"),
            (ItemKey::DiscNumber, "1"),
            (ItemKey::DiscTotal, "2"),
            (ItemKey::RecordingDate, "2020-05-01"),
            (ItemKey::Isrc, "USUM71900001"),
            (ItemKey::Genre, "Jazz"),
        ] {
            tag.insert_text(key, value.to_owned());
        }
        tag
    }

    #[test]
    fn every_field_survives_opus_to_mp3_and_m4a() {
        for target in [TagType::Id3v2, TagType::Mp4Ilst] {
            let mut tag = opus_tag();
            let before: Vec<_> = tag.items().map(lofty::tag::TagItem::key).collect();
            tag.re_map(target);
            let lost: Vec<_> = before
                .iter()
                .filter(|key| tag.get(**key).is_none())
                .collect();
            assert!(lost.is_empty(), "{target:?} lost {lost:?}");
        }
    }

    #[test]
    fn each_format_names_its_muxer_and_encoder() {
        assert_eq!(encoder_args(Format::Mp3).0, "mp3");
        assert_eq!(encoder_args(Format::M4a).0, "ipod");
        assert_eq!(encoder_args(Format::Opus).0, "opus");
        assert!(encoder_args(Format::Mp3).1.contains(&"libmp3lame"));
        assert!(encoder_args(Format::M4a).1.contains(&"256k"));
        assert!(encoder_args(Format::Opus).1.contains(&"160k"));
    }
}
