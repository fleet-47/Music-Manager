//! The Convert task (F9): stub. The conversation is documented on [`crate::Runner`].

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::EventKind;

/// The Convert task.
pub(crate) struct Convert<'a> {
    context: &'a TaskContext,
}

impl<'a> Convert<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self { context }
    }
}

impl Step for Convert<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        self.context
            .events
            .send(EventKind::Headline("Convert".to_owned()));
        std::future::ready(Err(TaskError::NotBuilt("Convert"))).await
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let _ = answers;
        std::future::ready(Err(TaskError::NotBuilt("Convert"))).await
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        std::future::ready(Err(TaskError::NotBuilt("Convert"))).await
    }
}
