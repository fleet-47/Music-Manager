//! Devices (F10) and the choices remembered for them, and the choices Convert (F9) remembers.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Format, LibraryId, Preset, Quality};

/// An external or removable volume, mounted or not, as `diskutil info -plist` describes it when it is
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
    /// A mounted disk image rather than a physical drive (`BusProtocol` is `Disk Image`).
    pub disk_image: bool,
    /// The device identifier (`disk4s2`) of a volume that is plugged in but not mounted; `None`
    /// for a mounted one. Such a device has no mount point and its free space is not known.
    pub not_mounted: Option<String>,
}

impl Device {
    /// The name as shown to the owner: the volume name, followed by " (disk image)" for a mounted
    /// disk image or " (not mounted)" for a volume that is plugged in but not mounted (F10). The
    /// one place this wording lives.
    #[must_use]
    pub fn label(&self) -> String {
        if self.not_mounted.is_some() {
            format!("{} (not mounted)", self.name)
        } else if self.disk_image {
            format!("{} (disk image)", self.name)
        } else {
            self.name.clone()
        }
    }

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
    /// The device's ticked top-level folders into the library.
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

/// What to take from the library: the whole library and/or some playlists (a tick list in the
/// Convert and To-device forms). Playlists are named by their file stem, as listed by
/// `sdl_library::playlists::list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// Every file of the chosen library ([`crate::library_files`]).
    #[serde(default)]
    pub whole_library: bool,
    /// The ticked playlists' file stems. Their songs are included, and each playlist file is
    /// written beside the converted or copied files.
    #[serde(default)]
    pub playlists: Vec<String>,
}

impl Selection {
    /// Whether nothing is ticked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.whole_library && self.playlists.is_empty()
    }
}

/// What the owner chose for a device sync, remembered per device ([`Device::id`]).
///
/// Every field has a default, so a file written by an older version loads. Keys this version does
/// not know are ignored, among them the old `folders` key: it named library folders to copy to the
/// device, a meaning that no longer exists, so it is not read as the From device folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceChoice {
    /// The device's name as shown to the owner ([`Device::label`]) when this was saved, so the
    /// main panel can name the device after a restart, before it is plugged in. Empty in a file
    /// written before names were kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The library the sync reads from (To device) or writes into (From device). Remembered per
    /// device (F12).
    #[serde(default, skip_serializing_if = "LibraryId::is_own")]
    pub library: LibraryId,
    /// The library the last To-device run copied from, when it differs from the one now chosen the
    /// form's note says the two merge on the device (F12). `None` before any such run.
    #[serde(default)]
    pub last_synced: Option<LibraryId>,
    /// Copy direction.
    #[serde(default)]
    pub direction: Direction,
    /// To device: what to copy from the library.
    #[serde(default)]
    pub what: Selection,
    /// From device: the ticked top-level folders of the device (folders directly in its root).
    #[serde(default)]
    pub from_folders: Vec<String>,
    /// Format on the device ([`Format::DEVICE`]): `None` copies as is. To device only.
    #[serde(default)]
    pub format: Option<Format>,
    /// Quality, when `format` is set.
    #[serde(default)]
    pub quality: Quality,
    /// Keep cover art, when `format` is set. Default on.
    #[serde(default = "yes")]
    pub keep_cover: bool,
}

impl DeviceChoice {
    /// The library the device was last synced from, when that is not `chosen`: the device holds
    /// that other person's music, and syncing `chosen` merges the two there (F12).
    #[must_use]
    pub fn merges_with(&self, chosen: &LibraryId) -> Option<&LibraryId> {
        self.last_synced.as_ref().filter(|last| *last != chosen)
    }

    /// The preset to convert with, or `None` when files are copied as they are.
    #[must_use]
    pub fn preset(&self) -> Option<Preset> {
        self.format.map(|format| Preset {
            format,
            quality: self.quality,
            keep_cover: self.keep_cover,
        })
    }
}

impl Default for DeviceChoice {
    fn default() -> Self {
        Self {
            name: String::new(),
            library: LibraryId::default(),
            last_synced: None,
            direction: Direction::default(),
            what: Selection::default(),
            from_folders: Vec::new(),
            format: None,
            quality: Quality::default(),
            keep_cover: true,
        }
    }
}

/// What the owner chose for Convert, remembered from run to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertChoice {
    /// The library to convert from (F12).
    #[serde(default, skip_serializing_if = "LibraryId::is_own")]
    pub library: LibraryId,
    /// What to convert.
    #[serde(default)]
    pub what: Selection,
    /// Target format: MP3 or M4A ([`Format::CONVERT`]).
    #[serde(default = "ConvertChoice::default_format")]
    pub format: Format,
    /// Quality.
    #[serde(default)]
    pub quality: Quality,
    /// Keep cover art. Default on.
    #[serde(default = "yes")]
    pub keep_cover: bool,
    /// Destination folder as typed; a leading `~` is expanded with
    /// [`crate::config::expand_home`] when it is used. A destination inside `music_root` is
    /// refused in the plan (F9).
    #[serde(default = "ConvertChoice::default_destination")]
    pub destination: PathBuf,
}

impl ConvertChoice {
    /// The preset this choice converts with.
    #[must_use]
    pub const fn preset(&self) -> Preset {
        Preset {
            format: self.format,
            quality: self.quality,
            keep_cover: self.keep_cover,
        }
    }

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
            library: LibraryId::default(),
            what: Selection::default(),
            format: Self::default_format(),
            quality: Quality::default(),
            keep_cover: true,
            destination: Self::default_destination(),
        }
    }
}

const fn yes() -> bool {
    true
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
            disk_image: false,
            not_mounted: None,
        }
    }

    #[test]
    fn id_is_the_uuid_else_the_name() {
        assert_eq!(device(Some("U-1")).id(), "U-1");
        assert_eq!(device(None).id(), "SNOWSKY");
    }

    #[test]
    fn a_volume_that_is_not_mounted_is_labelled_after_its_name() {
        let mut unmounted = device(None);
        unmounted.not_mounted = Some("disk4s2".into());
        assert_eq!(unmounted.label(), "SNOWSKY (not mounted)");
    }

    #[test]
    fn a_disk_image_is_labelled_after_its_name() {
        let mut image = device(None);
        assert_eq!(image.label(), "SNOWSKY");
        image.disk_image = true;
        assert_eq!(image.label(), "SNOWSKY (disk image)");
    }

    #[test]
    fn convert_defaults() {
        let choice = ConvertChoice::default();
        assert_eq!(choice.format, Format::Mp3);
        assert_eq!(choice.destination, PathBuf::from("~/Music-converted"));
        assert!(choice.what.is_empty());
        assert!(choice.keep_cover);
        assert_eq!(choice.preset().quality, Quality::High);
    }

    #[test]
    fn a_device_copies_as_is_unless_a_format_is_chosen() {
        let mut choice = DeviceChoice::default();
        assert_eq!(choice.preset(), None);
        choice.format = Some(Format::M4a);
        assert_eq!(choice.preset().map(|p| p.format), Some(Format::M4a));
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
