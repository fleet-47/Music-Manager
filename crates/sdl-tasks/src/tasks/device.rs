//! The Sync device task (F10). The conversation is documented on [`crate::Runner`].
//!
//! The form opens with the devices found and the playlists to tick, and keeps the device list
//! current while it is open (a volume appears or goes). Every answer is remembered per device and
//! planned; the run copies or converts, and stops after the file in hand when asked. The device
//! and the files the run wrote are handed to the runner for the eject that follows.

use std::future::pending;

use sdl_core::{Device, EventKind};
use sdl_devices::{self as devices, DevicePlan, DeviceWatch};
use sdl_library::playlists;

use crate::conversation::{Answered, Answers, Offer, Opened, Step};
use crate::progress::{on_blocking_pool, stopped_line, written_line};
use crate::{Flow, TaskContext, TaskError};

/// A plan and the device it is for.
struct Planned {
    device: Device,
    plan: DevicePlan,
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
        events.send(EventKind::Playlists(playlists::list(
            &self.context.config.music_root,
        )?));
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
        self.context.remember_device(&device, choice.clone())?;
        events.send(EventKind::Headline("Counting the files".to_owned()));
        let plan = devices::plan(&self.context.config.music_root, &device, &choice).await?;
        let summary = plan.summary().clone();
        events.send(EventKind::Plan(summary.clone()));
        self.planned = Some(Planned { device, plan });
        Ok(Answered::Waiting {
            can_start: summary.can_start(),
        })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let Some(Planned { device, plan }) = self.planned.take() else {
            return Ok(Flow::Cancelled);
        };
        let (events, stop) = (context.events.clone(), context.stop.clone());
        let (result, written) = on_blocking_pool(move || async move {
            let result = devices::run(&plan, &events, &stop).await;
            (result, plan.written())
        })
        .await?;
        // Whatever way the run ended, the files it wrote are the ones clean-up may touch.
        context.set_device(Some((device, written)));
        let counts = result?;
        if context.stop.is_requested() {
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

    fn device(name: &str, uuid: Option<&str>) -> Device {
        Device {
            name: name.into(),
            mount_point: format!("/Volumes/{name}").into(),
            volume_uuid: uuid.map(str::to_owned),
            free_bytes: 0,
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
}
