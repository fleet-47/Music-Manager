//! Shared types for `sdl`: the Spotify song, task events, config, state file and path rules.
//! Library crates depend on this crate only, never on each other.

pub mod config;
pub mod device;
pub mod event;
pub mod format;
pub mod paths;
pub mod state;
pub mod track;

pub use config::{Config, ConfigError};
pub use device::{ConvertChoice, Device, DeviceChoice, Direction};
pub use event::{
    Attention, AttentionKind, Counts, ErrorInfo, Event, EventKind, EventReceiver, EventSender,
    ListChoice, PlanSummary, SignInState, TaskId, TaskState, channel,
};
pub use format::{Format, FormatChoice};
pub use paths::Target;
pub use state::{LastRun, RunOutcome, State, StateError};
pub use track::Track;
