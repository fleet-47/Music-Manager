//! Devices (F10): the mounted external or removable volumes, plans for copying To and From a
//! device, the copy runs, device playlists, and clean-up and eject.
//!
//! Uses `sdl-library` through its public interface: the converter (`converter::convert_file`,
//! `converter::parallelism`, `converter::output_size`), playlist writing
//! (`playlists::write_for_destination`) and tag reading (`tags::place_from_tags`). Drives are found and ejected with macOS `diskutil`; nothing is
//! mounted or formatted, and nothing is deleted except the macOS items named in
//! [`clean_up_and_eject`].
//!
//! Bodies marked "not built yet" return [`DevicesError::NotBuilt`]; their doc comments are the
//! contract. Names that start with a dot are never copied, in either direction.

use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sdl_core::{Counts, Device, DeviceChoice, EventSender, PlanSummary, StopFlag};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// Errors from the devices crate. Messages are plain words fit for the error popup.
#[derive(Debug, thiserror::Error)]
pub enum DevicesError {
    /// This part of the contract has a signature but no body yet.
    #[error("{0} is not built yet")]
    NotBuilt(&'static str),
    /// `diskutil` could not be run or reported failure.
    #[error("diskutil {action} failed: {message}")]
    Diskutil {
        /// Sub-command.
        action: &'static str,
        /// Reason.
        message: String,
    },
    /// Watching `/Volumes` failed.
    #[error("cannot watch for devices: {0}")]
    Watch(String),
    /// The device has no room for the next file. The files already complete stay.
    #[error("{device} is full")]
    Full {
        /// Device name.
        device: String,
    },
    /// The device went away during the run. The files already complete stay.
    #[error("{device} was removed during the copy")]
    Gone {
        /// Device name.
        device: String,
    },
    /// File system failure.
    #[error("{path}: {source}")]
    Io {
        /// Path concerned.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// A failure in `sdl-library` (converter, playlists, tags).
    #[error(transparent)]
    Library(#[from] sdl_library::LibraryError),
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
/// so it is listed too.
///
/// # Errors
/// [`DevicesError::NotBuilt`] until built; then [`DevicesError::Diskutil`].
pub async fn list() -> Result<Vec<Device>, DevicesError> {
    std::future::ready(Err(DevicesError::NotBuilt("listing devices"))).await
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

/// Watch `dir` non-recursively (the real call watches `/Volumes`).
fn watch_dir(dir: &Path) -> Result<DeviceWatch, DevicesError> {
    let (tx, changes) = unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
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

/// The device's top-level folders (folders directly in its root, not starting with a dot), for the
/// From device tick list (F10).
///
/// # Errors
/// [`DevicesError::NotBuilt`] until built; then [`DevicesError::Io`].
pub async fn folders(device: &Device) -> Result<Vec<String>, DevicesError> {
    let _ = device;
    std::future::ready(Err(DevicesError::NotBuilt("listing a device's folders"))).await
}

/// What a device sync would do, and the summary shown before Start.
#[derive(Debug, Clone)]
pub struct DevicePlan {
    summary: PlanSummary,
    device: Device,
    music_root: PathBuf,
    choice: DeviceChoice,
    /// `(source, destination)` file pairs, copy or convert.
    jobs: Vec<(PathBuf, PathBuf)>,
}

impl DevicePlan {
    /// The summary line's numbers: `N files, about X GB to copy · M already there · Y GB free on
    /// <device>`. When there is not enough room, `can_start()` is false (not a refusal reason of
    /// its own: the form says so in plain words).
    #[must_use]
    pub const fn summary(&self) -> &PlanSummary {
        &self.summary
    }
}

/// Plan a sync for `choice.direction`.
///
/// - **To device**: each song of the chosen selection (`choice.what`: whole library
///   via `sdl_core::library_files`, and each ticked playlist's songs) maps to
///   `<device>/Music/<same relative path>`; a song is skipped when present there in any format
///   (`Place::under(..).present()`). Sizes come from `sdl_library::converter::output_size`: exact
///   for copies, estimated for conversions (`estimated` is then set). Free space is
///   `device.free_bytes`.
/// - **From device**: `.opus`, `.mp3` and `.m4a` files under the ticked `choice.from_folders` (folders
///   in the device root); a file is skipped when its place ([`place_incoming`]) is present in the
///   library. Free space is not checked (`free_bytes` is `None`).
///
/// Names that start with a dot are never planned.
///
/// # Errors
/// [`DevicesError::NotBuilt`] until built; then [`DevicesError::Io`] or [`DevicesError::Library`].
pub async fn plan(
    music_root: &Path,
    device: &Device,
    choice: &DeviceChoice,
) -> Result<DevicePlan, DevicesError> {
    let _ = (music_root, device, choice);
    std::future::ready(Err(DevicesError::NotBuilt("planning a device sync"))).await
}

/// What a finished (or stopped) device run left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceReport {
    /// The counts.
    pub counts: Counts,
    /// The files this run wrote (not the files it skipped): [`clean_up_and_eject`] removes `._X`
    /// only for an `X` listed here.
    pub written: Vec<PathBuf>,
}

/// Run a plan.
///
/// To device: each file is copied to a `*.part` name and renamed, or converted first with
/// `sdl_library::converter::convert_file` (`converter::parallelism()` at a time) when a format is
/// chosen and the file is not already in it; then each ticked playlist is written to
/// `<device>/Music/<List name>.m3u8` with `sdl_library::playlists::write_for_destination`. From
/// device: files are copied as they are to the place [`place_incoming`] gives. Sends
/// [`sdl_core::EventKind::Counts`] after each file and an `Attention` for a failed one. `stop` is
/// checked between files: the file in hand finishes, then the run ends and the device stays
/// mounted. A write that fails because the device is full or gone ends the run with
/// [`DevicesError::Full`] or [`DevicesError::Gone`]; the files already complete stay. Never
/// deletes a song on either side.
///
/// # Errors
/// [`DevicesError::NotBuilt`] until built; then the errors above.
pub async fn run(
    plan: &DevicePlan,
    events: &EventSender,
    stop: &StopFlag,
) -> Result<DeviceReport, DevicesError> {
    let _ = (
        &plan.device,
        &plan.music_root,
        &plan.choice,
        &plan.jobs,
        events,
        stop,
    );
    std::future::ready(Err(DevicesError::NotBuilt("running a device sync"))).await
}

/// Where a file that comes from a device lands in the library under `music_root` (F10, From
/// device). `file` is under `device.mount_point`. The place comes from the file's tags
/// (`sdl_library::tags::place_from_tags`, F7) with the file's own extension; a file without a
/// title or artist tag goes to `<music_root>/Unsorted/<its path relative to the device root>`,
/// each component made safe like any library name.
///
/// # Errors
/// [`DevicesError::NotBuilt`] until built; then [`DevicesError::Library`].
pub async fn place_incoming(
    music_root: &Path,
    device: &Device,
    file: &Path,
) -> Result<PathBuf, DevicesError> {
    let _ = (music_root, device, file);
    std::future::ready(Err(DevicesError::NotBuilt("placing a file from a device"))).await
}

/// Clean up and eject (the owner has been asked, and was told it empties the device's Trash):
/// remove `.Spotlight-V100`, `.Trashes`, `.fseventsd` and `.TemporaryItems` from the device root
/// and every `.DS_Store`; remove a `._X` file only when `X` is in `written` (this run wrote it)
/// and the file starts with the `AppleDouble` signature (bytes `00 05 16 07`); then
/// `diskutil unmount <mount point>` (`tokio::process::Command`). Removes nothing else.
///
/// # Errors
/// [`DevicesError::NotBuilt`] until built; then [`DevicesError::Io`] or
/// [`DevicesError::Diskutil`].
pub async fn clean_up_and_eject(device: &Device, written: &[PathBuf]) -> Result<(), DevicesError> {
    let _ = (device, written);
    std::future::ready(Err(DevicesError::NotBuilt("cleaning up and ejecting"))).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn the_watch_notices_a_volume_appearing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let mut watch = watch_dir(dir.path())?;
        std::fs::create_dir(dir.path().join("NEWVOLUME"))?;
        let seen = tokio::time::timeout(Duration::from_secs(10), watch.changed()).await?;
        assert_eq!(seen, Some(()));
        Ok(())
    }
}
