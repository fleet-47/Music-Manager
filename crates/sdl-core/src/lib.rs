//! Shared types for `sdl`: tracks, pipeline stages, progress events, config and path rules.
//! Library crates depend on this crate only, never on each other.

pub mod config;
pub mod event;
pub mod paths;
pub mod stage;
pub mod track;

pub use config::{Config, ConfigError};
pub use event::{Event, EventSender, SignInState, emit};
pub use stage::{Stage, Status};
pub use track::Track;
