//! Checks against the real machine, skipped in a normal test run (`cargo test -- --ignored`).
//!
//! `sdl_devices_on_a_real_fat32_volume` needs `hdiutil` and the variable `SDL_TEST_IMAGE_DIR`
//! (a folder for the disk image it makes, mounts and removes again); it only ever writes to that
//! image. `conversion_with_the_real_ffmpeg` needs `ffmpeg` on `PATH`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sdl_core::{
    Counts, DeviceChoice, EventKind, EventSender, Format, Quality, Selection, StopFlag, TaskId,
    channel,
};

use crate::testing::{mp3, put};
use crate::{DevicesError, clean_up_and_eject, list, plan, run, watch};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A mounted FAT32 disk image that is detached and deleted when dropped.
struct Image {
    file: PathBuf,
    mount: PathBuf,
    /// The whole-disk node `hdiutil attach` printed, e.g. `/dev/disk4`.
    node: String,
}

impl Image {
    fn new(dir: &Path, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let file = dir.join(format!("{name}.dmg"));
        let made = Command::new("hdiutil")
            .args([
                "create",
                "-size",
                "40m",
                "-fs",
                "MS-DOS FAT32",
                "-volname",
                name,
            ])
            .arg(&file)
            .output()?;
        if !made.status.success() {
            return Err(String::from_utf8_lossy(&made.stderr).into());
        }
        let file = file.with_extension("dmg");
        let attached = Command::new("hdiutil")
            .args(["attach", "-nobrowse"])
            .arg(&file)
            .output()?;
        if !attached.status.success() {
            return Err(String::from_utf8_lossy(&attached.stderr).into());
        }
        let printed = String::from_utf8_lossy(&attached.stdout).into_owned();
        let node = printed
            .split_whitespace()
            .next()
            .ok_or("hdiutil printed no device")?
            .to_owned();
        Ok(Self {
            mount: Path::new("/Volumes").join(name),
            file,
            node,
        })
    }

    /// `diskutil info` says the volume is a disk image this test made.
    fn is_a_disk_image(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let info = Command::new("diskutil")
            .arg("info")
            .arg(&self.mount)
            .output()?;
        let text = String::from_utf8_lossy(&info.stdout);
        Ok(text.contains("Disk Image") && text.contains(&self.mount.display().to_string()))
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // Only this test's own image: detach its disk node (the volume may be unmounted already),
        // then delete its file.
        let _ = Command::new("hdiutil")
            .arg("detach")
            .arg(&self.node)
            .output();
        let _ = std::fs::remove_file(&self.file);
    }
}

