//! What the main panel and the running panel say, as plain data. The screen only draws it.

use std::time::Duration;

use sdl_core::{Direction, Format, RunOutcome, State, TaskId};
use sdl_tasks::estimate_remaining;

use super::{App, FunctionView};
use crate::fmt::{ago, estimate_text, percent};
use sdl_core::words::{need_attention, thousands, x_of_y};

/// A function's last-run line, with its age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastView {
    /// `412 added, 3 need attention · 5 min ago`.
    pub text: String,
    /// Picks the colour: done green, needs attention yellow, failed red, stopped skipped-grey.
    pub outcome: RunOutcome,
}

/// One row of the main panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    /// The function.
    pub task: TaskId,
    /// Its name.
    pub name: &'static str,
    /// Its current selection, in words (`3 lists chosen`).
    pub selection: String,
    /// Its last run; `None` before the first run.
    pub last: Option<LastView>,
    /// The number of held songs: only on the Review row.
    pub held: Option<usize>,
    /// Whether it is running now (its row shows it).
    pub running: bool,
}

/// One task of the running panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    /// The function.
    pub task: TaskId,
    /// Its name.
    pub name: &'static str,
    /// What it is doing now.
    pub headline: String,
    /// `412 of 2,117`; with no known total, only what is done (`412`).
    pub count: String,
    /// Whole percent for the bar; `None` shows a spinner instead (no known total).
    pub percent: Option<u16>,
    /// `about 3 min left`, or `estimating time left` until something is done.
    pub estimate: String,
    /// `3 need attention`; `None` when nothing needs it.
    pub attention: Option<String>,
}

/// The main panel's rows, one per function, in order.
#[must_use]
pub fn main_rows(app: &App) -> Vec<RowView> {
    TaskId::ALL
        .into_iter()
        .map(|task| {
            let last = app.settings.state.last_run.get(&task).map(|run| LastView {
                text: format!("{} · {}", run.line, ago(app.clock.epoch, run.at)),
                outcome: run.outcome,
            });
            RowView {
                task,
                name: task.name(),
                selection: selection_text(task, &app.settings.state, app.settings.review.len()),
                last,
                held: (task == TaskId::Review).then(|| app.settings.review.len()),
                running: app.view(task).is_some_and(FunctionView::is_running),
            }
        })
        .collect()
}

/// A function's remembered selection, in words.
#[must_use]
pub fn selection_text(task: TaskId, state: &State, held: usize) -> String {
    match task {
        TaskId::SyncSpotify => match state.ticked_lists.len() {
            0 => "no lists chosen yet".to_owned(),
            1 => "1 list chosen".to_owned(),
            n => format!("{} lists chosen", thousands(n)),
        },
        TaskId::DownloadLink => "paste a link".to_owned(),
        // The count itself is the row's yellow `N held songs waiting`.
        TaskId::Review => match held {
            0 => "nothing waiting".to_owned(),
            _ => String::new(),
        },
        TaskId::SyncDevice => state
            .last_device
            .as_ref()
            .and_then(|id| state.devices.get(id))
            .map_or_else(
                || "no device synced yet".to_owned(),
                // The format only applies To device.
                |choice| match choice.direction {
                    Direction::ToDevice => format!(
                        "{} · {}",
                        choice.direction.label(),
                        Format::device_label(choice.format)
                    ),
                    Direction::FromDevice => choice.direction.label().to_owned(),
                },
            ),
        TaskId::Convert => format!(
            "{} {} to {}",
            state.convert.format.label(),
            state.convert.quality.name(),
            state.convert.destination.display()
        ),
    }
}

/// The running panel's tasks, in panel order; `elapsed` is read from the clock of `app`.
#[must_use]
pub fn running_rows(app: &App) -> Vec<RunView> {
    app.running()
        .map(|view| {
            let counts = view.state.counts;
            let elapsed = view.started.map_or(Duration::ZERO, |at| {
                app.clock.instant.saturating_duration_since(at)
            });
            let (count, percent_done, estimate) = match counts.total {
                Some(total) => (
                    x_of_y(counts.done, total),
                    Some(percent(counts.done, total)),
                    estimate_text(estimate_remaining(elapsed, counts.done, total)),
                ),
                None => (
                    thousands(counts.done),
                    None,
                    "estimating time left".to_owned(),
                ),
            };
            RunView {
                task: view.task,
                name: view.task.name(),
                headline: view.state.headline.clone(),
                count,
                percent: percent_done,
                estimate,
                attention: (counts.need_attention() > 0)
                    .then(|| need_attention(counts.need_attention())),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::layout::Size;
    use sdl_core::{Counts, Event, EventKind, LastRun};

    use super::*;
    use crate::app::{Clock, Msg, Settings};

    fn app() -> App {
        let clock = Clock {
            instant: Instant::now(),
            epoch: 1_000,
        };
        App::new(Settings::default(), true, Size::new(100, 30), clock)
    }

    fn feed(app: &mut App, kind: EventKind) {
        let clock = app.clock;
        let _ = app.update(
            Msg::Task(Event {
                task: TaskId::SyncSpotify,
                kind,
            }),
            clock,
        );
    }

    #[test]
    fn every_function_has_a_row_with_a_selection_and_the_review_count() {
        let mut app = app();
        let rows = main_rows(&app);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].selection, "no lists chosen yet");
        assert_eq!(rows[2].held, Some(0));
        assert_eq!(rows[2].selection, "nothing waiting");
        assert_eq!(rows[4].selection, "MP3 High to ~/Music-converted");
        assert!(rows.iter().all(|row| row.last.is_none() && !row.running));
        app.settings.state.last_run.insert(
            TaskId::Convert,
            LastRun {
                line: "12 converted".into(),
                at: 400,
                outcome: RunOutcome::Done,
                attention: Vec::new(),
            },
        );
        let rows = main_rows(&app);
        assert_eq!(
            rows[4].last,
            Some(LastView {
                text: "12 converted · 10 min ago".into(),
                outcome: RunOutcome::Done
            })
        );
    }

    #[test]
    fn a_running_task_shows_count_bar_estimate_and_attention() {
        let mut app = app();
        feed(&mut app, EventKind::Started);
        feed(&mut app, EventKind::Headline("Downloading".into()));
        assert_eq!(
            running_rows(&app)[0],
            RunView {
                task: TaskId::SyncSpotify,
                name: "Sync Spotify",
                headline: "Downloading".into(),
                count: "0".into(),
                percent: None,
                estimate: "estimating time left".into(),
                attention: None,
            }
        );
        feed(
            &mut app,
            EventKind::Counts(Counts {
                total: Some(2_117),
                done: 412,
                held: 2,
                failed: 1,
                ..Counts::default()
            }),
        );
        let run = &running_rows(&app)[0];
        assert_eq!(run.count, "412 of 2,117");
        assert_eq!(run.percent, Some(19));
        assert_eq!(run.attention.as_deref(), Some("3 need attention"));
    }

    #[test]
    fn a_device_row_names_direction_and_format() {
        let mut state = State {
            last_device: Some("uuid".into()),
            ..State::default()
        };
        state.devices.insert(
            "uuid".into(),
            sdl_core::DeviceChoice {
                format: Some(Format::Mp3),
                ..sdl_core::DeviceChoice::default()
            },
        );
        assert_eq!(
            selection_text(TaskId::SyncDevice, &state, 0),
            "To device · MP3"
        );
    }
}
