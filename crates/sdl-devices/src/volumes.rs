//! The device list (F10): `/Volumes` entries asked with `diskutil info -plist`, the watch on
//! `/Volumes`, and a device's top-level folders.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sdl_core::Device;
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::DevicesError;
use crate::paths::io_err;

/// The keys of `diskutil info -plist` this crate reads (real output, macOS 27.2).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeInfo {
    mount_point: Option<PathBuf>,
    volume_name: Option<String>,
    #[serde(rename = "VolumeUUID")]
    volume_uuid: Option<String>,
    #[serde(default)]
    removable_media_or_external_device: bool,
    #[serde(rename = "APFSContainerFree")]
    apfs_container_free: Option<u64>,
    free_space: Option<u64>,
    /// `Disk Image` for a mounted disk image; a physical drive names its bus (`USB`, `Apple
    /// Fabric`, ...).
    bus_protocol: Option<String>,
}

impl VolumeInfo {
    fn is_disk_image(&self) -> bool {
        self.bus_protocol.as_deref() == Some("Disk Image")
    }
}

/// Every entry of `/Volumes` asked with `diskutil info -plist <mount point>`
/// (`tokio::process::Command`), parsed with `plist` and `serde`, kept when external or
/// removable. A device is identified by its volume UUID, falling back to its name
/// ([`Device::id`]).
///
/// Keys, read from real `diskutil info -plist` output on this Mac (macOS 27.2): `MountPoint`,
/// `VolumeName`, `VolumeUUID` (present on both the APFS system volume and a FAT32 volume),
/// `RemovableMediaOrExternalDevice` (true for a FAT32 disk image, false for the internal system
/// and `Recovery` volumes; it is the "external or removable" test), `APFSContainerFree` (APFS
/// only) and `FreeSpace` (0 on APFS, the real free bytes on FAT32). Free space is
/// `APFSContainerFree` when present, else `FreeSpace`. A mounted disk image counts as removable,
/// so it is listed too, and `BusProtocol` is `Disk Image` for it (`Apple Fabric` for the internal
/// disk), which is what [`Device::disk_image`] is read from.
///
/// Entries are sorted by name. An entry `diskutil` does not know as a volume, and any name that
/// starts with a dot, is left out. Only reads; nothing is mounted or changed.
///
/// # Errors
/// [`DevicesError::Io`] when `/Volumes` cannot be read; [`DevicesError::Diskutil`] when
/// `diskutil` cannot be run or its answer cannot be read.
pub async fn list() -> Result<Vec<Device>, DevicesError> {
    list_in(Path::new("/Volumes")).await
}

async fn list_in(volumes: &Path) -> Result<Vec<Device>, DevicesError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(volumes).map_err(io_err(volumes))? {
        let entry = entry.map_err(io_err(volumes))?;
        if !entry.file_name().to_string_lossy().starts_with('.') {
            entries.push(entry.path());
        }
    }
    entries.sort();
    let asked = futures::future::join_all(entries.iter().map(|path| device_at(path))).await;
    let mut devices = Vec::new();
    for device in asked {
        devices.extend(device?);
    }
    Ok(devices)
}

/// The device mounted at `mount_point`, or `None` when it is not external or removable (or
/// `diskutil` does not know it as a volume).
async fn device_at(mount_point: &Path) -> Result<Option<Device>, DevicesError> {
    let output = Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(mount_point)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| DevicesError::Diskutil {
            action: "info",
            message: error.to_string(),
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let info: VolumeInfo =
        plist::from_bytes(&output.stdout).map_err(|error| DevicesError::Diskutil {
            action: "info",
            message: format!(
                "could not read its answer for {}: {error}",
                mount_point.display()
            ),
        })?;
    if !info.removable_media_or_external_device {
        return Ok(None);
    }
    let disk_image = info.is_disk_image();
    let name = info
        .volume_name
        .filter(|name| !name.is_empty())
        .or_else(|| {
            mount_point
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    Ok(Some(Device {
        name,
        mount_point: info
            .mount_point
            .unwrap_or_else(|| mount_point.to_path_buf()),
        volume_uuid: info.volume_uuid.filter(|uuid| !uuid.is_empty()),
        free_bytes: info.apfs_container_free.or(info.free_space).unwrap_or(0),
        disk_image,
    }))
}

/// A watch on `/Volumes` (`notify`, non-recursive): says when a volume appeared or went, so the
/// open form can list again. Dropping it stops the watch.
pub struct DeviceWatch {
    _watcher: RecommendedWatcher,
    changes: UnboundedReceiver<()>,
}

impl std::fmt::Debug for DeviceWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceWatch").finish_non_exhaustive()
    }
}

impl DeviceWatch {
    /// Wait until something in the watched folder changed. `None` when the watch has ended. A
    /// burst of changes may arrive as several wake-ups; the caller lists again each time.
    pub async fn changed(&mut self) -> Option<()> {
        self.changes.recv().await
    }
}

/// Watch `/Volumes`.
///
/// # Errors
/// [`DevicesError::Watch`] when the watch cannot be set up.
pub fn watch() -> Result<DeviceWatch, DevicesError> {
    watch_dir(Path::new("/Volumes"))
}

/// Watch `dir` non-recursively (the real call watches `/Volumes`). Reading a folder is not a
/// change, so access events do not wake the caller.
fn watch_dir(dir: &Path) -> Result<DeviceWatch, DevicesError> {
    let (tx, changes) = unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|event| !event.kind.is_access()) {
            // The receiver is gone when the watch was dropped; nobody is left to tell.
            let _ = tx.send(());
        }
    })
    .map_err(|error| DevicesError::Watch(error.to_string()))?;
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(|error| DevicesError::Watch(error.to_string()))?;
    Ok(DeviceWatch {
        _watcher: watcher,
        changes,
    })
}

