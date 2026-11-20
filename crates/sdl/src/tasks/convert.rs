//! Convert (F10): offer the library's folders, answer the owner's choices with a plan, then convert.

use sdl_core::EventKind;
use sdl_library::converter;
use sdl_library::files::{self, Plan};

use super::TaskError;
use super::form::{Answered, Answers, Step};
use super::progress::{folders_in, written_line};
use super::runner::{Flow, TaskContext};

/// Convert as a step form.
pub(super) struct Convert<'a> {
    context: &'a TaskContext,
    plan: Option<Plan>,
}

impl<'a> Convert<'a> {
    pub(super) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            plan: None,
        }
    }
}

impl Step for Convert<'_> {
    async fn open(&mut self) -> Result<(), TaskError> {
        let events = &self.context.events;
        events.send(EventKind::Headline("Reading your library".to_owned()));
        let folders = folders_in(self.context.config.music_root.clone()).await?;
        events.send(EventKind::Folders(folders));
        Ok(())
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::Convert(choice) = answers else {
            return Ok(Answered::Ignored);
        };
        self.context.remember_convert(choice.clone())?;
        let events = &self.context.events;
        events.send(EventKind::Headline("Checking what to convert".to_owned()));
        self.plan = None;
        let plan = converter::plan(&self.context.config.music_root, &choice).await?;
        events.send(EventKind::Plan(plan.summary.clone()));
        let can_start = plan.summary.can_start();
        self.plan = Some(plan);
        Ok(Answered::Waiting { can_start })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let Some(plan) = self.plan.take() else {
            return Ok(Flow::Cancelled);
        };
        let events = &self.context.events;
        events.send(EventKind::Headline("Converting songs".to_owned()));
        let counts = files::run(&plan, events).await?;
        Ok(Flow::done(written_line(&counts, "converted"), &counts))
    }
}
