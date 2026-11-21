//! The five functions' task bodies, one module each.

pub(crate) mod convert;
pub(crate) mod device;
pub(crate) mod intake;
pub(crate) mod link;
pub(crate) mod review;
pub(crate) mod spotify;

use sdl_core::{EventKind, Library};

use crate::{TaskContext, TaskError};

/// Tell the form which libraries are on disk (`Libraries`): when it opens, and on
/// `Offer::Libraries`.
pub(crate) fn send_libraries(context: &TaskContext) -> Result<(), TaskError> {
    let libraries = Library::list(&context.config)?;
    context.events.send(EventKind::Libraries(libraries));
    Ok(())
}

/// Tell the form the playlists of `library` (`Playlists`).
pub(crate) fn send_playlists(context: &TaskContext, library: &Library) -> Result<(), TaskError> {
    let lists = sdl_library::playlists::list(library)?;
    context.events.send(EventKind::Playlists(lists));
    Ok(())
}
