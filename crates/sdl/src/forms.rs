//! The four functions' step forms (F2, F3, F4, F10), declared on the shared [`Form`]: what each one
//! asks, how a change in one row changes another, and what Enter sends to the task.
//!
//! A [`Dialog`] is a form plus the function's own context. The screen feeds it keys, pastes and
//! task events; it answers with a [`Reply`] and the [`Command`]s to send. Every dialog goes through
//! the same steps: Editing (Enter continues), Checking (the task works out the plan, the rows are
//! locked), Ready (the plan is shown, Enter starts; changing a row goes back to Editing) or Blocked
//! (the plan cannot start, change a row to try again). Download link has no plan: Enter starts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::KeyEvent;
use sdl_core::{
    ConvertChoice, Device, DeviceChoice, Direction, EventKind, Format, FormatChoice, ListChoice,
    PlanSummary, State, TaskId,
};

use crate::control::Command;
use crate::fmt;
use crate::form::{Form, Row, RowId, Step, Summary, Tick};
use crate::keys::{self, Hint};
use crate::tasks::{Answers, Offer, task_name};

const LISTS: RowId = "lists";
const FORMAT: RowId = "format";
const REMEMBER: RowId = "remember";
const LINK: RowId = "link";
const FOLDERS: RowId = "folders";
const DESTINATION: RowId = "destination";
const DEVICE: RowId = "device";
const DIRECTION: RowId = "direction";
const DEVICE_FORMAT: RowId = "device_format";

const NO_DEVICE: &str = "No device found. Plug one in and press r to look again.";
const NO_LIBRARY_FOLDERS: &str = "Your music folder has no folders yet.";
const NO_DEVICE_FOLDERS: &str = "This device has no folders in its Music folder.";

/// Where a dialog is in its steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The owner fills in rows; Enter continues.
    Editing,
    /// The task is working out the plan.
    Checking,
    /// The plan is shown; Enter starts.
    Ready,
    /// The plan cannot start; changing a row goes back to Editing.
    Blocked,
}

/// What the screen does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Keep the dialog open and send these commands.
    Stay(Vec<Command>),
    /// Close the dialog, the function runs: send these commands.
    Start(Vec<Command>),
    /// Close the dialog and tell the waiting task to cancel.
    Cancel,
}

#[derive(Debug, Clone)]
struct Devices {
    found: Vec<Device>,
    remembered: BTreeMap<String, DeviceChoice>,
    /// Folder names to tick when the next folder list arrives.
    wanted: Vec<String>,
    /// Folder lists asked for and not yet answered; only the last answer counts.
    pending: usize,
}

#[derive(Debug, Clone)]
enum Kind {
    Spotify(Vec<ListChoice>),
    Link,
    Convert,
    Device(Devices),
}

/// One function's step form and what it needs to turn the rows into commands.
#[derive(Debug, Clone)]
pub struct Dialog {
    task: TaskId,
    form: Form,
    stage: Stage,
    kind: Kind,
}

fn download_format_row(format: Format) -> Row {
    let labels = Format::DOWNLOAD
        .iter()
        .map(|f| f.download_label().to_owned())
        .collect();
    let at = Format::DOWNLOAD.iter().position(|f| *f == format);
    Row::choice(FORMAT, "Format", labels, at.unwrap_or(0))
}

fn direction_at(direction: Direction) -> usize {
    Direction::ALL
        .iter()
        .position(|d| *d == direction)
        .unwrap_or(0)
}

fn device_format_at(format: Option<Format>) -> usize {
    Format::DEVICE
        .iter()
        .position(|f| *f == format)
        .unwrap_or(0)
}

fn names(form: &Form, id: RowId) -> Vec<String> {
    let ticks = form.ticks(id);
    form.ticked(id)
        .into_iter()
        .filter_map(|at| ticks.get(at).map(|tick| tick.label.clone()))
        .collect()
}

fn folder_ticks(folders: &[String], on: &[String]) -> Vec<Tick> {
    folders
        .iter()
        .map(|name| Tick::new(name.clone(), on.contains(name)))
        .collect()
}

