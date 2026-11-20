//! The Sync device task (F10): stub. The conversation is documented on [`crate::Runner`].

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::EventKind;

/// The Sync device task.
pub(crate) struct SyncDevice<'a> {
    context: &'a TaskContext,
}

impl<'a> SyncDevice<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self { context }
    }
}

impl Step for SyncDevice<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        self.context
            .events
            .send(EventKind::Headline("Sync device".to_owned()));
        std::future::ready(Err(TaskError::NotBuilt("Sync device"))).await
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let _ = answers;
        std::future::ready(Err(TaskError::NotBuilt("Sync device"))).await
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        std::future::ready(Err(TaskError::NotBuilt("Sync device"))).await
    }
}
