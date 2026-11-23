//! Planning a device sync (F10): what would be written, how big it is, whether it fits, and where
//! a file that comes from a device lands in the library.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use sdl_core::format::is_audio_file;
use sdl_core::place::{component, device_music_dir};
use sdl_core::{Device, DeviceChoice, Direction, Format, Library, PlanSummary};
use sdl_library::converter::output_size;
use sdl_library::selection;
use sdl_library::{LibraryError, tags};
use walkdir::WalkDir;

use crate::DevicesError;
use crate::paths::{existing_in_any_format, stays_inside};

/// Folder of the library that holds files whose tags do not say where they belong.
const UNSORTED: &str = "Unsorted";

/// What a device sync would do, and the summary shown before Start.
#[derive(Debug, Clone)]
pub struct DevicePlan {
    pub(crate) summary: PlanSummary,
    pub(crate) device: Device,
    pub(crate) library: Library,
    pub(crate) choice: DeviceChoice,
    /// `(source, destination)` file pairs, copy or convert.
    pub(crate) jobs: Vec<(PathBuf, PathBuf)>,
    /// The files the last run of this plan wrote so far; clones of the plan share it.
    pub(crate) written: Arc<Mutex<Vec<PathBuf>>>,
}

impl DevicePlan {
    /// The summary line's numbers: `N files, about X GB to copy · M already there · Y GB free on
    /// <device>`. When there is not enough room, `can_start()` is false (not a refusal reason of
    /// its own: the form says so in plain words).
    #[must_use]
    pub const fn summary(&self) -> &PlanSummary {
        &self.summary
    }