impl Dialog {
    /// Sync Spotify: the lists to tick, the format rows.
    #[must_use]
    pub fn spotify(choices: Vec<ListChoice>, state: &State) -> Self {
        let ticks = choices
            .iter()
            .map(|c| {
                Tick::new(c.name.clone(), state.ticked_lists.contains(&c.id))
                    .detail(fmt::thousands(c.count))
            })
            .collect();
        let format = state
            .formats
            .get(&TaskId::SyncSpotify)
            .copied()
            .unwrap_or_default();
        let rows = vec![
            Row::ticks(LISTS, "Lists", ticks)
                .needs("Tick a list")
                .when_empty("Spotify has no lists for you."),
            download_format_row(format),
            Row::toggle(REMEMBER, "Remember this format", false),
        ];
        Self::build(TaskId::SyncSpotify, rows, Kind::Spotify(choices))
    }

    /// Download link: the link and the format rows. Enter starts the run.
    #[must_use]
    pub fn link(state: &State) -> Self {
        let format = state
            .formats
            .get(&TaskId::DownloadLink)
            .copied()
            .unwrap_or_default();
        let rows = vec![
            Row::text(LINK, "Link", "").needs("Paste a link"),
            download_format_row(format),
            Row::toggle(REMEMBER, "Remember this format", false),
        ];
        Self::build(TaskId::DownloadLink, rows, Kind::Link)
    }

    /// Convert: the library's folders, the target format and the destination.
    #[must_use]
    pub fn convert(folders: &[String], state: &State) -> Self {
        let choice = &state.convert;
        let format_at = Format::CONVERT
            .iter()
            .position(|f| *f == choice.format)
            .unwrap_or(0);
        let rows = vec![
            Row::ticks(FOLDERS, "Folders", folder_ticks(folders, &choice.folders))
                .needs("Tick a folder")
                .when_empty(NO_LIBRARY_FOLDERS),
            Row::choice(
                FORMAT,
                "Format",
                Format::CONVERT
                    .iter()
                    .map(|f| f.label().to_owned())
                    .collect(),
                format_at,
            ),
            Row::text(
                DESTINATION,
                "Destination",
                &choice.destination.to_string_lossy(),
            )
            .needs("Type a destination folder"),
        ];
        Self::build(TaskId::Convert, rows, Kind::Convert)
    }

    /// Sync device: the devices found, direction, folders and the format on the device. The
    /// commands ask for the folders of the first device.
    #[must_use]
    pub fn device(found: Vec<Device>, state: &State) -> (Self, Vec<Command>) {
        let rows = vec![
            Row::choice(DEVICE, "Device", Vec::new(), 0)
                .needs("Plug in a device")
                .when_empty(NO_DEVICE)
                .refreshable(),
            Row::choice(
                DIRECTION,
                "Direction",
                Direction::ALL
                    .iter()
                    .map(|d| d.label().to_owned())
                    .collect(),
                0,
            )
            .hidden(),
            Row::ticks(FOLDERS, "Folders", Vec::new())
                .needs("Tick a folder")
                .when_empty(NO_LIBRARY_FOLDERS)
                .hidden(),
            Row::choice(
                DEVICE_FORMAT,
                "Format on device",
                Format::DEVICE
                    .iter()
                    .map(|f| Format::device_label(*f).to_owned())
                    .collect(),
                0,
            )
            .hidden(),
        ];
        let mut dialog = Self::build(
            TaskId::SyncDevice,
            rows,
            Kind::Device(Devices {
                found: Vec::new(),
                remembered: state.devices.clone(),
                wanted: Vec::new(),
                pending: 0,
            }),
        );
        let commands = dialog.update_devices(found);
        (dialog, commands)
    }

    fn build(task: TaskId, rows: Vec<Row>, kind: Kind) -> Self {
        let mut dialog = Self {
            task,
            form: Form::new(task_name(task), rows),
            stage: Stage::Editing,
            kind,
        };
        dialog.show_base_summary();
        dialog
    }

    // ---- what the screen reads ----

    /// The function this dialog belongs to.
    #[must_use]
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// The form to draw.
    #[must_use]
    pub fn form(&self) -> &Form {
        &self.form
    }

