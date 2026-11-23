//! The device list (F10): `/Volumes` entries asked with `diskutil info -plist`, plugged-in
//! volumes that are not mounted (`diskutil list -plist external`), mounting one, the watch on
//! `/Volumes`, and a device's top-level folders.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sdl_core::Device;
use sdl_core::process::{self, ToolError};
use serde::Deserialize;
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
    /// Empty for a partition that holds no file system (`Apple_APFS_ISC`, a partition map).
    filesystem_type: Option<String>,
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
/// (`sdl_core::process::run`), parsed with `plist` and `serde`, kept when external or
/// removable; then the external volumes that are plugged in but not mounted ([`not_mounted`]). A
/// device is identified by its volume UUID, falling back to its name ([`Device::id`]).
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
/// Mounted entries are sorted by name, the not-mounted ones follow. An entry `diskutil` does not
/// know as a volume, and any name that starts with a dot, is left out. Only reads; nothing is
/// mounted or changed.
///
/// # Errors
/// [`DevicesError::Io`] when `/Volumes` cannot be read; [`DevicesError::Diskutil`] when
/// `diskutil` cannot be run or its answer cannot be read.
pub async fn list() -> Result<Vec<Device>, DevicesError> {
    let mut devices = list_in(Path::new("/Volumes")).await?;
    let unmounted = not_mounted().await?;
    for device in unmounted {
        if devices.iter().all(|other| other.id() != device.id()) {
            devices.push(device);
        }
    }
    Ok(devices)
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

/// `diskutil info -plist <target>` (a mount point or a device identifier), or `None` when
/// `diskutil` does not know it as a volume.
async fn info_of(target: &OsStr) -> Result<Option<VolumeInfo>, DevicesError> {
    let args = [
        OsString::from("info"),
        OsString::from("-plist"),
        target.to_owned(),
    ];
    let stdout = match process::run("diskutil", &args).await {
        Ok(stdout) => stdout,
        Err(ToolError::Failed { .. }) => return Ok(None),
        Err(other) => {
            return Err(DevicesError::Diskutil {
                action: "info",
                message: other.to_string(),
            });
        }
    };
    plist::from_bytes(&stdout)
        .map(Some)
        .map_err(|error| DevicesError::Diskutil {
            action: "info",
            message: format!(
                "could not read its answer for {}: {error}",
                target.to_string_lossy()
            ),
        })
}

/// The device mounted at `mount_point`, or `None` when it is not external or removable (or
/// `diskutil` does not know it as a volume).
async fn device_at(mount_point: &Path) -> Result<Option<Device>, DevicesError> {
    let Some(info) = info_of(mount_point.as_os_str()).await? else {
        return Ok(None);
    };
    if !info.removable_media_or_external_device {
        return Ok(None);
    }
    Ok(Some(device_of(info, mount_point.to_path_buf(), None)))
}

/// The device `info` describes. `fallback_mount` is used when `info` has no `MountPoint`, and
/// names the device when it has no `VolumeName`.
fn device_of(info: VolumeInfo, fallback_mount: PathBuf, not_mounted: Option<String>) -> Device {
    let disk_image = info.is_disk_image();
    let name = info
        .volume_name
        .filter(|name| !name.is_empty())
        .or_else(|| {
            fallback_mount
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let mount_point = info
        .mount_point
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(fallback_mount);
    Device {
        name,
        mount_point,
        volume_uuid: info.volume_uuid.filter(|uuid| !uuid.is_empty()),
        free_bytes: info.apfs_container_free.or(info.free_space).unwrap_or(0),
        disk_image,
        not_mounted,
    }
}

/// The keys of `diskutil list -plist` this crate reads. Real output (macOS 27.2): the top level
/// has `AllDisksAndPartitions`; a whole disk has `Partitions` (each with `Content`,
/// `DeviceIdentifier`, `Size`) or, for an APFS container, `APFSVolumes` (each with
/// `DeviceIdentifier`, `VolumeName`, `VolumeUUID`, and `MountPoint` when mounted).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskList {
    #[serde(default)]
    all_disks_and_partitions: Vec<WholeDisk>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WholeDisk {
    #[serde(default)]
    partitions: Vec<Slice>,
    #[serde(default, rename = "APFSVolumes")]
    apfs_volumes: Vec<Slice>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Slice {
    content: Option<String>,
    device_identifier: String,
    mount_point: Option<PathBuf>,
}

impl DiskList {
    /// The identifiers of the slices that are not mounted and may hold a user file system:
    /// partitions other than `EFI` and the APFS container, preboot and recovery partitions
    /// (`Apple_APFS*`), and APFS volumes.
    fn unmounted_candidates(&self) -> Vec<&str> {
        self.all_disks_and_partitions
            .iter()
            .flat_map(|disk| disk.partitions.iter().chain(&disk.apfs_volumes))
            .filter(|slice| {
                slice
                    .mount_point
                    .as_ref()
                    .is_none_or(|path| path.as_os_str().is_empty())
            })
            .filter(|slice| {
                slice
                    .content
                    .as_deref()
                    .is_none_or(|content| content != "EFI" && !content.starts_with("Apple_APFS"))
            })
            .map(|slice| slice.device_identifier.as_str())
            .collect()
    }
}

/// External volumes that are plugged in but not mounted: `diskutil list -plist external`
/// (`sdl_core::process::run`), then `diskutil info -plist <identifier>` for each slice that is not
/// mounted. Kept when the slice holds a file system (`FilesystemType`) and is external or
/// removable, which leaves out `EFI` and partition maps. Read-only.
///
/// # Errors
/// [`DevicesError::Diskutil`] when `diskutil` cannot be run or its answer cannot be read.
async fn not_mounted() -> Result<Vec<Device>, DevicesError> {
    let args = [
        OsString::from("list"),
        OsString::from("-plist"),
        OsString::from("external"),
    ];
    let stdout = process::run("diskutil", &args)
        .await
        .map_err(|error| DevicesError::Diskutil {
            action: "list",
            message: match error {
                ToolError::Failed { reason, .. } => reason,
                other => other.to_string(),
            },
        })?;
    let disks: DiskList = plist::from_bytes(&stdout).map_err(|error| DevicesError::Diskutil {
        action: "list",
        message: format!("could not read its answer: {error}"),
    })?;
    let asked = futures::future::join_all(
        disks
            .unmounted_candidates()
            .into_iter()
            .map(|id| async move { (id, info_of(OsStr::new(id)).await) }),
    )
    .await;
    let mut devices = Vec::new();
    for (id, info) in asked {
        if let Some(info) = info? {
            devices.extend(unmounted_device(id, info));
        }
    }
    Ok(devices)
}

/// The not-mounted device `info` describes, or `None` when it is mounted after all, is not
/// external or removable, or holds no file system.
fn unmounted_device(id: &str, info: VolumeInfo) -> Option<Device> {
    let mounted = info
        .mount_point
        .as_ref()
        .is_some_and(|path| !path.as_os_str().is_empty());
    let has_file_system = info
        .filesystem_type
        .as_ref()
        .is_some_and(|kind| !kind.is_empty());
    if mounted || !has_file_system || !info.removable_media_or_external_device {
        return None;
    }
    let mut device = device_of(info, PathBuf::new(), Some(id.to_owned()));
    if device.name.is_empty() {
        id.clone_into(&mut device.name);
    }
    Some(device)
}

/// Mount a not-mounted device: `diskutil mount <device identifier>`
/// (`sdl_core::process::run`), then `diskutil info -plist <device identifier>` for its mount
/// point, free space and identity. A device that is already mounted is returned as it is.
///
/// # Errors
/// [`DevicesError::Diskutil`] with `diskutil`'s own words when the mount is refused, or when
/// the mounted volume cannot be read.
pub async fn mount(device: &Device) -> Result<Device, DevicesError> {
    let Some(id) = &device.not_mounted else {
        return Ok(device.clone());
    };
    process::run("diskutil", &mount_args(id))
        .await
        .map_err(mount_error)?;
    let info = info_of(OsStr::new(id))
        .await?
        .filter(|info| {
            info.mount_point
                .as_ref()
                .is_some_and(|path| !path.as_os_str().is_empty())
        })
        .ok_or_else(|| DevicesError::Diskutil {
            action: "mount",
            message: format!("{} was mounted but has no mount point", device.name),
        })?;
    Ok(device_of(info, PathBuf::new(), None))
}

fn mount_args(id: &str) -> [OsString; 2] {
    [OsString::from("mount"), OsString::from(id)]
}

fn mount_error(error: ToolError) -> DevicesError {
    DevicesError::Diskutil {
        action: "mount",
        message: match error {
            ToolError::Failed { reason, .. } => reason,
            other => other.to_string(),
        },
    }
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
        let stdout =
            process::run("diskutil", &["info".into(), "-plist".into(), "/".into()]).await?;
        let info: VolumeInfo = plist::from_bytes(&stdout)?;
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

    /// Trimmed from real `diskutil list -plist` output (macOS 27.2): the internal disk's
    /// partitions, and an APFS container with one mounted and one unmounted volume (no
    /// `MountPoint` key). The `EFI` partition is the one slice not from real output (this Mac has
    /// none); its `Content` value is the one `man diskutil` names.
    const DISK_LIST_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AllDisksAndPartitions</key>
	<array>
		<dict>
			<key>Content</key>
			<string>GUID_partition_scheme</string>
			<key>DeviceIdentifier</key>
			<string>disk0</string>
			<key>Partitions</key>
			<array>
				<dict>
					<key>Content</key>
					<string>EFI</string>
					<key>DeviceIdentifier</key>
					<string>disk0s0</string>
					<key>Size</key>
					<integer>209715200</integer>
				</dict>
				<dict>
					<key>Content</key>
					<string>Apple_APFS_ISC</string>
					<key>DeviceIdentifier</key>
					<string>disk0s1</string>
					<key>Size</key>
					<integer>576716800</integer>
				</dict>
				<dict>
					<key>Content</key>
					<string>Apple_APFS</string>
					<key>DeviceIdentifier</key>
					<string>disk0s2</string>
					<key>Size</key>
					<integer>994610155520</integer>
				</dict>
				<dict>
					<key>Content</key>
					<string>Apple_APFS_Recovery</string>
					<key>DeviceIdentifier</key>
					<string>disk0s3</string>
					<key>Size</key>
					<integer>5368664064</integer>
				</dict>
			</array>
			<key>Size</key>
			<integer>1000555581440</integer>
		</dict>
		<dict>
			<key>APFSVolumes</key>
			<array>
				<dict>
					<key>DeviceIdentifier</key>
					<string>disk1s3</string>
					<key>MountPoint</key>
					<string>/System/Volumes/Hardware</string>
					<key>VolumeName</key>
					<string>Hardware</string>
					<key>VolumeUUID</key>
					<string>11716BA2-2F48-4485-BD45-622D701F44DB</string>
				</dict>
				<dict>
					<key>DeviceIdentifier</key>
					<string>disk1s4</string>
					<key>VolumeName</key>
					<string>Recovery</string>
					<key>VolumeUUID</key>
					<string>5BDD733A-FA39-49BA-88A6-D0BBE742CB86</string>
				</dict>
			</array>
			<key>Content</key>
			<string>Apple_APFS_Container</string>
			<key>DeviceIdentifier</key>
			<string>disk1</string>
		</dict>
	</array>
</dict>
</plist>"#;

    /// Trimmed from real `diskutil info -plist disk1s4` output (macOS 27.2): a volume that is
    /// not mounted has an empty `MountPoint`. `RemovableMediaOrExternalDevice` is `false` there
    /// (internal disk).
    fn unmounted_info(external: bool) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>APFSContainerFree</key>
	<integer>551669760</integer>
	<key>BusProtocol</key>
	<string>Apple Fabric</string>
	<key>FilesystemType</key>
	<string>apfs</string>
	<key>FreeSpace</key>
	<integer>0</integer>
	<key>MountPoint</key>
	<string></string>
	<key>RemovableMediaOrExternalDevice</key>
	<{external}/>
	<key>VolumeName</key>
	<string>Recovery</string>
	<key>VolumeUUID</key>
	<string>5BDD733A-FA39-49BA-88A6-D0BBE742CB86</string>
</dict>
</plist>"#
        )
    }

    #[test]
    fn only_slices_that_are_not_mounted_and_not_efi_or_containers_are_asked_about() -> TestResult {
        let disks: DiskList = plist::from_bytes(DISK_LIST_PLIST.as_bytes())?;
        assert_eq!(disks.unmounted_candidates(), ["disk1s4"]);
        Ok(())
    }

    #[test]
    fn an_unmounted_external_volume_is_a_device_without_a_mount_point() -> TestResult {
        let info: VolumeInfo = plist::from_bytes(unmounted_info(true).as_bytes())?;
        let device = unmounted_device("disk1s4", info).ok_or("a device")?;
        assert_eq!(device.name, "Recovery");
        assert_eq!(device.label(), "Recovery (not mounted)");
        assert_eq!(device.not_mounted.as_deref(), Some("disk1s4"));
        assert_eq!(device.mount_point, PathBuf::new());
        assert_eq!(device.id(), "5BDD733A-FA39-49BA-88A6-D0BBE742CB86");
        Ok(())
    }

    #[test]
    fn an_internal_volume_a_mounted_one_and_a_partition_without_a_file_system_are_left_out()
    -> TestResult {
        let internal: VolumeInfo = plist::from_bytes(unmounted_info(false).as_bytes())?;
        assert!(unmounted_device("disk1s4", internal).is_none());
        let mounted: VolumeInfo = plist::from_bytes(DISK_IMAGE_PLIST.as_bytes())?;
        assert!(unmounted_device("disk9s1", mounted).is_none());
        // Real `diskutil info -plist disk0s1` (`Apple_APFS_ISC`): no `FilesystemType`.
        let partition: VolumeInfo = plist::from_bytes(
            br#"<plist version="1.0"><dict>
	<key>MountPoint</key><string></string>
	<key>RemovableMediaOrExternalDevice</key><true/>
	<key>VolumeName</key><string></string>
</dict></plist>"#,
        )?;
        assert!(unmounted_device("disk0s1", partition).is_none());
        Ok(())
    }

    #[test]
    fn the_mount_step_runs_diskutil_mount_with_the_device_identifier() {
        assert_eq!(
            mount_args("disk4s2"),
            [OsString::from("mount"), OsString::from("disk4s2")]
        );
    }

    #[test]
    fn a_refused_mount_shows_diskutils_own_words() {
        let error = mount_error(ToolError::Failed {
            program: "diskutil".into(),
            reason: "Volume on disk4s2 failed to mount".into(),
        });
        assert!(matches!(
            &error,
            DevicesError::Diskutil { action: "mount", message }
                if message == "Volume on disk4s2 failed to mount"
        ));
        assert_eq!(
            error.to_string(),
            "diskutil mount failed: Volume on disk4s2 failed to mount"
        );
    }

    #[tokio::test]
    async fn a_mounted_device_is_not_mounted_again() -> TestResult {
        let info: VolumeInfo = plist::from_bytes(DISK_IMAGE_PLIST.as_bytes())?;
        let device = device_of(info, PathBuf::from("/Volumes/SDLTEST"), None);
        assert_eq!(mount(&device).await?, device);
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
