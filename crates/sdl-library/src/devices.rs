//! Devices (F4): find mounted external or removable volumes, plan a sync with one, run it, and
//! clean up and unmount. Drives are found and unmounted with macOS `diskutil`
//! (`tokio::process::Command`, arguments passed separately).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use sdl_core::paths::{device_music_dir, tmp_dir};
use sdl_core::{Device, DeviceChoice, Direction};
use serde::Deserialize;
use tokio::process::Command;

use crate::files::{Plan, plan_jobs};
use crate::{LibraryError, blocking, io_err};

/// Dot-folders macOS creates at a volume root.
const DOT_FOLDERS: [&str; 3] = [".Spotlight-V100", ".Trashes", ".fseventsd"];

/// Every mounted external or removable volume, sorted by name: each entry of `/Volumes` asked with
/// `diskutil info -plist <mount point>`, parsed with `plist::from_bytes` into a `serde` struct, and
/// kept when `Internal` is false or `RemovableMediaOrExternalDevice` is true. Keys, as in the real
/// output on this Mac (`man diskutil` does not list them): `VolumeName`, `VolumeUUID`,
/// `MountPoint`, `Internal`, `RemovableMediaOrExternalDevice`, `FreeSpace` (0 on APFS) and
/// `APFSContainerFree` (APFS only); free space is `APFSContainerFree` when present, else
/// `FreeSpace`. An entry that `diskutil` cannot describe (a network share, a symlink such as
/// `Macintosh HD`, an `Error` plist) is not a device and is left out, so one odd mount never hides
/// the others.
///
/// # Errors
/// `/Volumes` cannot be read, or `diskutil` cannot be started.
pub async fn list() -> Result<Vec<Device>, LibraryError> {
    let volumes = Path::new("/Volumes");
    let mut entries = tokio::fs::read_dir(volumes)
        .await
        .map_err(io_err(volumes))?;
    let mut mounts = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(io_err(volumes))? {
        if !entry.file_name().to_string_lossy().starts_with('.') {
            mounts.push(entry.path());
        }
    }
    let described = futures::future::join_all(mounts.iter().map(|mount| describe(mount))).await;
    let mut devices: Vec<Device> = described
        .into_iter()
        .filter_map(|info| info.ok().and_then(VolumeInfo::into_device))
        .collect();
    devices.sort_by_key(|device| device.name.to_lowercase());
    Ok(devices)
}

/// The keys of `diskutil info -plist` that sdl reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeInfo {
    /// Set on the plist `diskutil` prints when it cannot describe the path.
    #[serde(default)]
    error: bool,
    volume_name: Option<String>,
    #[serde(rename = "VolumeUUID")]
    volume_uuid: Option<String>,
    mount_point: Option<PathBuf>,
    internal: Option<bool>,
    removable_media_or_external_device: Option<bool>,
    free_space: Option<u64>,
    #[serde(rename = "APFSContainerFree")]
    apfs_container_free: Option<u64>,
}

impl VolumeInfo {
    /// `APFSContainerFree` on APFS (where `FreeSpace` is 0), else `FreeSpace`.
    fn free_bytes(&self) -> u64 {
        self.apfs_container_free.or(self.free_space).unwrap_or(0)
    }

    /// The volume as a device, when it is mounted and external or removable.
    fn into_device(self) -> Option<Device> {
        let free_bytes = self.free_bytes();
        let external =
            self.internal == Some(false) || self.removable_media_or_external_device == Some(true);
        if self.error || !external {
            return None;
        }
        let mount_point = self
            .mount_point
            .filter(|path| !path.as_os_str().is_empty())?;
        let name = self
            .volume_name
            .filter(|name| !name.is_empty())
            .or_else(|| {
                mount_point
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })?;
        Some(Device {
            name,
            volume_uuid: self.volume_uuid.filter(|uuid| !uuid.is_empty()),
            free_bytes,
            mount_point,
        })
    }
}

