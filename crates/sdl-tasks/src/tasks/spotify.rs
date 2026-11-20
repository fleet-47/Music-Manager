//! The Sync Spotify task (F4): stub. The conversation is documented on [`crate::Runner`].

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::EventKind;

/// The Sync Spotify task.
pub(crate) struct SyncSpotify<'a> {
    context: &'a TaskContext,
}

impl<'a> SyncSpotify<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self { context }
    }
}

impl Step for SyncSpotify<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        self.context
            .events
            .send(EventKind::Headline("Sync Spotify".to_owned()));
        std::future::ready(Err(TaskError::NotBuilt("Sync Spotify"))).await
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let _ = answers;
        std::future::ready(Err(TaskError::NotBuilt("Sync Spotify"))).await
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        std::future::ready(Err(TaskError::NotBuilt("Sync Spotify"))).await
    }
}
