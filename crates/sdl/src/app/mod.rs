//! The app state and its messages, free of drawing code. The screen draws an [`App`]; the dialogs
//! (the five functions' forms) are held in its popups; [`App::update`] is the one place a
//! [`Msg`] changes anything.
//!
//! Ownership of the files here: `view`, `rows`, `wake`, `popup`, `msg` and this file are the
//! contract (changing them is a contract change); `update.rs` belongs to the screen.

pub mod msg;
pub mod popup;
pub mod rows;
pub mod update;
pub mod view;
pub mod wake;

use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::Size;
use sdl_core::{ReviewList, State, TaskId};
use sdl_form::HitMap;

pub use msg::{Cmd, Msg};
pub use popup::{Confirm, Popup, Waiting};
pub use view::{FunctionView, Phase};

/// Fewer columns than this: the cards stack in one column instead of the grid (F1).
pub const GRID_MIN_COLS: u16 = 72;

/// Rows the card grid needs: two cards of seven rows.
pub const GRID_ROWS: u16 = 14;

/// Rows the stacked cards need: five cards of four rows.
pub const STACK_ROWS: u16 = 20;

/// How the home screen is laid out for the terminal's size (F1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// Cards in three columns: bring music in, check, take music out.
    Grid,
    /// Cards in one column.
    Stack,
    /// One line per function: the terminal is too short for cards.
    List,
}

/// The time, read once per message by the loop so the app itself never reads a clock.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    /// Monotonic, for elapsed time and the time estimate.
    pub instant: Instant,
    /// Seconds since the Unix epoch, for "5 min ago".
    pub epoch: u64,
}

/// What the runner keeps, as the app last saw it.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// `state.toml`: remembered choices, the last-run lines.
    pub state: State,
    /// `review.toml`: the held songs.
    pub review: ReviewList,
    /// The library folder (`music_root`), for notes that name it.
    pub music_root: PathBuf,
}

/// What a click on the main screen means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainTarget {
    /// A function's card (or its row on a short screen): a click selects it; a second click
    /// runs it.
    Row(TaskId),
}

/// What a click in a plain popup (confirmation, error, help, attention list) means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupTarget {
    /// Yes, Continue.
    Yes,
    /// No, Cancel.
    No,
    /// Close.
    Close,
    /// An item of the attention list.
    Item(usize),
    /// The popup's scrollbar.
    Scrollbar,
}

/// The rectangles the last frame drew. Written by the screen after each draw, read by
/// `App::update` for mouse events; everything else treats it as read-only.
#[derive(Debug, Clone, Default)]
pub struct Hits {
    /// Main screen: function rows and running tasks.
    pub main: HitMap<MainTarget>,
    /// The open dialog's form (`sdl_form::render`'s map).
    pub form: HitMap<sdl_form::Target>,
    /// A plain popup.
    pub popup: HitMap<PopupTarget>,
    /// The footer's hints, each with the key a click on it presses.
    pub footer: HitMap<crossterm::event::KeyCode>,
    /// Where each open popup's frame was drawn, bottom first, so a popup keeps its place while it
    /// is open even when its content gets shorter.
    pub anchors: Vec<Anchor>,
}

/// Where a popup's frame has its top left corner, and which popup that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The popup's title: another popup in the same place in the stack starts centred.
    pub title: String,
    /// The frame's top left corner.
    pub at: ratatui::layout::Position,
}