#[tokio::test]
#[ignore = "needs hdiutil and SDL_TEST_IMAGE_DIR"]
async fn sdl_devices_on_a_real_fat32_volume() -> TestResult {
    let dir = PathBuf::from(std::env::var("SDL_TEST_IMAGE_DIR")?);
    let name = format!("SDLDEV{}", std::process::id());
    let mut volumes = watch()?;
    let image = Image::new(&dir, &name)?;
    assert!(image.is_a_disk_image()?, "{}", image.mount.display());
    let seen = tokio::time::timeout(Duration::from_secs(10), volumes.changed()).await?;
    assert_eq!(seen, Some(()));

    let devices = list().await?;
    let device = devices
        .iter()
        .find(|device| device.name == name)
        .ok_or("the image is not in the device list")?
        .clone();
    assert!(device.free_bytes > 0 && device.free_bytes < 40 * 1024 * 1024);
    assert!(device.volume_uuid.is_some());

    // Six 8 MB songs do not fit in 40 MB: the summary says so, and starting anyway ends the run
    // in plain words with every finished file whole.
    let library = tempfile::tempdir()?;
    for index in 0..6 {
        put(
            &library.path().join(format!("A/Al/song{index}.opus")),
            &vec![u8::try_from(index)?; 8 * 1024 * 1024],
        )?;
    }
    let choice = DeviceChoice {
        what: Selection {
            whole_library: true,
            playlists: Vec::new(),
        },
        ..DeviceChoice::default()
    };
    let planned = plan(library.path(), &device, &choice).await?;
    assert!(!planned.summary().can_start());
    let mut roomy = device.clone();
    roomy.free_bytes = u64::MAX;
    let forced = plan(library.path(), &roomy, &choice).await?;
    let (tx, mut rx) = channel();
    let events = EventSender::new(TaskId::SyncDevice, tx);
    let error = run(&forced, &events, &StopFlag::new()).await.err();
    assert!(
        matches!(error, Some(DevicesError::Full { .. })),
        "{error:?}"
    );
    let mut last = Counts::default();
    while let Ok(event) = rx.try_recv() {
        if let EventKind::Counts(counts) = event.kind {
            last = counts;
        }
    }
    let on_device: Vec<_> = walkdir::WalkDir::new(&image.mount)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .collect();
    // macOS itself leaves a `._X` beside every file and folder this process makes on FAT32.
    let apple_double = |path: &PathBuf| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("._"))
    };
    let songs: Vec<_> = on_device
        .iter()
        .filter(|path| !apple_double(path))
        .cloned()
        .collect();
    assert_eq!(songs.len(), last.added, "{on_device:?}");
    let mut wrote = forced.written();
    wrote.retain(|path| path.is_file());
    wrote.sort();
    let mut seen = songs.clone();
    seen.sort();
    assert_eq!(wrote, seen, "the plan keeps what the failed run wrote");
    assert!(on_device.iter().any(apple_double), "macOS made no ._ file");
    for file in &songs {
        assert_eq!(
            std::fs::metadata(file)?.len(),
            8 * 1024 * 1024,
            "{}",
            file.display()
        );
    }

    // Eject: macOS items and the DS_Store go, the songs stay, and the volume unmounts.
    put(&image.mount.join(".Trashes/501/x"), b"x")?;
    put(&image.mount.join(".fseventsd/x"), b"x")?;
    put(&image.mount.join("Music/.DS_Store"), b"x")?;
    crate::eject::clean_up(&image.mount, &forced.written())?;
    assert!(!image.mount.join(".Trashes").exists());
    assert!(!image.mount.join("Music/.DS_Store").exists());
    for song in &songs {
        assert!(song.exists());
        let beside = song.with_file_name(format!(
            "._{}",
            song.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        assert!(!beside.exists(), "{} survived", beside.display());
    }
    clean_up_and_eject(&device, &forced.written()).await?;
    assert!(!image.mount.exists(), "still mounted");
    Ok(())
}

#[tokio::test]
#[ignore = "needs ffmpeg on PATH"]
async fn conversion_with_the_real_ffmpeg() -> TestResult {
    let library = tempfile::tempdir()?;
    let dev = tempfile::tempdir()?;
    put(
        &library.path().join("Band/Rec/Song.mp3"),
        &mp3(Some("Song"), Some("Band"), Some("Rec")),
    )?;
    let device = sdl_core::Device {
        name: "TESTDEV".into(),
        mount_point: dev.path().to_path_buf(),
        volume_uuid: None,
        free_bytes: u64::MAX,
        disk_image: false,
    };
    let choice = DeviceChoice {
        what: Selection {
            whole_library: true,
            playlists: Vec::new(),
        },
        format: Some(Format::M4a),
        quality: Quality::Small,
        ..DeviceChoice::default()
    };
    let planned = plan(library.path(), &device, &choice).await;
    let planned = planned?;
    assert!(planned.summary().estimated);
    assert_eq!(
        planned.jobs[0].1,
        dev.path().join("Music/Band/Rec/Song.m4a")
    );
    let (tx, _rx) = channel();
    let events = EventSender::new(TaskId::SyncDevice, tx);
    let report = run(&planned, &events, &StopFlag::new()).await?;
    assert_eq!((report.added, report.failed), (1, 0), "{report:?}");
    assert!(std::fs::metadata(dev.path().join("Music/Band/Rec/Song.m4a"))?.len() > 0);
    Ok(())
}
