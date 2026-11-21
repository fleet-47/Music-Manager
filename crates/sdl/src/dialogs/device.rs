//! Sync device (F10): device, direction, what, format, quality and cover, then the plan line,
//! then Start; afterwards the app asks "Clean up and eject <device>?".
//!
//! The form follows what is chosen: To device shows `What` and `Format on device`, From device
//! shows the device's folders; `Quality` and `Cover` show only while converting. Choices are
//! remembered per device, so choosing another device sets them from what that device last used.

use sdl_core::{Device, DeviceChoice, Direction, EventKind, Format};
use sdl_form::{Choice, Field, Form, Value};
use sdl_tasks::{Answers, Offer};

use super::choices::{
    chosen_of, flag_of, quality_of, quality_options, selection_of, ticked_of, what_options,
    what_value,
};
use super::conversation::Conversation;
use sdl_core::TaskId;
use sdl_tasks::Command;

use crate::app::{Cmd, Settings};
use crate::fmt::bytes;
use sdl_core::words::{files, thousands};

/// Radio group of mounted external or removable volumes. Option ids are `Device::id`.
pub(super) const DEVICE: &str = "device";
/// Radio group: To device, From device. Option ids `to` and `from`.
pub(super) const DIRECTION: &str = "direction";
/// Tick list (To device): Whole library, then each playlist.
pub(super) const WHAT: &str = "what";
/// Tick list (From device): the device's top-level folders.
pub(super) const FOLDERS: &str = "folders";
/// Dropdown (To device): As is, MP3, M4A.
pub(super) const FORMAT: &str = "format";
/// Dropdown, shown only when converting.
pub(super) const QUALITY: &str = "quality";
/// Toggle, shown only when converting.
pub(super) const COVER: &str = "cover";

/// The option id of "As is" in the format dropdown.
const AS_IS: &str = "as-is";

/// The note about playlists (F10): `.m3u8` files the player must support, and a Disc showing a
/// changed playlist as a new numbered copy beside the old one.
pub(super) const NOTE: &str = "Playlists are written as .m3u8 files, which the player must \
support. A Disc shows a changed playlist as a new numbered copy beside the old one.";

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The devices last offered, to turn the chosen id back into a `Device` for the answer.
    devices: Vec<Device>,
    /// The device whose remembered choices the form shows now.
    device: Option<String>,
    /// The device whose folders were asked for (or arrived) since the devices last changed.
    folders_for: Option<String>,
    /// Whether the devices and the playlists have arrived once; the form waits for both.
    devices_in: bool,
    playlists_in: bool,
}

/// The form: the last device's choices filled in, waiting for the devices.
pub(super) fn open(settings: &Settings) -> (Form, State, Conversation) {
    let mut form = Form::new("Sync device")
        .field(
            Field::radio(DEVICE, "Device")
                .needs("Choose a device")
                .when_empty("No device found. Plug one in."),
        )
        .field(Field::radio(DIRECTION, "Direction").options(vec![
            Choice::new("to", Direction::ToDevice.label()),
            Choice::new("from", Direction::FromDevice.label()),
        ]))
        .field(
            Field::ticks(WHAT, "What")
                .needs("Tick at least one")
                .options(what_options(&[])),
        )
        .field(
            Field::ticks(FOLDERS, "Folders on the device")
                .needs("Tick at least one folder")
                .when_empty("This device has no folders")
                .hidden(),
        )
        .field(
            Field::dropdown(FORMAT, "Format on device").options(
                Format::DEVICE
                    .iter()
                    .map(|format| {
                        Choice::new(
                            format.map_or(AS_IS, Format::extension),
                            Format::device_label(*format),
                        )
                    })
                    .collect(),
            ),
        )
        .field(Field::dropdown(QUALITY, "Quality").options(quality_options(Format::Mp3)))
        .field(Field::toggle(COVER, "Keep cover art"));
    form.set_notes(vec![NOTE.to_owned()]);
    let last = settings.state.last_device.clone();
    let choice = last
        .as_ref()
        .and_then(|id| settings.state.devices.get(id))
        .cloned()
        .unwrap_or_default();
    form.set(DEVICE, Value::Choice(last.clone()));
    fill(&mut form, &choice);
    let state = State {
        device: last,
        ..State::default()
    };
    let conv = Conversation::planned("Making the plan", Some("Looking for devices"));
    (form, state, conv)
}

