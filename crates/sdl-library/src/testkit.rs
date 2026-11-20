//! Test helpers: real audio made with `ffmpeg`'s sine source, and `ffprobe` readings of it.
//! Both binaries come from mise (`mise x -- cargo test`).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A `seconds`-long sine (with noise) at 48 kHz stereo, encoded by the extension of `path`
/// (`.opus`, `.mp3`, `.m4a`), tagged with a title, artist, album, track 3 of 12 and genre.
pub fn make_song(path: &Path, seconds: u32, title: &str) -> TestResult {
    make_audio(path, seconds)?;
    let tagged = lofty::read_from_path(path)?;
    let mut tag = Tag::new(tagged.primary_tag_type());
    tag.set_title(title.to_owned());
    tag.set_artist("The Artist".to_owned());
    tag.set_album("The Album".to_owned());
    tag.insert_text(ItemKey::AlbumArtist, "The Album Artist".to_owned());
    tag.set_track(3);
    tag.set_track_total(12);
    tag.set_genre("Jazz".to_owned());
    tag.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// The same audio with no tags at all.
pub fn make_audio(path: &Path, seconds: u32) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A sine with pink noise on top: a pure sine compresses to almost nothing, so its bit rate
    // says nothing about the preset.
    let sine = format!("sine=frequency=440:duration={seconds}:sample_rate=48000");
    let noise = format!("anoisesrc=amplitude=0.3:color=pink:sample_rate=48000:duration={seconds}");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", &sine, "-f", "lavfi", "-i", &noise])
        .args([
            "-filter_complex",
            "[0][1]amix=inputs=2:duration=first,aformat=channel_layouts=stereo",
        ])
        .arg(path)
        .status()?;
    assert!(status.success(), "ffmpeg could not make {}", path.display());
    Ok(())
}

/// `ffprobe` fields of the first audio stream and the container tags, as `key -> value`.
/// Tag keys are lower-cased and prefixed `tag:`.
pub fn probe(path: &Path) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0"])
        .args([
            "-show_entries",
            "stream=codec_name,sample_rate,channels,bit_rate:stream_tags",
        ])
        .args(["-show_entries", "format=bit_rate:format_tags"])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()?;
    assert!(
        output.status.success(),
        "ffprobe failed on {}",
        path.display()
    );
    let mut found = HashMap::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        if let Some((key, value)) = line.split_once('=').filter(|(_, value)| *value != "N/A") {
            let key = key.strip_prefix("TAG:").map_or_else(
                || key.to_owned(),
                |name| format!("tag:{}", name.to_lowercase()),
            );
            found.entry(key).or_insert_with(|| value.to_owned());
        }
    }
    Ok(found)
}

/// A small PNG made by `ffmpeg` at `path`; returns its bytes.
pub fn make_cover(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "color=c=red:s=16x16", "-frames:v", "1"])
        .arg(path)
        .status()?;
    assert!(status.success(), "ffmpeg could not make a cover");
    Ok(std::fs::read(path)?)
}
