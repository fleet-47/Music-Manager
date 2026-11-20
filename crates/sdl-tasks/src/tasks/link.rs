//! The Download link task (F5): stub. The conversation is documented on [`crate::Runner`].

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::EventKind;

/// The Download link task.
pub(crate) struct DownloadLink<'a> {
    context: &'a TaskContext,
}

impl<'a> DownloadLink<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self { context }
    }
}

impl Step for DownloadLink<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        self.context
            .events
            .send(EventKind::Headline("Download link".to_owned()));
        std::future::ready(Err(TaskError::NotBuilt("Download link"))).await
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let _ = answers;
        std::future::ready(Err(TaskError::NotBuilt("Download link"))).await
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        std::future::ready(Err(TaskError::NotBuilt("Download link"))).await
    }
}
