//! Conversion presets (F9): format, quality and cover choice, with the nominal bitrates the size
//! estimate uses.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Format;

/// How good a lossy conversion sounds against how large it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// MP3 `-q:a 2` (about 190 kbps), M4A 192k.
    #[default]
    High,
    /// MP3 `-q:a 4` (about 165 kbps), M4A 160k.
    Medium,
    /// MP3 `-q:a 6` (about 115 kbps), M4A 128k.
    Small,
}

impl Quality {
    /// All three, in the order of the Quality row.
    pub const ALL: [Self; 3] = [Self::High, Self::Medium, Self::Small];

    /// Plain name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Small => "Small",
        }
    }

    /// The row label for `format`: `High (about 190 kbps)`. Opus has one quality, so its label is
    /// just the bitrate.
    #[must_use]
    pub fn label(self, format: Format) -> String {
        let kbps = Preset {
            format,
            quality: self,
            keep_cover: true,
        }
        .nominal_kbps();
        if format == Format::Opus {
            format!("about {kbps} kbps")
        } else {
            format!("{} (about {kbps} kbps)", self.name())
        }
    }
}

/// What a conversion produces: a format, a quality and whether the cover art is kept.
///
/// Opus has a single preset (`-c:a libopus -b:a 160k`), used only to convert a link's own audio
/// that is not already Opus, MP3 or M4A (F3); its `quality` is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// Target format.
    pub format: Format,
    /// Quality (ignored for Opus).
    pub quality: Quality,
    /// Copy the cover art from the source.
    pub keep_cover: bool,
}

impl Preset {
    /// The Opus preset, keeping the cover.
    pub const OPUS: Self = Self {
        format: Format::Opus,
        quality: Quality::High,
        keep_cover: true,
    };

    /// The bitrate the size estimate uses, in kbps: MP3 190 / 165 / 115 (VBR quality 2 / 4 / 6),
    /// M4A 192 / 160 / 128, Opus 160.
    #[must_use]
    pub const fn nominal_kbps(self) -> u32 {
        match (self.format, self.quality) {
            (Format::Opus, _) | (Format::M4a, Quality::Medium) => 160,
            (Format::Mp3, Quality::High) => 190,
            (Format::Mp3, Quality::Medium) => 165,
            (Format::Mp3, Quality::Small) => 115,
            (Format::M4a, Quality::High) => 192,
            (Format::M4a, Quality::Small) => 128,
        }
    }

    /// Estimated size of a converted file: duration times [`Preset::nominal_kbps`]; saturates.
    #[must_use]
    pub fn estimate_bytes(self, duration: Duration) -> u64 {
        // kbps * 1000 bits / 8 = kbps * 125 bytes per second.
        duration
            .as_secs()
            .saturating_mul(u64::from(self.nominal_kbps()).saturating_mul(125))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(format: Format, quality: Quality) -> Preset {
        Preset {
            format,
            quality,
            keep_cover: true,
        }
    }

    #[test]
    fn nominal_bitrates_follow_the_spec() {
        let kbps = |format, quality| preset(format, quality).nominal_kbps();
        assert_eq!(Quality::ALL.map(|q| kbps(Format::Mp3, q)), [190, 165, 115]);
        assert_eq!(Quality::ALL.map(|q| kbps(Format::M4a, q)), [192, 160, 128]);
        assert_eq!(Quality::ALL.map(|q| kbps(Format::Opus, q)), [160, 160, 160]);
        assert_eq!(Preset::OPUS.nominal_kbps(), 160);
    }

    #[test]
    fn size_is_duration_times_bitrate() {
        // 200 s at 192 kbps = 200 * 24_000 bytes.
        assert_eq!(
            preset(Format::M4a, Quality::High).estimate_bytes(Duration::from_secs(200)),
            4_800_000
        );
        assert_eq!(
            preset(Format::Mp3, Quality::Small).estimate_bytes(Duration::ZERO),
            0
        );
        assert_eq!(
            preset(Format::Mp3, Quality::High).estimate_bytes(Duration::MAX),
            u64::MAX
        );
    }

    #[test]
    fn labels_show_the_approximate_bitrate() {
        assert_eq!(Quality::High.label(Format::Mp3), "High (about 190 kbps)");
        assert_eq!(Quality::Small.label(Format::M4a), "Small (about 128 kbps)");
        assert_eq!(Quality::High.label(Format::Opus), "about 160 kbps");
    }
}
