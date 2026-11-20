//! Audio formats (F3, F9, F10) and the extensions the library recognises.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// An audio file format: what the library holds, a conversion produces or a device copy ends in.
/// Each has one extension, so a format always names the file it makes. Where "As is" appears (the
/// device Format row, F10) it is no format at all: `Option<Format>` is `None`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Opus (`.opus`): what every song is in the library.
    #[default]
    Opus,
    /// MP3 (`.mp3`).
    Mp3,
    /// M4A, AAC (`.m4a`).
    M4a,
}

impl Format {
    /// The Convert Format row, in order; label with [`Format::label`].
    pub const CONVERT: [Self; 2] = [Self::Mp3, Self::M4a];
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

/// The extensions of the audio files the library, a device and Convert deal with: `opus`, `mp3`,
/// `m4a`. The same three decide whether a song is present in any format
/// ([`crate::Place::present`]).
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

    #[test]
    fn extensions_per_format() {
        assert_eq!(Format::Opus.extension(), "opus");
        assert_eq!(Format::Mp3.extension(), "mp3");
        assert_eq!(Format::M4a.extension(), "m4a");
        assert_eq!(Format::default(), Format::Opus);
    }

    #[test]
    fn labels_per_row() {
        assert_eq!(Format::Opus.label(), "Opus");
        assert_eq!(Format::device_label(None), "As is");
        assert_eq!(Format::device_label(Some(Format::M4a)), "M4A");
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
    fn choice_rows() {
        assert_eq!(Format::CONVERT, [Format::Mp3, Format::M4a]);
        assert_eq!(Format::DEVICE, [None, Some(Format::Mp3), Some(Format::M4a)]);
    }
}