/// Put a device's remembered choices in the form and show what they need.
fn fill(form: &mut Form, choice: &DeviceChoice) {
    let to = choice.direction == Direction::ToDevice;
    form.set(
        DIRECTION,
        Value::Choice(Some((if to { "to" } else { "from" }).to_owned())),
    );
    form.set(WHAT, what_value(&choice.what));
    form.set(FOLDERS, Value::Ticked(choice.from_folders.clone()));
    form.set(
        FORMAT,
        Value::Choice(Some(
            choice.format.map_or(AS_IS, Format::extension).to_owned(),
        )),
    );
    form.set_options(
        QUALITY,
        quality_options(choice.format.unwrap_or(Format::Mp3)),
    );
    form.set(
        QUALITY,
        Value::Choice(Some(choice.quality.name().to_owned())),
    );
    form.set(COVER, Value::Flag(choice.keep_cover));
    show_what_is_needed(form);
}

fn from_device(form: &Form) -> bool {
    chosen_of(form, DIRECTION).as_deref() == Some("from")
}

fn format_of(form: &Form) -> Option<Format> {
    let id = chosen_of(form, FORMAT);
    Format::CONVERT
        .into_iter()
        .find(|format| Some(format.extension()) == id.as_deref())
}

/// Show `What` and the format for To device, the folders for From device, and the quality and
/// cover only while converting.
fn show_what_is_needed(form: &mut Form) {
    let from = from_device(form);
    let converting = !from && format_of(form).is_some();
    form.set_visible(WHAT, !from);
    form.set_visible(FOLDERS, from);
    form.set_visible(FORMAT, !from);
    form.set_visible(QUALITY, converting);
    form.set_visible(COVER, converting);
}

/// The device chosen now, if it is one of those offered.
fn chosen_device<'a>(form: &Form, state: &'a State) -> Option<&'a Device> {
    let id = chosen_of(form, DEVICE)?;
    state.devices.iter().find(|device| device.id() == id)
}

/// Task events. `Devices` fills the radio group, keeps the choice, and when another device is now
/// chosen sets the form from what that device remembered; `Folders` and `Playlists` fill their
/// tick lists; `Plan` is the summary line and says why a plan cannot start. Returns the commands
/// to send (the device's folders, when From device needs them).
pub(super) fn on_event(
    form: &mut Form,
    state: &mut State,
    conv: &mut Conversation,
    kind: &EventKind,
    settings: &Settings,
) -> Vec<Cmd> {
    match kind {
        EventKind::Devices(devices) => {
            state.devices.clone_from(devices);
            form.set_options(
                DEVICE,
                devices
                    .iter()
                    .map(|device| {
                        Choice::new(device.id().to_owned(), device.label())
                            .with_detail(format!("{} free", bytes(device.free_bytes)))
                    })
                    .collect(),
            );
            // Nothing chosen: the last device synced is chosen when it is there (F10).
            if chosen_device(form, state).is_none()
                && let Some(last) = &settings.state.last_device
                && devices.iter().any(|device| device.id() == last)
            {
                form.set(DEVICE, Value::Choice(Some(last.clone())));
            }
            let chosen = chosen_device(form, state).map(|device| device.id().to_owned());
            if chosen != state.device {
                if let Some(id) = &chosen {
                    let choice = settings.state.devices.get(id).cloned().unwrap_or_default();
                    fill(form, &choice);
                }
                state.device = chosen;
            }
            state.folders_for = None;
            state.devices_in = true;
            if state.playlists_in {
                conv.wait_for(None);
            }
            conv.changed();
            form.set_blocked(None);
        }
        EventKind::Playlists(names) => {
            form.set_options(WHAT, what_options(names));
            state.playlists_in = true;
            if state.devices_in {
                conv.wait_for(None);
            }
            conv.changed();
        }
        EventKind::Folders(names) => {
            form.set_options(
                FOLDERS,
                names
                    .iter()
                    .map(|name| Choice::new(name.clone(), name.clone()))
                    .collect(),
            );
            conv.wait_for(None);
            conv.changed();
        }
        EventKind::Plan(plan) => {
            let device =
                chosen_device(form, state).map_or_else(|| "the device".to_owned(), Device::label);
            let mut summary = format!(
                "{}, {}{} to copy · {} already there",
                files(plan.files),
                if plan.estimated { "about " } else { "" },
                bytes(plan.bytes),
                thousands(plan.already)
            );
            if let Some(free) = plan.free_bytes {
                summary = format!("{summary} · {} free on {device}", bytes(free));
            }
            let blocked = plan.refusal.clone().or_else(|| {
                let free = plan.free_bytes.filter(|_| !plan.fits())?;
                Some(format!(
                    "Not enough room on {device}: {} to copy, {} free",
                    bytes(plan.bytes),
                    bytes(free)
                ))
            });
            if conv.plan(summary) {
                form.set_blocked(blocked);
            }
        }
        EventKind::Headline(text) => conv.headline(text),
        _ => {}
    }
    settle(form, state, conv)
}

