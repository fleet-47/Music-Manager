//! Driving a run (spec H4): the one outcome type, the one tally and the one driver every
//! function's run goes through.
//!
//! Interface, in full:
//! * A run is a list of jobs. Each job ends as an [`Outcome`], or as an error that ends the whole
//!   run.
//! * [`Run::drive`] runs the jobs `limit` at a time. It sends `Counts` once before anything runs
//!   and again after every outcome, and an `Attention` for every held or failed outcome. The
//!   total is every job plus every already settled outcome.
//! * `settled` are outcomes known before the run (a link that could not be read): they are
//!   counted first, in order, like jobs that have finished.
//! * A job's error ends the run and is returned: no further job starts and the jobs in hand are
//!   dropped.
//! * `stop` is the cooperative stop (Convert, Sync device): once it is cancelled no further job
//!   starts, the jobs in hand finish and are counted, and the tally so far is returned. A job
//!   that never started is not counted. A hard stop (the download functions) is the caller
//!   aborting the task, which drops the run; nothing here is needed for it.

use std::num::NonZeroUsize;

pub use tokio_util::sync::CancellationToken;

use futures::{StreamExt, stream};

use crate::{Attention, Counts, EventKind, EventSender};

/// Run `job` unless a stop was asked for by the time it is first polled; `None` means it never
/// started.
async fn unless_stopped<J, T>(stop: &CancellationToken, job: J) -> Option<T>
where
    J: Future<Output = T>,
{
    if stop.is_cancelled() {
        None
    } else {
        Some(job.await)
    }
}

/// Count `outcome` and send its attention item, if any, then the new counts.
fn report(tally: &mut Tally, outcome: Outcome, events: &EventSender) {
    if let Some(item) = tally.record(outcome) {
        events.send(EventKind::Attention(item));
    }
    events.send(EventKind::Counts(tally.counts()));
}

/// How one job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Downloaded and placed in the library, or converted, or copied.
    Added,
    /// Placed in the library, but its cover could not be downloaded or stored (F6): counted as
    /// added, and in the last-run line's "without a cover".
    AddedWithoutCover,
    /// Already there; nothing was written.
    Skipped,
    /// Held for review: no confident match.
    Held(Attention),
    /// Failed; the run goes on.
    Failed(Attention),
}

/// The counts of one run: what the UI keeps ([`Counts`]) and the songs added without a cover,
/// which only the last-run line uses (F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    counts: Counts,
    without_cover: usize,
}

impl Tally {
    /// A tally for `total` items, nothing done.
    #[must_use]
    pub fn new(total: usize) -> Self {
        Self {
            counts: Counts {
                total: Some(total),
                ..Counts::default()
            },
            without_cover: 0,
        }
    }

    /// The counts so far.
    #[must_use]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Songs added without a cover.
    #[must_use]
    pub fn without_cover(&self) -> usize {
        self.without_cover
    }

    /// This tally with `already` more items counted as skipped: the items left out of the run
    /// before it started, for the last-run line. The run itself counts only the work it does.
    #[must_use]
    pub fn with_already(self, already: usize) -> Self {
        Self {
            counts: Counts {
                skipped: self.counts.skipped.saturating_add(already),
                ..self.counts
            },
            ..self
        }
    }

    /// Count one finished item. Returns the attention item when the outcome needs attention.
    pub fn record(&mut self, outcome: Outcome) -> Option<Attention> {
        self.counts.done = self.counts.done.saturating_add(1);
        match outcome {
            Outcome::Added => {
                self.counts.added = self.counts.added.saturating_add(1);
                None
            }
            Outcome::AddedWithoutCover => {
                self.counts.added = self.counts.added.saturating_add(1);
                self.without_cover = self.without_cover.saturating_add(1);
                None
            }
            Outcome::Skipped => {
                self.counts.skipped = self.counts.skipped.saturating_add(1);
                None
            }
            Outcome::Held(item) => {
                self.counts.held = self.counts.held.saturating_add(1);
                Some(item)
            }
            Outcome::Failed(item) => {
                self.counts.failed = self.counts.failed.saturating_add(1);
                Some(item)
            }
        }
    }
}

/// How a run is driven; see the module's interface.
#[derive(Debug)]
pub struct Run<'a> {
    /// How many jobs run at once.
    pub limit: NonZeroUsize,
    /// Where `Counts` and `Attention` go.
    pub events: &'a EventSender,
    /// The cooperative stop. A function that is only ever stopped hard passes a token nobody
    /// cancels.
    pub stop: &'a CancellationToken,
    /// Outcomes known before the run, counted first.
    pub settled: Vec<Outcome>,
}