    /// The files the last [`crate::run`] of this plan wrote and the folders it made, up to now,
    /// also when the run ended with an error (a full or vanished device) or a stop: what
    /// [`crate::clean_up_device`] needs to remove the `._` files macOS made beside them.
    #[must_use]
    pub fn written(&self) -> Vec<PathBuf> {
        self.written
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Plan a sync for `choice.direction`, reading from (To device) or writing into (From device)
/// `library`, which the caller resolved from `choice.library` (F12). Songs on the device have no
/// library level: `<device>/Music/<artist>/<album>/<file>`.
///
/// - **To device**: each song of the chosen selection (`choice.what`, listed by
///   `sdl_library::selection::songs`: the whole library, then each ticked playlist's songs that
///   exist) maps to
///   `<device>/Music/<same relative path>`, with the preset's extension when a format is chosen
///   and the file is not already in it; a song is skipped when present there in any format.
///   Sizes come from `sdl_library::converter::output_size`: exact for copies, estimated for
///   conversions (`estimated` is then set). Free space is `device.free_bytes`.
/// - **From device**: `.opus`, `.mp3` and `.m4a` files under the ticked `choice.from_folders` (folders
///   in the device root); a file is skipped when its place ([`incoming_path`]) is present in the
///   library in any format. Free space is not checked (`free_bytes` is `None`).
///
/// Names that start with a dot are never planned. Reads tags and durations, so the scan runs on a
/// blocking thread.
///
/// # Errors
/// [`DevicesError::Io`] when a folder or file cannot be read. A file whose tags or duration
/// cannot be read never fails the plan (From device: it goes to `Unsorted`; To device: its size
/// is estimated from the file itself, `sdl_library::converter::output_size`).
pub async fn plan(
    library: &Library,
    device: &Device,
    choice: &DeviceChoice,
) -> Result<DevicePlan, DevicesError> {
    let (library, device, choice) = (library.clone(), device.clone(), choice.clone());
    tokio::task::spawn_blocking(move || build(library, device, choice))
        .await
        .map_err(|error| joined(&error))?
}

fn joined(error: &tokio::task::JoinError) -> DevicesError {
    DevicesError::Library(LibraryError::Task(error.to_string()))
}

fn build(
    library: Library,
    device: Device,
    choice: DeviceChoice,
) -> Result<DevicePlan, DevicesError> {
    let (jobs, already, bytes, estimated) = match choice.direction {
        Direction::ToDevice => plan_to_device(&library, &device, &choice)?,
        Direction::FromDevice => plan_from_device(&library, &device, &choice)?,
    };
    let free_bytes = match choice.direction {
        Direction::ToDevice => Some(device.free_bytes),
        Direction::FromDevice => None,
    };
    // Nothing to write: no file, and (To device) no playlist to write either.
    let nothing = jobs.is_empty()
        && (choice.direction == Direction::FromDevice || choice.what.playlists.is_empty());
    let refusal = nothing.then(|| nothing_to_copy(&device, choice.direction, already));
    Ok(DevicePlan {
        summary: PlanSummary {
            files: jobs.len(),
            bytes,
            estimated,
            already,
            free_bytes,
            refusal,
        },
        device,
        library,
        choice,
        jobs,
        written: Arc::default(),
    })
}

/// Why a plan with nothing to write cannot start, in plain words.
fn nothing_to_copy(device: &Device, direction: Direction, already: usize) -> String {
    let name = &device.name;
    match (direction, already > 0) {
        (Direction::ToDevice, true) => {
            format!("Nothing to copy: every chosen song is already on {name}.")
        }
        (Direction::ToDevice, false) => {
            "Nothing to copy: what is ticked holds no songs. Tick something else.".to_owned()
        }
        (Direction::FromDevice, true) => format!(
            "Nothing to copy: every song in the ticked folders of {name} is already in the library."
        ),
        (Direction::FromDevice, false) => format!(
            "Nothing to copy: the ticked folders of {name} hold no .opus, .mp3 or .m4a files."
        ),
    }
}

type Planned = (Vec<(PathBuf, PathBuf)>, usize, u64, bool);

fn plan_to_device(
    library: &Library,
    device: &Device,
    choice: &DeviceChoice,
) -> Result<Planned, DevicesError> {
    let preset = choice.preset();
    let dest_root = device_music_dir(&device.mount_point);
    let (mut jobs, mut already, mut bytes, mut estimated) = (Vec::new(), 0_usize, 0_u64, false);
    for song in selection::songs(library, &choice.what)? {
        let (src, relative) = (song.file, song.relative);
        let converts = preset.is_some_and(|preset| !preset.format.holds(&src));
        let relative = match preset {
            Some(preset) if converts => relative.with_extension(preset.format.extension()),
            _ => relative,
        };
        let dest = dest_root.join(relative);
        if existing_in_any_format(&dest)?.is_some() {
            already = already.saturating_add(1);
            continue;
        }
        let size = output_size(&src, preset)?;
        bytes = bytes.saturating_add(size.bytes);
        estimated |= size.estimated;
        jobs.push((src, dest));
    }
    Ok((jobs, already, bytes, estimated))
}

fn plan_from_device(
    library: &Library,
    device: &Device,
    choice: &DeviceChoice,
) -> Result<Planned, DevicesError> {
    let (mut jobs, mut already, mut bytes) = (Vec::new(), 0_usize, 0_u64);
    for folder in &choice.from_folders {
        if !stays_inside(Path::new(folder)) {
            continue;
        }
        let root = device.mount_point.join(folder);
        let walk = WalkDir::new(&root)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
        for entry in walk {
            let entry = entry.map_err(|error| {
                let path = error.path().unwrap_or(&root).to_path_buf();
                DevicesError::Io {
                    path,
                    source: error.into(),
                }
            })?;
            if !entry.file_type().is_file() || !is_audio_file(entry.path()) {
                continue;
            }
            let dest = incoming_path(library, &device.mount_point, entry.path())?;
            if existing_in_any_format(&dest)?.is_some() {
                already = already.saturating_add(1);
                continue;
            }
            let size = entry.metadata().map_err(|error| DevicesError::Io {
                path: entry.path().to_path_buf(),
                source: error.into(),
            })?;
            bytes = bytes.saturating_add(size.len());
            jobs.push((entry.into_path(), dest));
        }
    }
    Ok((jobs, already, bytes, false))
}

/// Where a file that comes from a device lands in `library` (F10, F12, From device). `file` is under `device.mount_point`. The place comes from the file's tags
/// (`sdl_library::tags::place_from_tags`, F7) with the file's own extension; a file without a
/// title or artist tag, or whose tags cannot be read, goes to
/// `<library folder>/Unsorted/<its path relative to the device root>`, each component made safe like
/// any library name. A file that is not `.opus`, `.mp3` or `.m4a` is [`DevicesError::Io`].
fn incoming_path(library: &Library, mount: &Path, file: &Path) -> Result<PathBuf, DevicesError> {
    let Some(format) = Format::of(file) else {
        return Err(not_audio(file));
    };
    // A file whose tags cannot be read has no title or artist tag to go by: it goes to
    // `Unsorted` like an untagged file, so one odd file never blocks the plan.
    match tags::place_from_tags(file, library) {
        Ok(Some(place)) => Ok(place.path(format)),
        Ok(None) | Err(LibraryError::Tag { .. }) => unsorted_path(library, mount, file),
        Err(other) => Err(other.into()),
    }
}

fn not_audio(file: &Path) -> DevicesError {
    DevicesError::Io {
        path: file.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not an .opus, .mp3 or .m4a file",
        ),
    }
}

/// `<library folder>/Unsorted/<path of file relative to the device root>`, every component made safe;
/// a component that is empty once made safe becomes `_`.
fn unsorted_path(library: &Library, mount: &Path, file: &Path) -> Result<PathBuf, DevicesError> {
    let (Some(format), Ok(relative)) = (Format::of(file), file.strip_prefix(mount)) else {
        return Err(not_audio(file));
    };
    let mut dest = library.root().join(UNSORTED);
    if let Some(folders) = relative.parent() {
        for part in folders.components() {
            dest.push(safe_part(&part.as_os_str().to_string_lossy()));
        }
    }
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.push(format!("{}.{}", safe_part(&stem), format.extension()));
    Ok(dest)
}

fn safe_part(name: &str) -> String {
    let safe = component(name);
    if safe.is_empty() {
        "_".to_owned()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{Format, Quality, Selection};

    use super::*;
    use crate::testing::lib;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn touch(path: &Path, bytes: usize) -> TestResult {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, vec![0_u8; bytes])?;
        Ok(())
    }

    fn device(mount: &Path, free_bytes: u64) -> Device {
        Device {
            name: "TESTDEV".into(),
            mount_point: mount.to_path_buf(),
            volume_uuid: None,
            free_bytes,
            disk_image: false,
            not_mounted: None,
        }
    }

    fn whole_library() -> DeviceChoice {
        DeviceChoice {
            what: Selection {
                whole_library: true,
                playlists: Vec::new(),
            },
            ..DeviceChoice::default()
        }
    }

    #[tokio::test]
    async fn the_whole_library_maps_to_the_music_folder_and_counts_what_is_there() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), 100)?;
        touch(&root.path().join("A/Al/two.opus"), 200)?;
        touch(&root.path().join("A/Al/Mr. Three.opus"), 300)?;
        touch(&root.path().join("A/Al/._junk.opus"), 5)?;
        touch(&root.path().join("A/.hidden/x.opus"), 5)?;
        // two is already on the device, in another format.
        touch(&dev.path().join("Music/A/Al/two.mp3"), 1)?;
        let plan = plan(
            &lib(root.path()),
            &device(dev.path(), 10_000),
            &whole_library(),
        )
        .await?;
        let summary = plan.summary();
        assert_eq!((summary.files, summary.bytes, summary.already), (2, 400, 1));
        assert!(!summary.estimated);
        assert_eq!(summary.free_bytes, Some(10_000));
        assert!(summary.can_start());
        let dests: Vec<_> = plan.jobs.iter().map(|(_, dest)| dest.clone()).collect();
        assert_eq!(
            dests,
            [
                dev.path().join("Music/A/Al/Mr. Three.opus"),
                dev.path().join("Music/A/Al/one.opus"),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_device_gets_one_librarys_songs_with_no_library_level() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("Me/A/Al/mine.opus"), 10)?;
        touch(&root.path().join("Sam/B/Bl/theirs.opus"), 20)?;
        let sam = Library::at("Sam", &root.path().join("Sam"), false);
        let planned = plan(&sam, &device(dev.path(), 10_000), &whole_library()).await?;
        let dests: Vec<_> = planned.jobs.iter().map(|(_, dest)| dest.clone()).collect();
        assert_eq!(dests, [dev.path().join("Music/B/Bl/theirs.opus")]);
        Ok(())
    }

    #[tokio::test]
    async fn not_enough_room_cannot_start() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), 100)?;
        let tight = plan(&lib(root.path()), &device(dev.path(), 99), &whole_library()).await?;
        assert!(!tight.summary().fits());
        assert!(!tight.summary().can_start());
        let exact = plan(
            &lib(root.path()),
            &device(dev.path(), 100),
            &whole_library(),
        )
        .await?;
        assert!(exact.summary().can_start());
        Ok(())
    }

    #[tokio::test]
    async fn a_playlist_resolves_to_the_songs_that_exist_in_any_format() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.opus"), 10)?;
        touch(&root.path().join("A/Al/two.m4a"), 20)?;
        touch(&root.path().join("A/Al/other.opus"), 40)?;
        std::fs::write(
            root.path().join("Mix.m3u8"),
            "#EXTM3U\n#PLAYLIST:Mix\nA/Al/one.opus\nA/Al/two.opus\nA/Al/gone.opus\n/etc/passwd.mp3\n../x.opus\n.\
             hidden/y.opus\n",
        )?;
        let choice = DeviceChoice {
            what: Selection {
                whole_library: false,
                playlists: vec!["Mix".into(), "Missing".into()],
            },
            ..DeviceChoice::default()
        };
        let plan = plan(&lib(root.path()), &device(dev.path(), 1000), &choice).await?;
        assert_eq!((plan.summary().files, plan.summary().bytes), (2, 30));
        Ok(())
    }

    #[tokio::test]
    async fn a_file_already_in_the_chosen_format_is_an_exact_copy() -> TestResult {
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        touch(&root.path().join("A/Al/one.mp3"), 77)?;
        let choice = DeviceChoice {
            format: Some(Format::Mp3),
            quality: Quality::Small,
            ..whole_library()
        };
        let plan = plan(&lib(root.path()), &device(dev.path(), 1000), &choice).await?;
        assert_eq!(
            (plan.summary().bytes, plan.summary().estimated),
            (77, false)
        );
        assert_eq!(plan.jobs[0].1, dev.path().join("Music/A/Al/one.mp3"));
        Ok(())
    }

    #[test]
    fn an_untagged_file_keeps_its_device_path_under_unsorted_made_safe() -> TestResult {
        let root = Path::new("/lib");
        let mount = Path::new("/Volumes/D");
        let dest = unsorted_path(
            &lib(root),
            mount,
            &mount.join("Old Rips/CD 1/Mr. Song: live?.MP3"),
        )?;
        assert_eq!(
            dest,
            PathBuf::from("/lib/Unsorted/Old Rips/CD 1/Mr. Song live.mp3")
        );
        let odd = unsorted_path(&lib(root), mount, &mount.join("...x/?.m4a"))?;
        assert_eq!(odd, PathBuf::from("/lib/Unsorted/x/_.m4a"));
        assert!(unsorted_path(&lib(root), mount, &mount.join("a.flac")).is_err());
        assert!(unsorted_path(&lib(root), mount, Path::new("/elsewhere/a.mp3")).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn from_device_plans_tagged_and_untagged_files_under_the_ticked_folders_only()
    -> TestResult {
        use crate::testing::{mp3, put};
        let root = tempfile::tempdir()?;
        let dev = tempfile::tempdir()?;
        let tagged = mp3(Some("Song One"), Some("Band"), Some("Rec"));
        put(&dev.path().join("Old/tagged.mp3"), &tagged)?;
        put(
            &dev.path().join("Old/sub/untagged.mp3"),
            &mp3(None, None, None),
        )?;
        put(
            &dev.path().join("Old/present.mp3"),
            &mp3(Some("Song Two"), Some("Band"), Some("Rec")),
        )?;
        put(&dev.path().join("Old/.hidden.mp3"), &tagged)?;
        put(&dev.path().join("Old/.dir/x.mp3"), &tagged)?;
        put(&dev.path().join("Old/note.txt"), b"x")?;
        put(&dev.path().join("Old/broken.m4a"), b"not audio at all")?;
        put(&dev.path().join("Other/x.mp3"), &tagged)?;
        put(&root.path().join("Band/Rec/Song Two.opus"), b"already")?;
        let choice = DeviceChoice {
            direction: Direction::FromDevice,
            from_folders: vec!["Old".into()],
            ..DeviceChoice::default()
        };
        let plan = plan(&lib(root.path()), &device(dev.path(), 0), &choice).await?;
        let summary = plan.summary();
        assert_eq!((summary.files, summary.already), (3, 1));
        assert_eq!(summary.free_bytes, None);
        assert!(summary.can_start());
        let dests: Vec<_> = plan.jobs.iter().map(|(_, dest)| dest.clone()).collect();
        assert_eq!(
            dests,
            [
                root.path().join("Unsorted/Old/broken.m4a"),
                root.path().join("Unsorted/Old/sub/untagged.mp3"),
                root.path().join("Band/Rec/Song One.mp3"),
            ]
        );
        assert_eq!(
            incoming_path(
                &lib(root.path()),
                dev.path(),
                &dev.path().join("Old/tagged.mp3")
            )?,
            root.path().join("Band/Rec/Song One.mp3")
        );
        Ok(())
    }
}