    /// The step the dialog is in.
    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// What the owner picked, for the function's row on the main screen: the link, or the
    /// device's name.
    #[must_use]
    pub fn chosen_label(&self) -> Option<String> {
        match &self.kind {
            Kind::Link => Some(self.form.text(LINK).trim().to_owned()),
            Kind::Device(_) => self.selected_device().map(|device| device.name.clone()),
            Kind::Spotify(_) | Kind::Convert => None,
        }
    }

    /// The keys that work now.
    #[must_use]
    pub fn hints(&self) -> Vec<Hint> {
        match self.stage {
            Stage::Editing => {
                let enter = if matches!(self.kind, Kind::Link) {
                    keys::FORM_START
                } else {
                    keys::FORM_CONTINUE
                };
                self.form.hints(Some(enter))
            }
            Stage::Checking => vec![keys::CANCEL.hint()],
            Stage::Ready => self.form.hints(Some(keys::FORM_START)),
            Stage::Blocked => self.form.hints(None),
        }
    }

    // ---- input ----

    /// A key was pressed.
    pub fn key(&mut self, event: KeyEvent) -> Reply {
        let step = self.form.handle_key(event);
        self.on_step(step)
    }

    /// Text was pasted.
    pub fn paste(&mut self, text: &str) -> Reply {
        let step = self.form.paste(text);
        self.on_step(step)
    }

    fn on_step(&mut self, step: Step) -> Reply {
        match step {
            Step::Idle => Reply::Stay(Vec::new()),
            Step::Cancel => Reply::Cancel,
            Step::Changed(id) => Reply::Stay(self.changed(id)),
            Step::Refresh(_) => Reply::Stay(self.look_again()),
            Step::Submit => self.submit(),
        }
    }

    fn submit(&mut self) -> Reply {
        match self.stage {
            Stage::Checking | Stage::Blocked => Reply::Stay(Vec::new()),
            Stage::Ready => Reply::Start(vec![Command::Go(self.task)]),
            Stage::Editing => match self.check_command() {
                Check::Start(command) => Reply::Start(vec![command]),
                Check::Ask(command, busy) => {
                    self.stage = Stage::Checking;
                    self.form.set_locked(true);
                    self.form.set_summary(Summary::Busy(busy.to_owned()));
                    Reply::Stay(vec![command])
                }
                Check::None => Reply::Stay(Vec::new()),
            },
        }
    }

    fn check_command(&self) -> Check {
        match &self.kind {
            Kind::Link => Check::Start(Command::Answer(Answers::Link {
                url: self.form.text(LINK).trim().to_owned(),
                format: self.format_choice(),
            })),
            Kind::Spotify(choices) => {
                let lists = self
                    .form
                    .ticked(LISTS)
                    .into_iter()
                    .filter_map(|at| choices.get(at).cloned())
                    .collect();
                Check::Ask(
                    Command::Answer(Answers::Spotify {
                        lists,
                        format: self.format_choice(),
                    }),
                    "Listing the songs…",
                )
            }
            Kind::Convert => Check::Ask(
                Command::Answer(Answers::Convert(self.convert_choice())),
                "Checking what to convert…",
            ),
            Kind::Device(_) => match self.selected_device() {
                Some(device) => Check::Ask(
                    Command::Answer(Answers::Device {
                        device: device.clone(),
                        choice: self.device_choice(),
                    }),
                    "Checking what to copy…",
                ),
                None => Check::None,
            },
        }
    }

    fn format_choice(&self) -> FormatChoice {
        FormatChoice {
            format: Format::DOWNLOAD
                .get(self.form.choice(FORMAT))
                .copied()
                .unwrap_or_default(),
            remember: self.form.toggle(REMEMBER),
        }
    }

    fn convert_choice(&self) -> ConvertChoice {
        ConvertChoice {
            folders: names(&self.form, FOLDERS),
            format: Format::CONVERT
                .get(self.form.choice(FORMAT))
                .copied()
                .unwrap_or_default(),
            destination: PathBuf::from(self.form.text(DESTINATION).trim()),
        }
    }

