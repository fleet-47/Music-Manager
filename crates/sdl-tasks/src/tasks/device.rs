//! The Sync device task (F10). The conversation is documented on [`crate::Runner`].
//!
//! The form opens with the devices found and the playlists to tick, and keeps the device list
//! current while it is open (a volume appears or goes). Every answer is remembered per device and
//! planned; the run copies or converts, and stops after the file in hand when asked. The device
//! is handed to the runner for the eject that follows. Every run ends with the device's clean-up.

use std::future::pending;

use sdl_core::{Counts, Device, Direction, EventKind, Library, LibraryId, PlanSummary};
use sdl_devices::{self as devices, DevicePlan, DeviceWatch};

use crate::conversation::{Answered, Answers, Offer, Opened, Step};
use crate::progress::{stopped_line, written_line};
use crate::tasks::{send_libraries, send_playlists};
use crate::{Flow, TaskContext, TaskError};

/// A plan and the device it is for.
struct Planned {
    device: Device,
    plan: DevicePlan,
    /// The library the plan reads from or writes into.
    library: LibraryId,
    direction: Direction,
}

/// The Sync device task.
pub(crate) struct SyncDevice<'a> {
    context: &'a TaskContext,
    devices: Vec<Device>,
    watch: Option<DeviceWatch>,
    planned: Option<Planned>,
}

impl<'a> SyncDevice<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            devices: Vec::new(),
            watch: None,
            planned: None,
        }
    }

    /// Look again, tell the form, and say whether the plan made before is now out of date.
    async fn refresh(&mut self) -> Result<bool, TaskError> {
        let found = devices::list().await?;
        self.context.events.send(EventKind::Devices(found.clone()));
        self.devices = found;
        Ok(self
            .planned
            .as_ref()
            .is_some_and(|planned| gone(&planned.device, &self.devices)))
    }
}

/// Whether `device` is no longer among the `found` devices.
fn gone(device: &Device, found: &[Device]) -> bool {
    !found.iter().any(|other| other.id() == device.id())
}

impl Step for SyncDevice<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        let events = &self.context.events;
        events.send(EventKind::Headline("Looking for devices".to_owned()));
        self.devices = devices::list().await?;
        events.send(EventKind::Devices(self.devices.clone()));
        send_libraries(self.context)?;
        send_playlists(self.context, &Library::mine(&self.context.config))?;
        self.watch = Some(devices::watch()?);
        events.send(EventKind::Headline(
            "Choose a device and what to copy".to_owned(),
        ));
        Ok(Opened::Ready)
    }

    async fn background(&mut self) -> Result<bool, TaskError> {
        let Some(watch) = &mut self.watch else {
            return pending().await;
        };
        if watch.changed().await.is_none() {
            self.watch = None;
            return Ok(false);
        }
        self.refresh().await
    }

    async fn offer(&mut self, offer: Offer) -> Result<bool, TaskError> {
        match offer {
            Offer::Libraries => {
                send_libraries(self.context)?;
                Ok(false)
            }
            Offer::Playlists(id) => {
                send_playlists(self.context, &Library::resolve(&self.context.config, &id))?;
                Ok(false)
            }
            Offer::Devices => self.refresh().await,
            Offer::Folders(device) => {
                let names = devices::folders(&device).await?;
                self.context.events.send(EventKind::Folders(names));
                Ok(false)
            }
        }
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::SyncDevice { device, choice } = answers else {
            return Ok(Answered::Ignored);
        };
        let events = &self.context.events;
        // A drive that is plugged in but not mounted is mounted now, at Continue (F10). Mounting
        // does not plan: the form reads the refreshed list (the same identity, now mounted) and
        // asks again, so From device can show the folders to tick first. A refusal is the plan's
        // refusal, in `diskutil`'s own words, and the form stays open. An answer that still
        // carries the old not-mounted entry after the list was refreshed is the mounted device.
        let device = if device.not_mounted.is_some() {
            let mounted = self
                .devices
                .iter()
                .find(|other| other.id() == device.id() && other.not_mounted.is_none());
            if let Some(mounted) = mounted {
                mounted.clone()
            } else {
                match devices::mount(&device).await {
                    Ok(_) => {
                        self.refresh().await?;
                    }
                    Err(error) => {
                        events.send(EventKind::Plan(PlanSummary {
                            files: 0,
                            bytes: 0,
                            estimated: false,
                            already: 0,
                            free_bytes: None,
                            refusal: Some(error.to_string()),
                        }));
                    }
                }
                return Ok(Answered::Waiting { can_start: false });
            }
        } else {
            device
        };
        self.context.remember_device(&device, choice.clone())?;
        events.send(EventKind::Headline("Counting the files".to_owned()));
        let library = Library::resolve(&self.context.config, &choice.library);
        let plan = devices::plan(&library, &device, &choice).await?;
        let summary = plan.summary().clone();
        events.send(EventKind::Plan(summary.clone()));
        self.planned = Some(Planned {
            device,
            plan,
            library: choice.library.clone(),
            direction: choice.direction,
        });
        Ok(Answered::Waiting {
            can_start: summary.can_start(),
        })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let Some(planned) = self.planned.take() else {
            return Ok(Flow::Cancelled);
        };
        let result = devices::run(&planned.plan, &context.events, &context.stop).await;
        // Read now: a Stop confirmed during the clean-up does not change how the run ended.
        let stopped = context.stop.is_cancelled();
        self.end_run(&planned, result, stopped).await
    }
}

