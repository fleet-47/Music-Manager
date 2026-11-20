//! Audio formats (F9, F10) and the extensions the library recognises.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// An audio file format: what a download, a conversion or a device copy produces. Each has one
/// extension, so a format always names the file it makes.
///
/// Where "Original" appears (the download Format row, F9) it is [`Format::Opus`] fetched as served:
/// the yt-dlp arguments, not the format, decide that an Opus source is never re-encoded. Where
/// "As is" appears (the device Format row, F4) it is no format at all: `Option<Format>` is `None`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Opus (`.opus`). State files may call it `original`.
    #[default]
    #[serde(alias = "original")]
    Opus,
    /// MP3, best VBR (`.mp3`).
    Mp3,
    /// M4A, AAC (`.m4a`).
    M4a,
}

impl Format {
    /// The download Format row (Sync Spotify, Download link), in order; label with
    /// [`Format::download_label`].
    pub const DOWNLOAD: [Self; 3] = [Self::Opus, Self::Mp3, Self::M4a];
    /// The Convert Format row, in order; label with [`Format::label`].
    pub const CONVERT: [Self; 3] = [Self::Mp3, Self::M4a, Self::Opus];
    /// The device "Format on device" row, in order: As is, MP3, M4A; label with
    /// [`Format::device_label`].
    pub const DEVICE: [Option<Self>; 3] = [None, Some(Self::Mp3), Some(Self::M4a)];

    /// File extension, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
        }
    }

    /// Plain name, as in the Convert row.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Opus => "Opus",
            Self::Mp3 => "MP3",
            Self::M4a => "M4A",
        }
    }

    /// Name in the download Format row: Opus is "Original" there, since it is taken as served.
    #[must_use]
    pub const fn download_label(self) -> &'static str {
        match self {
            Self::Opus => "Original",
            other => other.label(),
        }
    }

    /// Name in the device "Format on device" row: `None` is "As is".
    #[must_use]
    pub const fn device_label(format: Option<Self>) -> &'static str {
        match format {
            None => "As is",
            Some(format) => format.label(),
        }
    }

    /// Whether `path` already has this format's extension (case-insensitive).
    #[must_use]
    pub fn holds(self, path: &Path) -> bool {
        has_extension(path, self.extension())
    }
}

/// The format rows of a step form (F9): the **Format** choice and the **Remember this format**
/// toggle. Sent with a function's start; the task saves the format as that function's default only
/// when `remember` is set, otherwise it applies to this run only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatChoice {
    /// The format for this run.
    pub format: Format,
    /// Save `format` as the function's default.
    pub remember: bool,
}

/// The extensions of the audio files the library, a device and Convert deal with: `opus`, `mp3`,
/// `m4a`. The same three decide whether a song is present in any format
/// ([`crate::paths::Target::is_present`]).
pub const AUDIO_EXTENSIONS: [&str; 3] = ["opus", "mp3", "m4a"];

/// Whether `path` names an audio file ([`AUDIO_EXTENSIONS`], case-insensitive).
#[must_use]
pub fn is_audio_file(path: &Path) -> bool {
    AUDIO_EXTENSIONS.iter().any(|ext| has_extension(path, ext))
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|found| found.to_str())
        .is_some_and(|found| found.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Holder {
        format: Format,
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn extensions_per_format() {
        assert_eq!(Format::Opus.extension(), "opus");
        assert_eq!(Format::Mp3.extension(), "mp3");
        assert_eq!(Format::M4a.extension(), "m4a");
        assert_eq!(Format::default(), Format::Opus);
    }

    #[test]
    fn labels_per_row() {
        assert_eq!(Format::Opus.download_label(), "Original");
        assert_eq!(Format::Mp3.download_label(), "MP3");
        assert_eq!(Format::Opus.label(), "Opus");
        assert_eq!(Format::device_label(None), "As is");
        assert_eq!(Format::device_label(Some(Format::M4a)), "M4A");
    }

    #[test]
    fn original_in_a_state_file_reads_as_opus() -> TestResult {
        let held: Holder = toml::from_str("format = \"original\"")?;
        assert_eq!(held.format, Format::Opus);
        Ok(())
    }

    #[test]
    fn holds_is_case_insensitive_and_exact() {
        assert!(Format::Mp3.holds(Path::new("/a/b.MP3")));
        assert!(!Format::Mp3.holds(Path::new("/a/b.m4a")));
        assert!(!Format::Mp3.holds(Path::new("/a/mp3")));
        assert!(Format::Opus.holds(Path::new("x.opus")));
    }

    #[test]
    fn audio_files_are_the_three_extensions() {
        assert!(is_audio_file(Path::new("a.opus")));
        assert!(is_audio_file(Path::new("a.M4A")));
        assert!(!is_audio_file(Path::new("a.flac")));
        assert!(!is_audio_file(Path::new("a.opus.part")));
        assert!(!is_audio_file(Path::new(".DS_Store")));
    }

    #[test]
    fn choice_rows_hold_three_each() {
        assert_eq!(Format::DOWNLOAD, [Format::Opus, Format::Mp3, Format::M4a]);
        assert_eq!(Format::CONVERT, [Format::Mp3, Format::M4a, Format::Opus]);
        assert_eq!(Format::DEVICE, [None, Some(Format::Mp3), Some(Format::M4a)]);
    }
}