    fn device_choice(&self) -> DeviceChoice {
        DeviceChoice {
            folders: names(&self.form, FOLDERS),
            direction: self.direction(),
            format: Format::DEVICE
                .get(self.form.choice(DEVICE_FORMAT))
                .copied()
                .flatten(),
        }
    }

    fn direction(&self) -> Direction {
        Direction::ALL
            .get(self.form.choice(DIRECTION))
            .copied()
            .unwrap_or_default()
    }

    fn selected_device(&self) -> Option<&Device> {
        match &self.kind {
            Kind::Device(devices) => devices.found.get(self.form.choice(DEVICE)),
            _ => None,
        }
    }

    // ---- reacting to rows ----

    /// A row changed: a plan no longer holds, and the device rows depend on each other.
    fn changed(&mut self, id: RowId) -> Vec<Command> {
        if matches!(self.stage, Stage::Ready | Stage::Blocked) {
            self.stage = Stage::Editing;
        }
        let commands = match id {
            DEVICE => self.select_device(),
            DIRECTION => {
                let on = names(&self.form, FOLDERS);
                if let Kind::Device(devices) = &mut self.kind {
                    devices.wanted = on;
                }
                self.show_direction_rows();
                self.request_folders()
            }
            _ => Vec::new(),
        };
        self.show_base_summary();
        commands
    }

    /// What the form says while the owner is still choosing.
    fn show_base_summary(&mut self) {
        let summary = match &self.kind {
            Kind::Spotify(choices) => {
                let ticked = self.form.ticked(LISTS);
                let songs = ticked
                    .iter()
                    .filter_map(|at| choices.get(*at))
                    .map(|choice| choice.count)
                    .fold(0, usize::saturating_add);
                if ticked.is_empty() {
                    Summary::None
                } else {
                    Summary::Info(format!(
                        "{} ticked · {} songs",
                        fmt::thousands(ticked.len()),
                        fmt::thousands(songs)
                    ))
                }
            }
            Kind::Link | Kind::Convert | Kind::Device(_) => Summary::None,
        };
        self.form.set_summary(summary);
    }

    fn show_direction_rows(&mut self) {
        let to_device = self.direction() == Direction::ToDevice;
        self.form.set_visible(DEVICE_FORMAT, to_device);
    }

    /// Load what was remembered for the chosen device, and ask for its folders.
    fn select_device(&mut self) -> Vec<Command> {
        let Some(device) = self.selected_device().cloned() else {
            return Vec::new();
        };
        let choice = match &mut self.kind {
            Kind::Device(devices) => {
                let choice = devices
                    .remembered
                    .get(device.id())
                    .cloned()
                    .unwrap_or_default();
                devices.wanted.clone_from(&choice.folders);
                choice
            }
            _ => return Vec::new(),
        };
        self.form
            .set_choice(DIRECTION, direction_at(choice.direction));
        self.form
            .set_choice(DEVICE_FORMAT, device_format_at(choice.format));
        self.show_direction_rows();
        self.request_folders()
    }

    fn request_folders(&mut self) -> Vec<Command> {
        let Some(device) = self.selected_device().cloned() else {
            return Vec::new();
        };
        let direction = self.direction();
        let empty = match direction {
            Direction::ToDevice => NO_LIBRARY_FOLDERS,
            Direction::FromDevice => NO_DEVICE_FOLDERS,
        };
        self.form.set_when_empty(FOLDERS, empty);
        self.form.set_loading(FOLDERS, true);
        if let Kind::Device(devices) = &mut self.kind {
            devices.pending = devices.pending.saturating_add(1);
        }
        vec![Command::Offer(Offer::Folders { device, direction })]
    }

    fn look_again(&mut self) -> Vec<Command> {
        self.stage = Stage::Editing;
        self.form
            .set_summary(Summary::Busy("Looking for devices…".to_owned()));
        vec![Command::Offer(Offer::Devices)]
    }

    // ---- what the task reports ----