/// The device's top-level folders (folders directly in its root, not starting with a dot), sorted,
/// for the From device tick list (F10).
///
/// # Errors
/// [`DevicesError::Io`] when the device's root cannot be read.
pub async fn folders(device: &Device) -> Result<Vec<String>, DevicesError> {
    // One small folder: read it on the calling task.
    std::future::ready(folders_in(&device.mount_point)).await
}

fn folders_in(root: &Path) -> Result<Vec<String>, DevicesError> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root).map_err(io_err(root))? {
        let entry = entry.map_err(io_err(root))?;
        let is_folder = entry.file_type().map_err(io_err(&entry.path()))?.is_dir();
        if let (true, Ok(name)) = (is_folder, entry.file_name().into_string())
            && !name.starts_with('.')
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn the_internal_disk_is_not_a_device_and_its_free_space_is_the_container_key()
    -> TestResult {
        let output = Command::new("diskutil")
            .args(["info", "-plist", "/"])
            .output()
            .await?;
        let info: VolumeInfo = plist::from_bytes(&output.stdout)?;
        assert!(!info.removable_media_or_external_device);
        assert!(info.apfs_container_free.is_some_and(|free| free > 0));
        assert_eq!(info.free_space.unwrap_or(0), 0);
        assert!(device_at(Path::new("/")).await?.is_none());
        Ok(())
    }

    /// Trimmed from real `diskutil info -plist` output (macOS 27.2) for a 40 MB FAT32 image
    /// attached with `hdiutil`.
    const DISK_IMAGE_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>BusProtocol</key>
	<string>Disk Image</string>
	<key>FilesystemName</key>
	<string>MS-DOS FAT32</string>
	<key>FreeSpace</key>
	<integer>41280000</integer>
	<key>Internal</key>
	<false/>
	<key>MediaName</key>
	<string>Disk Image</string>
	<key>MountPoint</key>
	<string>/Volumes/SDLTEST</string>
	<key>RemovableMediaOrExternalDevice</key>
	<true/>
	<key>VirtualOrPhysical</key>
	<string>Virtual</string>
	<key>VolumeName</key>
	<string>SDLTEST</string>
	<key>VolumeUUID</key>
	<string>D1206E50-7C38-3AAE-B4DB-0B5104478293</string>
</dict>
</plist>"#;

    /// Trimmed from real `diskutil info -plist /` output (macOS 27.2).
    const INTERNAL_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>APFSContainerFree</key>
	<integer>866613555200</integer>
	<key>BusProtocol</key>
	<string>Apple Fabric</string>
	<key>FreeSpace</key>
	<integer>0</integer>
	<key>Internal</key>
	<true/>
	<key>RemovableMediaOrExternalDevice</key>
	<false/>
</dict>
</plist>"#;

    #[test]
    fn the_bus_protocol_tells_a_disk_image_from_a_drive() -> TestResult {
        let image: VolumeInfo = plist::from_bytes(DISK_IMAGE_PLIST.as_bytes())?;
        assert!(image.is_disk_image());
        assert!(image.removable_media_or_external_device);
        assert_eq!(image.volume_name.as_deref(), Some("SDLTEST"));
        assert_eq!(image.free_space, Some(41_280_000));
        let internal: VolumeInfo = plist::from_bytes(INTERNAL_PLIST.as_bytes())?;
        assert!(!internal.is_disk_image());
        Ok(())
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_volume_is_left_out() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("NotAVolume"))?;
        std::fs::create_dir(dir.path().join(".hidden"))?;
        assert_eq!(list_in(dir.path()).await?, Vec::new());
        Ok(())
    }

    #[tokio::test]
    async fn the_real_volumes_folder_lists_without_error() -> TestResult {
        // Read-only: asks diskutil about each entry; the internal disk never appears.
        let devices = list().await?;
        assert!(devices.iter().all(|device| device.name != "Macintosh HD"));
        Ok(())
    }

    #[test]
    fn folders_are_the_top_level_folders_without_dot_names() -> TestResult {
        let dir = tempfile::tempdir()?;
        for name in ["Music", "Podcasts", ".Trashes", ".fseventsd"] {
            std::fs::create_dir(dir.path().join(name))?;
        }
        std::fs::write(dir.path().join("readme.txt"), b"x")?;
        std::fs::create_dir_all(dir.path().join("Music/Deep"))?;
        assert_eq!(folders_in(dir.path())?, ["Music", "Podcasts"]);
        Ok(())
    }

    #[tokio::test]
    async fn the_watch_notices_a_volume_appearing_and_going() -> TestResult {
        let dir = tempfile::tempdir()?;
        let mut watch = watch_dir(dir.path())?;
        let volume = dir.path().join("NEWVOLUME");
        std::fs::create_dir(&volume)?;
        let seen = tokio::time::timeout(Duration::from_secs(10), watch.changed()).await?;
        assert_eq!(seen, Some(()));
        // Let the first burst settle, then look for the removal.
        tokio::time::sleep(Duration::from_millis(500)).await;
        while tokio::time::timeout(Duration::from_millis(200), watch.changed())
            .await
            .is_ok()
        {}
        std::fs::remove_dir(&volume)?;
        let seen = tokio::time::timeout(Duration::from_secs(10), watch.changed()).await?;
        assert_eq!(seen, Some(()));
        Ok(())
    }
}
