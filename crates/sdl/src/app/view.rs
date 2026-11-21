//! What the UI keeps per function, folded from task events (F11): whether it runs, its counts, its
//! headline, its attention items, and when its run started.

use std::time::Instant;

use sdl_core::{EventKind, TaskId, TaskState};

/// Where a function is in its life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    /// Not running. Its result is the last-run line.
    #[default]
    Idle,
    /// Started and waiting at its form (nothing runs yet): the running panel does not show it.
    AtForm,
    /// The run has started (`Started`): the running panel shows it.
    Running,
}

/// One function's view state.
#[derive(Debug, Clone)]
pub struct FunctionView {
    /// The function.
    pub task: TaskId,
    /// Where it is.
    pub phase: Phase,
    /// Counts, headline and attention items of the current run: the same fold the runner keeps
    /// ([`TaskState::apply`]).
    pub state: TaskState,
    /// When `Started` arrived; the time estimate is measured from here.
    pub started: Option<Instant>,
}

impl FunctionView {
    /// An idle view.
    #[must_use]
    pub fn new(task: TaskId) -> Self {
        Self {
            task,
            phase: Phase::Idle,
            state: TaskState::default(),
            started: None,
        }
    }

    /// The owner opened the function: a new run, at its form, with nothing kept from the one
    /// before (that one is the saved last run).
    pub fn begin(&mut self) {
        *self = Self::new(self.task);
        self.phase = Phase::AtForm;
    }

    /// Fold one event of this function. `true` when the event ended the run (`Finished`,
    /// `Stopped`, `Error`, `RateLimited`): the screen flashes the row and reads the new last-run
    /// line from the runner.
    pub fn apply(&mut self, kind: &EventKind, now: Instant) -> bool {
        self.state.apply(kind);
        match kind {
            EventKind::Started => {
                self.phase = Phase::Running;
                self.started = Some(now);
                false
            }
            EventKind::Finished
            | EventKind::Stopped
            | EventKind::Error(_)
            | EventKind::RateLimited { .. } => {
                let was_running = self.phase == Phase::Running;
                self.phase = Phase::Idle;
                self.started = None;
                was_running
            }
            _ => false,
        }
    }

    /// Whether the running panel shows this function.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// Whether the function has a run open at all (at its form or running): the same function is
    /// not started twice.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// Whether the running panel shows a spinner for it: beside its name while it runs.
    #[must_use]
    pub fn shows_spinner(&self) -> bool {
        self.is_running()
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{Counts, ErrorInfo};

    use super::*;

    #[test]
    fn a_run_goes_from_form_to_running_to_idle() {
        let now = Instant::now();
        let mut view = FunctionView::new(TaskId::Convert);
        assert!(!view.is_open());
        view.begin();
        assert_eq!(view.phase, Phase::AtForm);
        assert!(view.is_open() && !view.is_running());
        assert!(!view.apply(&EventKind::Headline("Planning".into()), now));
        assert!(!view.apply(&EventKind::Started, now));
        assert!(view.is_running() && view.started == Some(now));
        assert!(view.shows_spinner());
        view.apply(
            &EventKind::Counts(Counts {
                total: Some(4),
                ..Counts::default()
            }),
            now,
        );
        assert!(
            view.shows_spinner(),
            "it spins beside its name while it runs"
        );
        assert!(view.apply(&EventKind::Finished, now));
        assert!(!view.shows_spinner());
        assert_eq!(view.phase, Phase::Idle);
        assert_eq!(view.state.counts.total, Some(4));
    }

    #[test]
    fn ending_at_the_form_is_not_a_run_that_ended() {
        let mut view = FunctionView::new(TaskId::SyncSpotify);
        view.begin();
        let error = EventKind::Error(ErrorInfo {
            message: "m".into(),
            advice: "a".into(),
        });
        assert!(!view.apply(&error, Instant::now()));
        assert_eq!(view.phase, Phase::Idle);
    }

    #[test]
    fn beginning_again_forgets_the_run_before() {
        let now = Instant::now();
        let mut view = FunctionView::new(TaskId::Review);
        view.begin();
        view.apply(&EventKind::Headline("old".into()), now);
        view.begin();
        assert_eq!(view.state.headline, "");
    }
}