impl Run<'_> {
    /// Run `jobs` to the end, or to the stop, or to the first error.
    ///
    /// # Errors
    /// The first error a job returns.
    pub async fn drive<J, E>(self, jobs: Vec<J>) -> Result<Tally, E>
    where
        J: Future<Output = Result<Outcome, E>>,
    {
        let mut tally = Tally::new(jobs.len().saturating_add(self.settled.len()));
        self.events.send(EventKind::Counts(tally.counts()));
        for outcome in self.settled {
            report(&mut tally, outcome, self.events);
        }
        let mut guarded = Vec::with_capacity(jobs.len());
        for job in jobs {
            guarded.push(unless_stopped(self.stop, job));
        }
        let mut finished = stream::iter(guarded).buffer_unordered(self.limit.get());
        while let Some(result) = finished.next().await {
            match result {
                Some(Ok(outcome)) => report(&mut tally, outcome, self.events),
                Some(Err(error)) => return Err(error),
                None => {}
            }
        }
        Ok(tally)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{EventKind, EventReceiver, TaskId, channel};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn item(title: &str) -> Attention {
        Attention::failed(title.into(), "A".into(), "why".into())
    }

    fn sender() -> (EventSender, EventReceiver) {
        let (tx, rx) = channel();
        (EventSender::new(TaskId::Convert, tx), rx)
    }

    fn sent(rx: &mut EventReceiver) -> Vec<EventKind> {
        let mut kinds = Vec::new();
        while let Ok(event) = rx.try_recv() {
            kinds.push(event.kind);
        }
        kinds
    }

    fn limit(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap_or(NonZeroUsize::MIN)
    }

    /// What a test job does when it runs.
    #[derive(Clone, Copy)]
    enum Does {
        Add,
        Fail,
        /// Ask for the cooperative stop, then finish as added.
        AskToStop,
    }

    /// A job that counts itself in `ran` when it starts.
    async fn job(
        does: Does,
        ran: &AtomicUsize,
        stop: &CancellationToken,
    ) -> Result<Outcome, String> {
        ran.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        match does {
            Does::Add => Ok(Outcome::Added),
            Does::Fail => Err("the disk is gone".to_owned()),
            Does::AskToStop => {
                stop.cancel();
                Ok(Outcome::Added)
            }
        }
    }

    #[tokio::test]
    async fn counts_go_out_before_the_run_and_after_every_outcome() -> TestResult {
        let (events, mut rx) = sender();
        let stop = CancellationToken::new();
        let run = Run {
            limit: limit(1),
            events: &events,
            stop: &stop,
            settled: vec![Outcome::Failed(item("unread link"))],
        };
        let jobs = vec![
            std::future::ready(Ok::<_, String>(Outcome::Added)),
            std::future::ready(Ok(Outcome::Skipped)),
            std::future::ready(Ok(Outcome::Held(item("held")))),
            std::future::ready(Ok(Outcome::AddedWithoutCover)),
        ];
        let tally = run.drive(jobs).await?;
        let expected = Counts {
            total: Some(5),
            done: 5,
            added: 2,
            skipped: 1,
            held: 1,
            failed: 1,
        };
        assert_eq!(tally.counts(), expected);
        assert_eq!(tally.without_cover(), 1);
        let kinds = sent(&mut rx);
        // Nothing done yet, with the whole total: the bar starts at once.
        assert_eq!(
            kinds.first(),
            Some(&EventKind::Counts(Counts {
                total: Some(5),
                ..Counts::default()
            }))
        );
        // The settled outcome is counted first, then the jobs, in order (one at a time).
        assert_eq!(
            kinds.get(1..3),
            Some(
                &[
                    EventKind::Attention(item("unread link")),
                    EventKind::Counts(Counts {
                        total: Some(5),
                        done: 1,
                        failed: 1,
                        ..Counts::default()
                    }),
                ][..]
            )
        );
        let counts = kinds
            .iter()
            .filter(|kind| matches!(kind, EventKind::Counts(_)))
            .count();
        assert_eq!(counts, 6, "once before, once after each of five");
        assert!(kinds.contains(&EventKind::Attention(item("held"))));
        assert_eq!(kinds.last(), Some(&EventKind::Counts(expected)));
        Ok(())
    }

    #[tokio::test]
    async fn no_more_jobs_run_at_once_than_the_limit() -> TestResult {
        async fn busy(now: &AtomicUsize, peak: &AtomicUsize) -> Result<Outcome, String> {
            let running = now.fetch_add(1, Ordering::SeqCst).saturating_add(1);
            peak.fetch_max(running, Ordering::SeqCst);
            tokio::task::yield_now().await;
            now.fetch_sub(1, Ordering::SeqCst);
            Ok(Outcome::Added)
        }
        let (events, _rx) = sender();
        let stop = CancellationToken::new();
        let (now, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let mut jobs = Vec::new();
        for _ in 0..6 {
            jobs.push(busy(&now, &peak));
        }
        let run = Run {
            limit: limit(2),
            events: &events,
            stop: &stop,
            settled: Vec::new(),
        };
        assert_eq!(run.drive(jobs).await?.counts().done, 6);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn a_jobs_error_ends_the_run_and_no_further_job_starts() {
        let (events, _rx) = sender();
        let stop = CancellationToken::new();
        let ran = AtomicUsize::new(0);
        let jobs = vec![
            job(Does::Add, &ran, &stop),
            job(Does::Fail, &ran, &stop),
            job(Does::Add, &ran, &stop),
        ];
        let run = Run {
            limit: limit(1),
            events: &events,
            stop: &stop,
            settled: Vec::new(),
        };
        assert_eq!(
            run.drive(jobs).await.err().as_deref(),
            Some("the disk is gone")
        );
        assert_eq!(ran.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_stop_lets_the_job_in_hand_finish_and_starts_no_other() -> TestResult {
        let (events, mut rx) = sender();
        let stop = CancellationToken::new();
        let ran = AtomicUsize::new(0);
        let jobs = vec![
            job(Does::Add, &ran, &stop),
            job(Does::AskToStop, &ran, &stop),
            job(Does::Add, &ran, &stop),
            job(Does::Add, &ran, &stop),
        ];
        let run = Run {
            limit: limit(1),
            events: &events,
            stop: &stop,
            settled: Vec::new(),
        };
        let tally = run.drive(jobs).await?;
        // The job that was in hand when the stop came is counted; the two after it never ran.
        assert_eq!(ran.load(Ordering::SeqCst), 2);
        let expected = Counts {
            total: Some(4),
            done: 2,
            added: 2,
            ..Counts::default()
        };
        assert_eq!(tally.counts(), expected);
        assert_eq!(sent(&mut rx).last(), Some(&EventKind::Counts(expected)));
        Ok(())
    }

    #[tokio::test]
    async fn a_stop_asked_before_the_run_starts_nothing() -> TestResult {
        let (events, _rx) = sender();
        let stop = CancellationToken::new();
        stop.cancel();
        let ran = AtomicUsize::new(0);
        let jobs = vec![job(Does::Add, &ran, &stop), job(Does::Add, &ran, &stop)];
        let run = Run {
            limit: limit(2),
            events: &events,
            stop: &stop,
            settled: Vec::new(),
        };
        let tally = run.drive(jobs).await?;
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert_eq!((tally.counts().total, tally.counts().done), (Some(2), 0));
        Ok(())
    }

    #[test]
    fn the_future_of_a_drive_is_send_when_its_jobs_are() {
        fn is_send<T: Send>(_: &T) {}
        let (events, _rx) = sender();
        let stop = CancellationToken::new();
        let ran = AtomicUsize::new(0);
        let jobs = vec![job(Does::Add, &ran, &stop)];
        let run = Run {
            limit: limit(1),
            events: &events,
            stop: &stop,
            settled: Vec::new(),
        };
        is_send(&run.drive(jobs));
    }

    #[test]
    fn a_tally_counts_every_outcome() {
        let mut tally = Tally::new(5);
        assert_eq!(tally.record(Outcome::Added), None);
        assert_eq!(tally.record(Outcome::Skipped), None);
        assert_eq!(tally.record(Outcome::Held(item("u"))), Some(item("u")));
        assert_eq!(tally.record(Outcome::Failed(item("f"))), Some(item("f")));
        assert_eq!(tally.record(Outcome::AddedWithoutCover), None);
        assert_eq!(tally.without_cover(), 1);
        assert_eq!(
            tally.counts(),
            Counts {
                total: Some(5),
                done: 5,
                added: 2,
                skipped: 1,
                held: 1,
                failed: 1,
            }
        );
        assert_eq!(tally.with_already(10).counts().skipped, 11);
    }
}
