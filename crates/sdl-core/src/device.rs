//! Devices (F4) and the choices remembered for them, and the choices Convert (F10) remembers.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Format;

/// A mounted external or removable volume, as `diskutil info -plist` describes it when it is
/// listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Volume name, as shown to the owner (`VolumeName`).
    pub name: String,
    /// Where it is mounted (`MountPoint`).
    pub mount_point: PathBuf,
    /// Volume UUID, when the volume has one (`VolumeUUID`).
    pub volume_uuid: Option<String>,
    /// Free space in bytes when it was listed (`APFSContainerFree` on APFS, else `FreeSpace`).
    pub free_bytes: u64,
}

impl Device {
    /// How the device is remembered: its volume UUID, falling back to its name.
    #[must_use]
    pub fn id(&self) -> &str {
        self.volume_uuid.as_deref().unwrap_or(&self.name)
    }
}

/// Which way a device sync copies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Library to device.
    #[default]
    ToDevice,
    /// Device `Music` folder into the library.
    FromDevice,
}

impl Direction {
    /// Both, in the order of the choice row.
    pub const ALL: [Self; 2] = [Self::ToDevice, Self::FromDevice];

    /// Plain name for a choice row.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ToDevice => "To device",
            Self::FromDevice => "From device",
        }
    }
}

/// What the owner chose for a device sync, remembered per device ([`Device::id`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceChoice {
    /// Ticked top-level folders on the source side.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Copy direction.
    #[serde(default)]
    pub direction: Direction,
    /// Format on the device ([`Format::DEVICE`]): `None` copies as is. Used for
    /// [`Direction::ToDevice`] only; From device always copies as is.
    #[serde(default)]
    pub format: Option<Format>,
}

/// What the owner chose for Convert, remembered from run to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertChoice {
    /// Ticked top-level library folders.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Target format: MP3, M4A or Opus ([`Format::CONVERT`]).
    #[serde(default = "ConvertChoice::default_format")]
    pub format: Format,
    /// Destination folder as typed; a leading `~` is expanded with
    /// [`crate::config::expand_home`] when it is used.
    #[serde(default = "ConvertChoice::default_destination")]
    pub destination: PathBuf,
}

impl ConvertChoice {
    /// The destination with a leading `~` expanded to the home directory. `None` when the home
    /// directory cannot be determined and the destination starts with `~`.
    #[must_use]
    pub fn destination_path(&self) -> Option<PathBuf> {
        if !self.destination.starts_with("~") {
            return Some(self.destination.clone());
        }
        let base = directories::BaseDirs::new()?;
        Some(crate::config::expand_home(
            &self.destination,
            base.home_dir(),
        ))
    }

    fn default_format() -> Format {
        Format::Mp3
    }

    fn default_destination() -> PathBuf {
        PathBuf::from("~/Music-converted")
    }
}

impl Default for ConvertChoice {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            format: Self::default_format(),
            destination: Self::default_destination(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uuid: Option<&str>) -> Device {
        Device {
            name: "SNOWSKY".into(),
            mount_point: PathBuf::from("/Volumes/SNOWSKY"),
            volume_uuid: uuid.map(str::to_owned),
            free_bytes: 1,
        }
    }

    #[test]
    fn id_is_the_uuid_else_the_name() {
        assert_eq!(device(Some("U-1")).id(), "U-1");
        assert_eq!(device(None).id(), "SNOWSKY");
    }

    #[test]
    fn convert_defaults() {
        let choice = ConvertChoice::default();
        assert_eq!(choice.format, Format::Mp3);
        assert_eq!(choice.destination, PathBuf::from("~/Music-converted"));
        assert_eq!(choice.folders, Vec::<String>::new());
    }

    #[test]
    fn an_absolute_destination_is_used_as_typed() {
        let choice = ConvertChoice {
            destination: PathBuf::from("/out/x"),
            ..ConvertChoice::default()
        };
        assert_eq!(choice.destination_path(), Some(PathBuf::from("/out/x")));
    }
}
