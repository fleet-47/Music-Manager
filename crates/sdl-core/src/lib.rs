//! Shared types for `sdl`: the Spotify song, task events, config, state file and path rules.
//! Library crates depend on this crate only, never on each other.

pub mod config;
pub mod event;
pub mod paths;
pub mod state;
pub mod track;

pub use config::{Config, ConfigError};
pub use event::{
    Attention, CardSummary, Counts, ErrorInfo, Event, EventKind, EventReceiver, EventSender,
    ListChoice, SignInState, TaskId, TaskState, channel,
};
pub use state::{LastRun, State, StateError};
pub use track::Track;