/// Ask `diskutil info -plist <mount>` and parse the answer.
async fn describe(mount: &Path) -> Result<VolumeInfo, LibraryError> {
    let mount_point = mount.to_string_lossy().into_owned();
    let bytes = run_diskutil("info", &["info", "-plist", &mount_point]).await?;
    plist::from_bytes(&bytes).map_err(|error| LibraryError::Diskutil {
        action: "info",
        message: error.to_string(),
    })
}

/// Plan a sync of `music_root` with `device` as chosen in `choice`, with
/// [`crate::files::plan_jobs`]:
///
/// * To device: the ticked library folders into `<device>/Music`
///   ([`sdl_core::paths::device_music_dir`]) with `choice.format` (`None` copies as is).
///   Conversions are staged in `<music_root>/.sdl-tmp` ([`sdl_core::paths::tmp_dir`]), so the
///   device only ever receives finished files. `free_bytes` is the device's free space asked again
///   with `diskutil info -plist` now, so the summary is current.
/// * From device: the ticked folders of `<device>/Music` into `music_root`, as is.
///   `free_bytes` is `None` (`diskutil` describes volumes, not a folder such as the music root).
///
/// Never deletes, never plans a dot-named path. Run the result with [`crate::files::run`].
///
/// # Errors
/// A read, walk or `diskutil` failure.
pub async fn plan(
    music_root: &Path,
    device: &Device,
    choice: &DeviceChoice,
) -> Result<Plan, LibraryError> {
    let on_device = device_music_dir(&device.mount_point);
    let root = music_root.to_path_buf();
    let folders = choice.folders.clone();
    match choice.direction {
        Direction::ToDevice => {
            let unreadable = || LibraryError::Diskutil {
                action: "info",
                message: format!("{} could not be read. Is it still plugged in?", device.name),
            };
            let info = describe(&device.mount_point)
                .await
                .ok()
                .filter(|info| !info.error)
                .ok_or_else(unreadable)?;
            let free = info.free_bytes();
            let format = choice.format;
            let staging = format.map(|_| tmp_dir(music_root));
            let planned = blocking(move || plan_jobs(&root, &folders, &on_device, format)).await?;
            Ok(Plan::new(planned, Some(free), staging))
        }
        Direction::FromDevice => {
            let planned = blocking(move || plan_jobs(&on_device, &folders, &root, None)).await?;
            Ok(Plan::new(planned, None, None))
        }
    }
}

/// Clean up and unmount: remove `.Spotlight-V100`, `.Trashes` and `.fseventsd` from the device
/// root, then `diskutil unmount <mount point>`. Call only after the owner confirms.
///
/// # Errors
/// A removal or `diskutil` failure.
pub async fn eject(device: &Device) -> Result<(), LibraryError> {
    let mount = device.mount_point.clone();
    blocking(move || clean_dot_folders(&mount)).await?;
    let mount_point = device.mount_point.to_string_lossy().into_owned();
    run_diskutil("unmount", &["unmount", &mount_point])
        .await
        .map(|_| ())
}

/// Remove the three macOS dot-folders from a volume root.
fn clean_dot_folders(mount: &Path) -> Result<(), LibraryError> {
    for name in DOT_FOLDERS {
        let path = mount.join(name);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&path)(e)),
        }
    }
    Ok(())
}