/// The whole app state.
#[derive(Debug)]
pub struct App {
    /// What the runner keeps.
    pub settings: Settings,
    /// One view per function, in [`TaskId::ALL`] order.
    pub views: Vec<FunctionView>,
    /// The function selected on the main screen.
    pub selected: TaskId,
    /// Open popups; the last is on top.
    pub popups: Vec<Popup>,
    /// The terminal size at the last draw or resize.
    pub size: Size,
    /// Whether effects may play (config `animations`, and `NO_COLOR` unset).
    pub animations: bool,
    /// Whether a `tachyonfx` effect is playing; written by the screen after each draw, read by
    /// [`wake::next_wake`].
    pub animating: bool,
    /// The time at the last message.
    pub clock: Clock,
    /// What the last frame drew.
    pub hits: Hits,
    /// Functions whose run ended since the screen last looked, so it flashes their rows once; the
    /// screen takes them ([`App::take_ended`]).
    pub ended: Vec<TaskId>,
    /// Spinner step, advanced on each [`Msg::Wake`]; every spinner on screen shows this frame.
    pub spin: usize,
    /// The devices Sync device last offered, so the eject question can name the one that was
    /// synced (`state.last_device`) after the run.
    pub devices: Vec<sdl_core::Device>,
    /// Set when the app should leave the loop.
    pub quit: bool,
    /// Functions whose run ended since the app started. A download's or a conversion's result
    /// is worth showing only while it is fresh, so their cards show it for these alone.
    pub fresh: Vec<TaskId>,
    /// The tools the functions need that were not found on `PATH` at start (yt-dlp, ffmpeg,
    /// deno). The home screen says so and offers to install them.
    pub missing_tools: Vec<String>,
    /// What the home screen says about the tools instead: installing, or how it ended.
    pub tools_note: Option<String>,
}

impl App {
    /// A new app with nothing open, the first function selected.
    #[must_use]
    pub fn new(settings: Settings, animations: bool, size: Size, clock: Clock) -> Self {
        Self {
            settings,
            views: TaskId::ALL.into_iter().map(FunctionView::new).collect(),
            selected: TaskId::SyncSpotify,
            popups: Vec::new(),
            size,
            animations,
            animating: false,
            clock,
            hits: Hits::default(),
            ended: Vec::new(),
            spin: 0,
            devices: Vec::new(),
            quit: false,
            fresh: Vec::new(),
            missing_tools: Vec::new(),
            tools_note: None,
        }
    }

    /// The view of `task`.
    #[must_use]
    pub fn view(&self, task: TaskId) -> Option<&FunctionView> {
        self.views.iter().find(|view| view.task == task)
    }

    /// The view of `task`, to change.
    pub fn view_mut(&mut self, task: TaskId) -> Option<&mut FunctionView> {
        self.views.iter_mut().find(|view| view.task == task)
    }

    /// The functions whose run has started and not ended, in panel order.
    pub fn running(&self) -> impl Iterator<Item = &FunctionView> {
        self.views.iter().filter(|view| view.is_running())
    }

    /// Whether some function runs (its card shows progress).
    #[must_use]
    pub fn shows_running_panel(&self) -> bool {
        self.running().next().is_some()
    }

    /// Whether cards are on screen, so a running function shows its spinner, bar and time
    /// estimate; the one-line list shows counts only.
    #[must_use]
    pub fn shows_detail(&self) -> bool {
        self.home() != Home::List
    }

    /// The home screen's layout for the terminal's size: the grid when it fits, else the stack,
    /// else one line per function. The footer and the tools line take their rows first.
    #[must_use]
    pub fn home(&self) -> Home {
        let rows = self
            .size
            .height
            .saturating_sub(1)
            .saturating_sub(u16::from(self.tools_banner().is_some()));
        if self.size.width >= GRID_MIN_COLS && rows >= GRID_ROWS {
            Home::Grid
        } else if rows >= STACK_ROWS {
            Home::Stack
        } else {
            Home::List
        }
    }

    /// The line the home screen shows about missing tools, when there is something to say.
    #[must_use]
    pub fn tools_banner(&self) -> Option<String> {
        if let Some(note) = &self.tools_note {
            return Some(note.clone());
        }
        if self.missing_tools.is_empty() {
            return None;
        }
        Some(format!(
            "{} not found on PATH. Press i to install with mise.",
            self.missing_tools.join(", ")
        ))
    }

    /// The popup on top.
    #[must_use]
    pub fn top(&self) -> Option<&Popup> {
        self.popups.last()
    }

    /// Take the functions whose run ended since the last call.
    pub fn take_ended(&mut self) -> Vec<TaskId> {
        std::mem::take(&mut self.ended)
    }
}