impl SyncDevice<'_> {
    /// How a run ends, whatever way it ended: clean-up while the device is mounted (the files the
    /// run wrote are the ones it may touch; a failure goes on the run's line and leaves the run's
    /// own result alone), then the device for the eject question, then the run's result.
    async fn end_run(
        &self,
        planned: &Planned,
        result: Result<Counts, devices::DevicesError>,
        stopped: bool,
    ) -> Result<Flow, TaskError> {
        let context = self.context;
        let Planned {
            device,
            plan,
            library,
            direction,
        } = planned;
        if let Err(error) = devices::clean_up_device(device, &plan.written()).await {
            context.note_on_line(format!("clean-up failed: {error}"));
        }
        context.set_device(Some(device.clone()));
        if *direction == Direction::ToDevice {
            context.remember_synced_library(device, library.clone())?;
        }
        let counts = result?;
        if stopped {
            return Ok(Flow::Stopped {
                line: stopped_line(&counts),
            });
        }
        Ok(Flow::done(written_line(&counts, "copied"), &counts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use std::num::NonZeroUsize;

    use sdl_core::{
        Config, DeviceChoice, ReviewList, RunOutcome, Selection, State, TaskId, channel,
    };

    use crate::Runner;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn device(name: &str, uuid: Option<&str>) -> Device {
        Device {
            name: name.into(),
            mount_point: format!("/Volumes/{name}").into(),
            volume_uuid: uuid.map(str::to_owned),
            free_bytes: 0,
            disk_image: false,
            not_mounted: None,
        }
    }

    #[test]
    fn a_plan_goes_stale_only_when_its_device_is_gone() {
        let card = device("Card", Some("U1"));
        let renamed = device("Renamed", Some("U1"));
        let other = device("Other", None);
        assert!(!gone(&card, &[other.clone(), renamed]), "same volume UUID");
        assert!(gone(&card, &[other]));
        assert!(gone(&card, &[]));
    }

    fn put(path: &Path, content: &[u8]) -> TestResult {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// What a test run is told to go through.
    struct Run {
        direction: Direction,
        /// Cancel the stop token before the run, as `Runner::stop` does.
        stop: bool,
        /// End the run with an error (the device is full) instead of running it, with the device
        /// still there.
        fail: bool,
    }

    /// Run the task body for a device with macOS leftovers on it, and end the run as the runner
    /// does. Returns the runner (for the last-run line), the device folder and the body's result.
    async fn sync(
        dir: &Path,
        run: Run,
    ) -> Result<(Runner, std::path::PathBuf, bool), Box<dyn std::error::Error>> {
        let config = Config {
            client_id: "id".into(),
            music_root: dir.join("music"),
            my_library: "Me".into(),
            download_jobs: NonZeroUsize::MIN,
            animations: true,
        };
        let library = Library::mine(&config);
        let (tx, _rx) = channel();
        let runner = Runner::with_files(
            config,
            (State::default(), dir.join("state.toml")),
            (ReviewList::default(), dir.join("review.toml")),
            tx,
        );
        let root = dir.join("dev");
        put(&root.join(".Trashes/501/x"), b"x")?;
        put(&root.join("Music/.DS_Store"), b"x")?;
        let song = match run.direction {
            Direction::ToDevice => dir.join("music/Me/A/Al/one.opus"),
            Direction::FromDevice => root.join("Music/one.opus"),
        };
        put(&song, b"audio")?;
        let device = Device {
            name: "TESTDEV".into(),
            mount_point: root.clone(),
            volume_uuid: None,
            free_bytes: u64::MAX,
            disk_image: false,
            not_mounted: None,
        };
        let choice = DeviceChoice {
            direction: run.direction,
            what: Selection {
                whole_library: true,
                playlists: Vec::new(),
            },
            from_folders: vec!["Music".into()],
            ..DeviceChoice::default()
        };
        let plan = devices::plan(&library, &device, &choice).await?;
        let context = runner.context(TaskId::SyncDevice);
        if run.stop {
            context.stop.cancel();
        }
        let mut task = SyncDevice::new(&context);
        let planned = Planned {
            device,
            plan,
            library: choice.library.clone(),
            direction: run.direction,
        };
        let result = if run.fail {
            // Nothing in a test makes a real device full: the run's error is handed to the end
            // of the run as `go` hands it.
            let full = devices::DevicesError::Full {
                device: "TESTDEV".into(),
            };
            task.end_run(&planned, Err(full), false).await
        } else {
            task.planned = Some(planned);
            task.go().await
        };
        let ok = result.is_ok();
        context.conclude(result);
        Ok((runner, root, ok))
    }

    fn line(runner: &Runner) -> Result<(String, RunOutcome), Box<dyn std::error::Error>> {
        let state = runner.state();
        let run = state
            .last_run
            .get(&TaskId::SyncDevice)
            .ok_or("no last-run line")?;
        Ok((run.line.clone(), run.outcome))
    }

    fn cleaned(root: &Path) -> bool {
        !root.join(".Trashes").exists() && !root.join("Music/.DS_Store").exists()
    }

    #[tokio::test]
    async fn clean_up_ends_a_completed_run_in_both_directions() -> TestResult {
        for direction in [Direction::ToDevice, Direction::FromDevice] {
            let dir = tempfile::tempdir()?;
            let (runner, root, ok) = sync(
                dir.path(),
                Run {
                    direction,
                    stop: false,
                    fail: false,
                },
            )
            .await?;
            assert!(ok, "{direction:?}");
            assert!(cleaned(&root), "{direction:?}");
            let (line, outcome) = line(&runner)?;
            assert_eq!(outcome, RunOutcome::Done, "{line}");
            assert!(!line.contains("clean-up"), "{line}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn clean_up_ends_a_stopped_run() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, root, _) = sync(
            dir.path(),
            Run {
                direction: Direction::ToDevice,
                stop: true,
                fail: false,
            },
        )
        .await?;
        assert!(cleaned(&root));
        assert_eq!(line(&runner)?.1, RunOutcome::Stopped);
        Ok(())
    }

    #[tokio::test]
    async fn a_stop_that_comes_during_clean_up_does_not_make_a_finished_run_stopped() -> TestResult
    {
        let dir = tempfile::tempdir()?;
        let (runner, root, _) = sync(
            dir.path(),
            Run {
                direction: Direction::ToDevice,
                stop: false,
                fail: false,
            },
        )
        .await?;
        assert_eq!(line(&runner)?.1, RunOutcome::Done);
        // The same end of a run that finished before the stop, with the stop confirmed after it.
        let context = runner.context(TaskId::SyncDevice);
        let task = SyncDevice::new(&context);
        let device = device("TESTDEV", None);
        let library = Library::mine(&context.config);
        let choice = DeviceChoice::default();
        let plan = devices::plan(
            &library,
            &Device {
                mount_point: root,
                ..device.clone()
            },
            &choice,
        )
        .await?;
        let planned = Planned {
            device,
            plan,
            library: choice.library.clone(),
            direction: Direction::ToDevice,
        };
        context.stop.cancel();
        let flow = task.end_run(&planned, Ok(Counts::default()), false).await?;
        assert!(matches!(flow, Flow::Done { .. }));
        let flow = task.end_run(&planned, Ok(Counts::default()), true).await?;
        assert!(matches!(flow, Flow::Stopped { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn an_answer_for_a_drive_already_mounted_does_not_mount_it_again() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("dev");
        std::fs::create_dir_all(&root)?;
        let (runner, _, _) = sync(
            dir.path(),
            Run {
                direction: Direction::ToDevice,
                stop: false,
                fail: false,
            },
        )
        .await?;
        let context = runner.context(TaskId::SyncDevice);
        let mut task = SyncDevice::new(&context);
        let mounted = Device {
            mount_point: root.clone(),
            free_bytes: u64::MAX,
            ..device("Card", Some("U1"))
        };
        task.devices = vec![mounted];
        // The form still holds the entry it listed before the drive was mounted; the identity is
        // the same. A real mount of this made-up identifier would be refused.
        let listed = Device {
            mount_point: PathBuf::new(),
            not_mounted: Some("disk99s9".into()),
            ..device("Card", Some("U1"))
        };
        let answered = task
            .answer(Answers::SyncDevice {
                device: listed,
                choice: DeviceChoice::default(),
            })
            .await?;
        assert!(matches!(answered, Answered::Waiting { .. }));
        let planned = task.planned.ok_or("no plan was made")?;
        assert_eq!(planned.device.mount_point, root);
        assert!(planned.device.not_mounted.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn clean_up_ends_a_failed_run_too() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, root, _) = sync(
            dir.path(),
            Run {
                direction: Direction::ToDevice,
                stop: false,
                fail: true,
            },
        )
        .await?;
        assert!(cleaned(&root));
        let (line, outcome) = line(&runner)?;
        assert_eq!(outcome, RunOutcome::Failed, "{line}");
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clean_up_failure_goes_on_the_line_and_the_run_keeps_its_result() -> TestResult {
        use std::os::unix::fs::PermissionsExt;

        // The same run, with and without a leftover that cannot be removed.
        let clean = tempfile::tempdir()?;
        let (runner, _, _) = sync(
            clean.path(),
            Run {
                direction: Direction::ToDevice,
                stop: false,
                fail: false,
            },
        )
        .await?;
        let expected = line(&runner)?;

        let dir = tempfile::tempdir()?;
        let locked = dir.path().join("dev/.Trashes/locked");
        put(&locked.join("x"), b"x")?;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))?;
        let outcome = sync(
            dir.path(),
            Run {
                direction: Direction::ToDevice,
                stop: false,
                fail: false,
            },
        )
        .await;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))?;
        let (runner, _, ok) = outcome?;
        assert!(ok);
        let (line, outcome) = line(&runner)?;
        assert_eq!(outcome, expected.1, "the run's own result is unchanged");
        assert!(
            line.starts_with(&expected.0) && line.contains("clean-up failed"),
            "{line}"
        );
        Ok(())
    }
}