/// Run `diskutil` with separate arguments (no shell) and return stdout (bytes, as
/// `plist::from_bytes` takes them).
async fn run_diskutil(action: &'static str, args: &[&str]) -> Result<Vec<u8>, LibraryError> {
    let out = Command::new("diskutil")
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| LibraryError::Diskutil {
            action,
            message: e.to_string(),
        })?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    // diskutil prints some failures on stdout only.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let message = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|text| !text.is_empty())
        .map_or_else(|| out.status.to_string(), str::to_owned);
    Err(LibraryError::Diskutil { action, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(path: &Path, bytes: &[u8]) -> TestResult {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    #[test]
    fn cleans_only_the_three_dot_folders() -> TestResult {
        let card = tempfile::tempdir()?;
        for name in DOT_FOLDERS {
            write(&card.path().join(name).join("x"), b"x")?;
        }
        write(&card.path().join(".diskos/keep"), b"k")?;
        write(&card.path().join("Music/a.opus"), b"a")?;
        clean_dot_folders(card.path())?;
        for name in DOT_FOLDERS {
            assert!(!card.path().join(name).exists());
        }
        assert!(card.path().join(".diskos/keep").exists());
        assert!(card.path().join("Music/a.opus").exists());
        clean_dot_folders(card.path())?;
        Ok(())
    }

    /// The keys sdl reads, as `diskutil info -plist` prints them on this Mac for an external FAT
    /// volume; `internal` and `free` vary per test.
    fn info(internal: bool, removable: bool, free: u64, apfs_free: Option<u64>) -> String {
        let apfs = apfs_free.map_or_else(String::new, |n| {
            format!("<key>APFSContainerFree</key><integer>{n}</integer>")
        });
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
             <key>VolumeName</key><string>SNOWSKY</string>\
             <key>VolumeUUID</key><string>AAAA-1111</string>\
             <key>MountPoint</key><string>/Volumes/SNOWSKY</string>\
             <key>Internal</key><{internal}/>\
             <key>RemovableMediaOrExternalDevice</key><{removable}/>\
             <key>FreeSpace</key><integer>{free}</integer>{apfs}\
             <key>Size</key><integer>9</integer></dict></plist>"
        )
    }

    fn device(text: &str) -> Result<Option<Device>, plist::Error> {
        Ok(plist::from_bytes::<VolumeInfo>(text.as_bytes())?.into_device())
    }

    #[test]
    fn external_or_removable_volumes_are_devices() -> TestResult {
        let found = device(&info(false, true, 500, None))?.ok_or("not kept")?;
        assert_eq!(found.name, "SNOWSKY");
        assert_eq!(found.id(), "AAAA-1111");
        assert_eq!(found.mount_point, PathBuf::from("/Volumes/SNOWSKY"));
        assert_eq!(found.free_bytes, 500);
        assert!(device(&info(false, false, 1, None))?.is_some());
        assert!(device(&info(true, true, 1, None))?.is_some());
        assert!(device(&info(true, false, 1, None))?.is_none());
        Ok(())
    }

    #[test]
    fn apfs_free_space_comes_from_the_container() -> TestResult {
        let found = device(&info(false, true, 0, Some(7_000)))?.ok_or("not kept")?;
        assert_eq!(found.free_bytes, 7_000);
        Ok(())
    }

    #[test]
    fn what_diskutil_cannot_describe_is_not_a_device() -> TestResult {
        let error = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
            <key>Error</key><true/><key>ErrorMessage</key><string>Could not find disk</string>\
            </dict></plist>";
        assert!(device(error)?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn from_device_plans_ticked_music_folders_as_is_and_skips_dot_names() -> TestResult {
        let dir = tempfile::tempdir()?;
        let lib = dir.path().join("lib");
        let card = dir.path().join("card");
        write(&card.join("Music/A/one.opus"), b"12345")?;
        write(&card.join("Music/A/two.mp3"), b"123")?;
        write(&card.join("Music/A/.hidden.opus"), b"1")?;
        write(&card.join("Music/.diskos/x.opus"), b"1")?;
        write(&card.join("Music/B/other.opus"), b"1")?;
        write(&lib.join("A/two.m4a"), b"1")?;
        let device = Device {
            name: "CARD".into(),
            mount_point: card,
            volume_uuid: None,
            free_bytes: 0,
        };
        let choice = DeviceChoice {
            folders: vec!["A".into()],
            direction: Direction::FromDevice,
            format: Some(sdl_core::Format::Mp3),
        };
        let plan = plan(&lib, &device, &choice).await?;
        // one.opus is new (format ignored: copied as is); two exists in the library as m4a.
        assert_eq!(plan.summary.files, 1);
        assert_eq!(plan.summary.already, 1);
        assert_eq!(plan.summary.bytes, 5);
        assert_eq!(plan.summary.free_bytes, None);
        assert!(plan.summary.can_start());
        Ok(())
    }
}