/// A field changed: the choices for another device, the visible fields and the quality labels
/// follow it. Returns the commands to send.
pub(super) fn changed(
    form: &mut Form,
    state: &mut State,
    conv: &mut Conversation,
    field: &str,
    settings: &Settings,
) -> Vec<Cmd> {
    match field {
        DEVICE => {
            let chosen = chosen_device(form, state).map(|device| device.id().to_owned());
            if chosen != state.device {
                if let Some(id) = &chosen {
                    let choice = settings.state.devices.get(id).cloned().unwrap_or_default();
                    fill(form, &choice);
                }
                state.device = chosen;
            }
        }
        FORMAT => {
            let format = format_of(form).unwrap_or(Format::Mp3);
            form.set_options(QUALITY, quality_options(format));
        }
        _ => {}
    }
    settle(form, state, conv)
}

/// Show what the choices need, and ask the device for its folders when From device needs them.
fn settle(form: &mut Form, state: &mut State, conv: &mut Conversation) -> Vec<Cmd> {
    show_what_is_needed(form);
    if !from_device(form) {
        return Vec::new();
    }
    let Some(device) = chosen_device(form, state).cloned() else {
        return Vec::new();
    };
    if state.folders_for.as_deref() == Some(device.id()) {
        return Vec::new();
    }
    state.folders_for = Some(device.id().to_owned());
    conv.wait_for(Some("Reading the device's folders"));
    conv.changed();
    vec![Cmd::Send(
        TaskId::SyncDevice,
        Command::Offer(Offer::Folders(device)),
    )]
}

/// The device and its choices; `None` while no device is chosen.
pub(super) fn answers(form: &Form, state: &State) -> Option<Answers> {
    let device = chosen_device(form, state)?.clone();
    let format = format_of(form);
    let choice = DeviceChoice {
        direction: if from_device(form) {
            Direction::FromDevice
        } else {
            Direction::ToDevice
        },
        what: selection_of(form, WHAT),
        from_folders: ticked_of(form, FOLDERS),
        format,
        quality: quality_of(form, QUALITY),
        keep_cover: flag_of(form, COVER),
    };
    Some(Answers::SyncDevice { device, choice })
}

/// `r`: look for devices again; the form waits for the answer.
pub(super) fn look_again(state: &mut State, conv: &mut Conversation) -> Vec<Cmd> {
    state.folders_for = None;
    conv.wait_for(Some("Looking for devices"));
    conv.changed();
    vec![Cmd::Send(
        TaskId::SyncDevice,
        Command::Offer(Offer::Devices),
    )]
}
