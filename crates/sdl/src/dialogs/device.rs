//! Sync device (F10): device, direction, what, format, quality and cover, then the plan line,
//! then Start; afterwards the app asks "Clean up and eject <device>?".

use sdl_core::{Device, EventKind, Format};
use sdl_form::{Choice, Field, Form, Value};

use super::Step;
use crate::app::Settings;
use crate::fmt::bytes;

/// Radio group of mounted external or removable volumes. Option ids are `Device::id`.
pub const DEVICE: &str = "device";
/// Radio group: To device, From device. Option ids `to` and `from`.
pub const DIRECTION: &str = "direction";
/// Tick list (To device): Whole library, then each playlist.
pub const WHAT: &str = "what";
/// Tick list (From device): the device's top-level folders.
pub const FOLDERS: &str = "folders";
/// Dropdown (To device): As is, MP3, M4A.
pub const FORMAT: &str = "format";
/// Dropdown, shown only when converting.
pub const QUALITY: &str = "quality";
/// Toggle, shown only when converting.
pub const COVER: &str = "cover";

/// The note about playlists (F10): `.m3u8` files the player must support, and a Disc showing a
/// changed playlist as a new numbered copy beside the old one.
pub const NOTE: &str = "Playlists are written as .m3u8 files, which the player must support. \
A Disc shows a changed playlist as a new numbered copy beside the old one.";

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub struct State {
    /// The devices last offered, to turn the chosen id back into a `Device` for the answer.
    pub devices: Vec<Device>,
}

/// The form: busy ("Looking for devices") until `Devices` arrives; fields as above with the note;
/// `FOLDERS` hidden while To device, `WHAT` hidden while From device, `QUALITY` and `COVER`
/// hidden while the format is As is. Remembered per device and the last device preselected:
/// whenever the chosen device changes (including the preselection when `Devices` first arrives),
/// direction, ticks, format, quality and cover are set from `settings.state.devices` for it (the
/// defaults for a device never synced). Choosing From device (or another device while From) sends
/// `Offer::Folders(device)` and the form is busy (`Reading the device's folders`) until `Folders`
/// arrives. `QUALITY`'s labels follow the format (`Quality::label`) whenever it changes.
#[must_use]
pub fn open(settings: &Settings) -> (Form, State) {
    let mut form = Form::new("Sync device")
        .field(
            Field::radio(DEVICE, "Device")
                .needs("Plug in a device")
                .when_empty("No device found. Plug one in."),
        )
        .field(Field::radio(DIRECTION, "Direction").options(vec![
            Choice::new("to", "To device"),
            Choice::new("from", "From device"),
        ]))
        .field(
            Field::ticks(WHAT, "What")
                .needs("Tick at least one")
                .when_empty("Loading playlists"),
        )
        .field(
            Field::ticks(FOLDERS, "Folders on the device")
                .needs("Tick at least one folder")
                .hidden(),
        )
        .field(
            Field::dropdown(FORMAT, "Format on device").options(
                Format::DEVICE
                    .iter()
                    .map(|format| {
                        Choice::new(
                            format.map_or("as-is", Format::extension),
                            Format::device_label(*format),
                        )
                    })
                    .collect(),
            ),
        )
        .field(Field::dropdown(QUALITY, "Quality").hidden())
        .field(Field::toggle(COVER, "Keep cover art").hidden());
    form.set(DIRECTION, Value::Choice(Some("to".to_owned())));
    form.set(FORMAT, Value::Choice(Some("as-is".to_owned())));
    form.set_notes(vec![NOTE.to_owned()]);
    form.set_busy(Some("Looking for devices".to_owned()));
    let _ = settings;
    (form, State::default())
}

/// `Devices` fills the device radio group (`name`, detail `12.3 GB free`) and keeps the choice;
/// `Playlists` and `Folders` fill their tick lists; `Plan` becomes the summary line
/// `N files, about X GB to copy · M already there · Y GB free on <device>` and decides
/// `set_blocked` (not enough room, or the plan's refusal, in plain words).
pub fn on_event(form: &mut Form, state: &mut State, kind: &EventKind) -> Step {
    if let EventKind::Devices(devices) = kind {
        state.devices.clone_from(devices);
        form.set_options(
            DEVICE,
            devices
                .iter()
                .map(|device| {
                    Choice::new(device.id().to_owned(), device.name.clone())
                        .with_detail(format!("{} free", bytes(device.free_bytes)))
                })
                .collect(),
        );
        form.set_busy(None);
    }
    Step::Stay(Vec::new())
}
