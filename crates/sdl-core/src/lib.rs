//! Shared types for `sdl`: the wanted song, match outcome, library layout and playlist model,
//! libraries, review list, presets, task events, config and state files.
//! Library crates depend on this crate only, never on each other (`sdl-devices` also on
//! `sdl-library`).

pub mod config;
pub mod device;
pub mod event;
pub mod format;
pub mod landing;
pub mod library;
pub mod matching;
pub mod place;
pub mod playlist;
pub mod preset;
pub mod process;
pub mod review;
pub mod run;
pub mod song;
pub mod state;
mod storage;
pub mod words;

pub use config::{Config, ConfigError};
pub use device::{ConvertChoice, Device, DeviceChoice, Direction, Selection};
pub use event::{
    Attention, AttentionKind, Counts, ErrorInfo, Event, EventKind, EventReceiver, EventSender,
    LIKED_SONGS_ID, LIKED_SONGS_NAME, ListChoice, PlanSummary, SignInState, TaskId, TaskState,
    channel,
};
pub use format::Format;
pub use library::{Library, LibraryId, NewLibraryError};
pub use matching::{Candidate, MatchOutcome};
pub use place::{Place, library_files};
pub use playlist::Playlist;
pub use preset::{Preset, Quality};
pub use review::{HeldSong, ReviewList};
pub use song::{SongSource, WantedSong};
pub use state::{LastRun, Never, RunOutcome, State};
pub use storage::StoreError;
