//! The Convert task (F9). The conversation is documented on [`crate::Runner`].
//!
//! The form opens with the playlists to tick. Every answer is remembered, then planned; the plan's
//! summary is the form's summary line, and a refused destination comes back as a plan the owner
//! cannot start. The run converts, then writes the ticked playlists, and stops after the file in
//! hand when asked.

use sdl_core::EventKind;
use sdl_library::convert::{ConvertPlan, plan, run};
use sdl_library::playlists;

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::fmt::thousands;
use crate::progress::{on_blocking_pool, stopped_line, written_line};
use crate::{Flow, TaskContext, TaskError};

/// The Convert task.
pub(crate) struct Convert<'a> {
    context: &'a TaskContext,
    plan: Option<ConvertPlan>,
}

impl<'a> Convert<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            plan: None,
        }
    }
}

impl Step for Convert<'_> {
    fn open(&mut self) -> impl Future<Output = Result<Opened, TaskError>> + Send {
        let events = &self.context.events;
        events.send(EventKind::Headline("Looking at your library".to_owned()));
        let opened = playlists::list(&self.context.config.music_root)
            .map(|lists| {
                events.send(EventKind::Playlists(lists));
                events.send(EventKind::Headline("Choose what to convert".to_owned()));
                Opened::Ready
            })
            .map_err(TaskError::from);
        std::future::ready(opened)
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::Convert(choice) = answers else {
            return Ok(Answered::Ignored);
        };
        let events = &self.context.events;
        self.context.remember_convert(choice.clone())?;
        events.send(EventKind::Headline("Counting the files".to_owned()));
        let planned = plan(&self.context.config.music_root, &choice).await?;
        let summary = planned.summary().clone();
        events.send(EventKind::Plan(summary.clone()));
        self.plan = Some(planned);
        Ok(Answered::Waiting {
            can_start: summary.can_start(),
        })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let Some(planned) = self.plan.take() else {
            return Ok(Flow::Cancelled);
        };
        let files = planned.summary().files;
        context.events.send(EventKind::Headline(format!(
            "Converting {} {}",
            thousands(files),
            if files == 1 { "file" } else { "files" }
        )));
        let (events, stop) = (context.events.clone(), context.stop.clone());
        let counts =
            on_blocking_pool(move || async move { run(&planned, &events, &stop).await }).await??;
        if context.stop.is_requested() {
            return Ok(Flow::Stopped {
                line: stopped_line(&counts),
            });
        }
        Ok(Flow::done(written_line(&counts, "converted"), &counts))
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{ConvertChoice, Event, EventReceiver, PlanSummary, RunOutcome, TaskId};

    use super::*;
    use crate::Command;
    use crate::tasks::intake::tests::runner;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn next_plan(rx: &mut EventReceiver) -> Result<PlanSummary, &'static str> {
        loop {
            let Event { kind, .. } = rx.recv().await.ok_or("the events ended early")?;
            match kind {
                EventKind::Plan(summary) => return Ok(summary),
                EventKind::Error(_) | EventKind::Stopped | EventKind::Finished => {
                    return Err("ended before a plan");
                }
                _ => {}
            }
        }
    }

    fn answer(choice: &ConvertChoice) -> Command {
        Command::Answer(Answers::Convert(choice.clone()))
    }

    #[tokio::test]
    async fn answers_are_remembered_a_refused_plan_cannot_start_and_a_good_one_runs() -> TestResult
    {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.start(TaskId::Convert)?;
        let inside = ConvertChoice {
            destination: dir.path().join("music/converted"),
            ..ConvertChoice::default()
        };
        assert!(runner.send(TaskId::Convert, answer(&inside)));
        let refused = next_plan(&mut rx).await?;
        assert!(refused.refusal.is_some() && !refused.can_start());
        assert_eq!(
            runner.state().convert,
            inside,
            "remembered even when refused"
        );
        assert!(
            runner.send(TaskId::Convert, Command::Go),
            "ignored: cannot start"
        );

        let good = ConvertChoice {
            destination: dir.path().join("converted"),
            ..ConvertChoice::default()
        };
        assert!(runner.send(TaskId::Convert, answer(&good)));
        let plan = next_plan(&mut rx).await?;
        assert!(plan.can_start());
        assert!(runner.send(TaskId::Convert, Command::Go));
        let mut kinds = Vec::new();
        loop {
            let event = rx.recv().await.ok_or("no end")?;
            let end = event.kind == EventKind::Finished;
            kinds.push(event.kind);
            if end {
                break;
            }
        }
        assert!(kinds.contains(&EventKind::Started));
        let run = runner.state().last_run.get(&TaskId::Convert).cloned();
        assert_eq!(
            run.map(|r| (r.line, r.outcome)),
            Some(("0 converted".to_owned(), RunOutcome::Done))
        );
        assert_eq!(runner.state().convert, good);
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_at_the_form_leaves_the_last_run_alone() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.start(TaskId::Convert)?;
        assert!(runner.send(TaskId::Convert, Command::Cancel));
        loop {
            let event = rx.recv().await.ok_or("no end")?;
            if event.kind == EventKind::Stopped {
                break;
            }
        }
        assert!(runner.state().last_run.is_empty());
        Ok(())
    }
}
