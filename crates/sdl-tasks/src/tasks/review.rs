//! The Review task (F8): stub. The conversation is documented on [`crate::Runner`].

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::EventKind;

/// The Review task.
pub(crate) struct Review<'a> {
    context: &'a TaskContext,
}

impl<'a> Review<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self { context }
    }
}

impl Step for Review<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        self.context
            .events
            .send(EventKind::Headline("Review".to_owned()));
        std::future::ready(Err(TaskError::NotBuilt("Review"))).await
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let _ = answers;
        std::future::ready(Err(TaskError::NotBuilt("Review"))).await
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        std::future::ready(Err(TaskError::NotBuilt("Review"))).await
    }
}
