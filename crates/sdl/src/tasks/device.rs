//! Sync device (F4): list devices, answer the form's offers (rescan, folders) and answers (plan),
//! then copy. After a copy, cleanup and unmount are `Runner::eject_device`, once the owner
//! confirms.

use sdl_core::paths::device_music_dir;
use sdl_core::{Device, Direction, EventKind, Format};
use sdl_library::devices;
use sdl_library::files::{self, Plan};

use super::TaskError;
use super::form::{Answered, Answers, Offer, Step};
use super::progress::{folders_in, written_line};
use super::runner::{Flow, TaskContext};

/// Sync device as a step form.
pub(super) struct DeviceSync<'a> {
    context: &'a TaskContext,
    /// The plan, with the device, direction and format it was made for.
    planned: Option<Planned>,
}

struct Planned {
    device: Device,
    direction: Direction,
    format: Option<Format>,
    plan: Plan,
}

impl<'a> DeviceSync<'a> {
    pub(super) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            planned: None,
        }
    }

    async fn look_for_devices(&self) -> Result<(), TaskError> {
        let events = &self.context.events;
        events.send(EventKind::Headline("Looking for devices".to_owned()));
        events.send(EventKind::Devices(devices::list().await?));
        Ok(())
    }
}

/// What the run is doing, in words: `Copying to KINGSTON`, `Converting and copying to KINGSTON`,
/// `Copying from KINGSTON`.
fn copy_headline(name: &str, direction: Direction, format: Option<Format>) -> String {
    match (direction, format) {
        (Direction::ToDevice, Some(_)) => format!("Converting and copying to {name}"),
        (Direction::ToDevice, None) => format!("Copying to {name}"),
        (Direction::FromDevice, _) => format!("Copying from {name}"),
    }
}

impl Step for DeviceSync<'_> {
    async fn open(&mut self) -> Result<(), TaskError> {
        self.look_for_devices().await
    }

    async fn offer(&mut self, offer: Offer) -> Result<bool, TaskError> {
        self.planned = None;
        match offer {
            Offer::Devices => self.look_for_devices().await?,
            Offer::Folders { device, direction } => {
                let source = match direction {
                    Direction::ToDevice => self.context.config.music_root.clone(),
                    Direction::FromDevice => device_music_dir(&device.mount_point),
                };
                let folders = folders_in(source).await?;
                self.context.events.send(EventKind::Folders(folders));
            }
        }
        Ok(true)
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::Device { device, choice } = answers else {
            return Ok(Answered::Ignored);
        };
        self.context.remember_device(&device, choice.clone())?;
        let events = &self.context.events;
        events.send(EventKind::Headline(format!(
            "Checking what is on {}",
            device.name
        )));
        self.planned = None;
        let plan = devices::plan(&self.context.config.music_root, &device, &choice).await?;
        events.send(EventKind::Plan(plan.summary.clone()));
        let can_start = plan.summary.can_start();
        self.planned = Some(Planned {
            device,
            direction: choice.direction,
            format: choice.format,
            plan,
        });
        Ok(Answered::Waiting { can_start })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let Some(planned) = self.planned.take() else {
            return Ok(Flow::Cancelled);
        };
        let events = &self.context.events;
        events.send(EventKind::Headline(copy_headline(
            &planned.device.name,
            planned.direction,
            planned.format,
        )));
        self.context.set_device(Some(planned.device));
        let counts = files::run(&planned.plan, events).await?;
        Ok(Flow::done(written_line(&counts, "copied"), &counts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headline_names_the_device_and_the_direction() {
        let to = Direction::ToDevice;
        assert_eq!(copy_headline("KINGSTON", to, None), "Copying to KINGSTON");
        assert_eq!(
            copy_headline("KINGSTON", to, Some(Format::Mp3)),
            "Converting and copying to KINGSTON"
        );
        assert_eq!(
            copy_headline("KINGSTON", Direction::FromDevice, Some(Format::Mp3)),
            "Copying from KINGSTON"
        );
    }
}
