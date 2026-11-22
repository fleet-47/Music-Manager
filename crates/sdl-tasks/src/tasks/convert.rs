//! The Convert task (F9). The conversation is documented on [`crate::Runner`].
//!
//! The form opens with the playlists to tick. Every answer is remembered, then planned; the plan's
//! summary is the form's summary line, and a refused destination comes back as a plan the owner
//! cannot start. The run converts, then writes the ticked playlists, and stops after the file in
//! hand when asked.

use sdl_core::{EventKind, Library};
use sdl_library::convert::{ConvertPlan, plan, run};

use crate::conversation::{Answered, Answers, Offer, Opened, Step};
use crate::progress::{stopped_line, written_line};
use crate::tasks::{send_libraries, send_playlists};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::words::files;

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
        let opened = send_libraries(self.context)
            .and_then(|()| send_playlists(self.context, &Library::mine(&self.context.config)))
            .map(|()| {
                events.send(EventKind::Headline("Choose what to convert".to_owned()));
                Opened::Ready
            });
        std::future::ready(opened)
    }

    fn offer(&mut self, offer: Offer) -> impl Future<Output = Result<bool, TaskError>> + Send {
        let sent = match offer {
            Offer::Libraries => send_libraries(self.context),
            Offer::Playlists(id) => {
                send_playlists(self.context, &Library::resolve(&self.context.config, &id))
            }
            Offer::Devices | Offer::Folders(_) => Ok(()),
        };
        std::future::ready(sent.map(|()| false))
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::Convert(choice) = answers else {
            return Ok(Answered::Ignored);
        };
        let events = &self.context.events;
        self.context.remember_convert(choice.clone())?;
        events.send(EventKind::Headline("Counting the files".to_owned()));
        let library = Library::resolve(&self.context.config, &choice.library);
        let planned = plan(&library, &choice).await?;
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
        context.events.send(EventKind::Headline(format!(
            "Converting {}",
            files(planned.summary().files)
        )));
        let counts = run(&planned, &context.events, &context.stop).await?;
        if context.stop.is_cancelled() {
            return Ok(Flow::Stopped {
                line: stopped_line(&counts),
            });
        }
        Ok(Flow::done(written_line(&counts, "converted"), &counts))
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{
        ConvertChoice, Event, EventReceiver, LibraryId, PlanSummary, RunOutcome, TaskId,
    };

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
            what: sdl_core::Selection {
                whole_library: true,
                playlists: Vec::new(),
            },
            ..ConvertChoice::default()
        };
        assert!(runner.send(TaskId::Convert, answer(&good)));
        let empty = next_plan(&mut rx).await?;
        assert!(
            empty
                .refusal
                .as_deref()
                .is_some_and(|why| why.starts_with("Nothing to convert")),
            "an empty library has nothing to do"
        );
        // A file that is not audio: the run goes through to its end with one failed item.
        let song = dir.path().join("music/Me/Band/Record/Song.opus");
        std::fs::create_dir_all(song.parent().ok_or("no folder")?)?;
        std::fs::write(&song, b"not audio")?;
        assert!(runner.send(TaskId::Convert, answer(&good)));
        let plan = next_plan(&mut rx).await?;
        assert!(plan.can_start() && plan.files == 1);
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
            Some((
                "0 converted, 1 needs attention".to_owned(),
                RunOutcome::NeedsAttention
            ))
        );
        assert_eq!(runner.state().convert, good);
        Ok(())
    }

    #[tokio::test]
    async fn the_form_offers_the_libraries_and_the_playlists_of_the_one_chosen() -> TestResult {
        let dir = tempfile::tempdir()?;
        let sam = dir.path().join("music/Sam");
        std::fs::create_dir_all(&sam)?;
        std::fs::write(sam.join("Road trip.m3u8"), b"#EXTM3U\n")?;
        let (mut runner, mut rx) = runner(dir.path());
        runner.start(TaskId::Convert)?;
        assert!(runner.send(
            TaskId::Convert,
            Command::Offer(Offer::Playlists(LibraryId::Other("Sam".into())))
        ));
        let mut libraries = None;
        let mut playlists = Vec::new();
        while playlists.len() < 2 {
            match rx.recv().await.ok_or("the events ended early")?.kind {
                EventKind::Libraries(found) => libraries = Some(found),
                EventKind::Playlists(found) => playlists.push(found),
                _ => {}
            }
        }
        let names: Vec<String> = libraries
            .ok_or("no libraries")?
            .iter()
            .map(|library| library.name().to_owned())
            .collect();
        assert_eq!(names, ["Me", "Sam"]);
        assert_eq!(
            playlists,
            [Vec::<String>::new(), vec!["Road trip".to_owned()]]
        );
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