    /// A task event for this dialog. Returns commands to send.
    pub fn apply(&mut self, event: &EventKind) -> Vec<Command> {
        match event {
            EventKind::NewOfTotal { new, total } if self.stage == Stage::Checking => {
                self.stage = Stage::Ready;
                self.form.set_locked(false);
                self.form.set_summary(Summary::Info(format!(
                    "{} new of {}",
                    fmt::thousands(*new),
                    fmt::thousands(*total)
                )));
                Vec::new()
            }
            EventKind::Plan(plan) if self.stage == Stage::Checking => {
                self.stage = if plan.can_start() {
                    Stage::Ready
                } else {
                    Stage::Blocked
                };
                self.form.set_locked(false);
                let summary = self.plan_summary(plan);
                self.form.set_summary(summary);
                Vec::new()
            }
            EventKind::Headline(doing) if self.stage == Stage::Checking => {
                self.form.set_summary(Summary::Busy(format!("{doing}…")));
                Vec::new()
            }
            EventKind::Folders(folders) => {
                self.set_folders(folders);
                Vec::new()
            }
            EventKind::Devices(found) => self.update_devices(found.clone()),
            _ => Vec::new(),
        }
    }

    fn set_folders(&mut self, folders: &[String]) {
        if let Kind::Device(devices) = &mut self.kind {
            devices.pending = devices.pending.saturating_sub(1);
            if devices.pending > 0 {
                return;
            }
            let ticks = folder_ticks(folders, &devices.wanted);
            self.form.set_ticks(FOLDERS, ticks);
        }
    }

    /// The device list changed (first look, or the owner looked again). The chosen device stays
    /// chosen when it is still there.
    fn update_devices(&mut self, found: Vec<Device>) -> Vec<Command> {
        let before = self.selected_device().map(|device| device.id().to_owned());
        let Kind::Device(devices) = &mut self.kind else {
            return Vec::new();
        };
        let remembered_first = found
            .iter()
            .position(|device| devices.remembered.contains_key(device.id()));
        let kept = before
            .as_deref()
            .and_then(|id| found.iter().position(|device| device.id() == id));
        let at = kept.or(remembered_first).unwrap_or(0);
        let options = found
            .iter()
            .map(|device| format!("{} · {} free", device.name, fmt::bytes(device.free_bytes)))
            .collect();
        let none = found.is_empty();
        devices.found = found;
        self.form.set_options(DEVICE, options, at);
        self.stage = Stage::Editing;
        self.form.set_locked(false);
        self.form.set_visible(DIRECTION, !none);
        self.form.set_visible(FOLDERS, !none);
        self.form.set_visible(DEVICE_FORMAT, !none);
        if none {
            self.form.focus_on(DEVICE);
            self.show_base_summary();
            return Vec::new();
        }
        let after = self.selected_device().map(|device| device.id().to_owned());
        let commands = if before.is_some() && before == after {
            self.show_direction_rows();
            Vec::new()
        } else {
            self.select_device()
        };
        self.show_base_summary();
        commands
    }

    fn plan_summary(&self, plan: &PlanSummary) -> Summary {
        if let Some(reason) = &plan.refusal {
            return Summary::Warn(reason.clone());
        }
        let already = format!("{} already there", fmt::thousands(plan.already));
        if matches!(self.kind, Kind::Convert) {
            return Summary::Info(format!("{} to convert · {already}", fmt::files(plan.files)));
        }
        let device = self
            .selected_device()
            .map_or("the device", |device| device.name.as_str());
        let size = fmt::bytes(plan.bytes);
        match plan.free_bytes {
            Some(free) if !plan.fits() => Summary::Warn(format!(
                "Not enough room on {device}: {} need {size}, {} free. Untick a folder.",
                fmt::files(plan.files),
                fmt::bytes(free)
            )),
            Some(free) => Summary::Info(format!(
                "{}, {size} to copy · {already} · {} free on {device}",
                fmt::files(plan.files),
                fmt::bytes(free)
            )),
            None => Summary::Info(format!(
                "{}, {size} to copy · {already}",
                fmt::files(plan.files)
            )),
        }
    }
}

enum Check {
    /// Download link: nothing to check, start.
    Start(Command),
    /// Ask the task for its plan; the text shows while it works.
    Ask(Command, &'static str),
    None,
}

#[cfg(test)]
mod tests;
