//! What the cards say, as plain data: one row per function, and for a running function the
//! progress its card shows. The screen only draws it.

use std::time::Duration;

use sdl_core::{Device, Direction, Format, RunOutcome, State, TaskId};
use sdl_tasks::estimate_remaining;

use super::{App, FunctionView};
use crate::fmt::{estimate_text, library_words, percent, stamp};
use sdl_core::words::{need_attention, thousands, x_of_y};

/// What a function's card says apart from its progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    /// The function.
    pub task: TaskId,
    /// Its name.
    pub name: &'static str,
    /// Its current selection, in words (`3 lists chosen`).
    pub selection: String,
    /// The number of held songs: only on the Review row.
    pub held: Option<usize>,
    /// Whether it is running now (its row shows it).
    pub running: bool,
}

/// A running function's progress, as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    /// The function.
    pub task: TaskId,
    /// Its name.
    pub name: &'static str,
    /// What it is doing now.
    pub headline: String,
    /// `412 of 2,117`; with no known total, what is done (`412 done`), or nothing before that.
    pub count: String,
    /// Whole percent for the bar; `None` (no known total) shows no bar.
    pub percent: Option<u16>,
    /// `about 3 min left`, or `estimating time left` until something is done.
    pub estimate: String,
    /// `3 need attention`; `None` when nothing needs it.
    pub attention: Option<String>,
}

/// One row per function, in order.
#[must_use]
pub fn main_rows(app: &App) -> Vec<RowView> {
    TaskId::ALL
        .into_iter()
        .map(|task| RowView {
            task,
            name: task.name(),
            selection: selection_text(
                task,
                &app.settings.state,
                &app.devices,
                app.settings.review.len(),
            ),
            held: (task == TaskId::Review).then(|| app.settings.review.len()),
            running: app.view(task).is_some_and(FunctionView::is_running),
        })
        .collect()
}

/// A function's remembered selection, in words.
#[must_use]
pub fn selection_text(task: TaskId, state: &State, devices: &[Device], held: usize) -> String {
    match task {
        TaskId::SyncSpotify => match state.ticked_lists.len() {
            // Nothing is remembered: the card says what the function does instead.
            0 => String::new(),
            1 => "1 list chosen".to_owned(),
            n => format!("{} lists chosen", thousands(n)),
        },
        // Nothing is remembered for these two: a link is pasted each time, and the held count
        // is the Review card's own yellow line.
        TaskId::DownloadLink | TaskId::Review => {
            let _ = held;
            String::new()
        }
        TaskId::SyncDevice => state
            .last_device
            .as_ref()
            .and_then(|id| state.devices.get(id).map(|choice| (id, choice)))
            .map_or_else(
                String::new,
                // The format only applies To device.
                |(id, choice)| {
                    let how = match choice.direction {
                        Direction::ToDevice => format!(
                            "{} · {}",
                            choice.direction.label(),
                            Format::device_label(choice.format)
                        ),
                        Direction::FromDevice => choice.direction.label().to_owned(),
                    };
                    let how = library_words(&choice.library)
                        .map_or_else(|| how.clone(), |library| format!("{how} · {library}"));
                    // The device is named by the list when it is plugged in, else by the name
                    // saved with its choices, so the row names it after a restart.
                    let name = devices
                        .iter()
                        .find(|device| device.id() == id)
                        .map(Device::label)
                        .or_else(|| state.last_device_name().map(str::to_owned));
                    match name {
                        Some(name) => format!("{name} · {how}"),
                        None => how,
                    }
                },
            ),
        TaskId::Convert => {
            let line = format!(
                "{} {} to {}",
                state.convert.format.label(),
                state.convert.quality.name(),
                state.convert.destination.display()
            );
            match library_words(&state.convert.library) {
                Some(library) => format!("{line} · {library}"),
                None => line,
            }
        }
    }
}

/// The colour role a card line is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// What the function will act on.
    Body,
    /// What the function does, shown until there is something better to say.
    Secondary,
    /// It went well.
    Done,
    /// Something needs the owner.
    Attention,
    /// Something failed.
    Failed,
    /// It was stopped.
    Skipped,
}

/// One line of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLine {
    /// The words.
    pub text: String,
    /// Their colour role.
    pub tone: Tone,
}

/// One function's card on the home screen. A card says only what helps now: what the function
/// will act on (or what it does, before anything is remembered), how its last run went where
/// that is worth knowing, and its progress while it runs, which takes the place of the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    /// The function.
    pub task: TaskId,
    /// Its name, the card's title.
    pub name: &'static str,
    /// What it will act on, or what it does.
    pub what: CardLine,
    /// A second line of explanation, for the card with room for it.
    pub note: Option<&'static str>,
    /// How the last run went, where that helps: always with its time for the two syncs, and
    /// only while fresh for the rest.
    pub result: Option<CardLine>,
    /// What Enter does here, as a verb; `None` when there is nothing to do.
    pub action: Option<&'static str>,
    /// Its progress while it runs.
    pub run: Option<RunView>,
    /// Whether it waits for the owner: held songs, or a last run that left something to look at.
    pub needs_you: bool,
}

/// What a function does, in one line.
fn purpose(task: TaskId) -> &'static str {
    match task {
        TaskId::SyncSpotify => "Liked Songs and your playlists",
        TaskId::DownloadLink => "A song or playlist from a link",
        TaskId::Review => "Nothing waiting",
        TaskId::SyncDevice => "Copy music to or from a player",
        TaskId::Convert => "Make copies in another format",
    }
}

/// The last run of `task` as a card line, or `None` where it would only be noise: a sync says
/// when it last ran, because that decides whether to run it again; a download, a review or a
/// conversion says how it went only until the app is closed.
fn result_line(app: &App, task: TaskId) -> Option<CardLine> {
    let run = app.settings.state.last_run.get(&task)?;
    let dated = matches!(task, TaskId::SyncSpotify | TaskId::SyncDevice);
    if !dated && !app.fresh.contains(&task) {
        return None;
    }
    let (glyph, tone) = match run.outcome {
        RunOutcome::Done => ("✓", Tone::Done),
        RunOutcome::NeedsAttention => ("!", Tone::Attention),
        RunOutcome::Failed => ("✗", Tone::Failed),
        RunOutcome::Stopped => ("–", Tone::Skipped),
    };
    let when = stamp(run.at);
    let text = if dated && !when.is_empty() {
        format!("{glyph} {} · {when}", run.line)
    } else {
        format!("{glyph} {}", run.line)
    };
    Some(CardLine { text, tone })
}

/// The home screen's cards, one per function, in order.
#[must_use]
pub fn cards(app: &App) -> Vec<CardView> {
    let runs = running_rows(app);
    main_rows(app)
        .into_iter()
        .map(|row| {
            let held = row.held.unwrap_or(0);
            let what = if held > 0 {
                let noun = if held == 1 { "song" } else { "songs" };
                CardLine {
                    text: format!("{} {noun} waiting", thousands(held)),
                    tone: Tone::Attention,
                }
            } else if row.selection.is_empty() {
                CardLine {
                    text: purpose(row.task).to_owned(),
                    tone: Tone::Secondary,
                }
            } else {
                CardLine {
                    text: row.selection.clone(),
                    tone: Tone::Body,
                }
            };
            let result = result_line(app, row.task);
            let action = match row.task {
                TaskId::SyncSpotify | TaskId::SyncDevice => Some("sync"),
                TaskId::DownloadLink => Some("paste a link"),
                TaskId::Review => (held > 0).then_some("review"),
                TaskId::Convert => Some("convert"),
            };
            let note = match (row.task, held) {
                (TaskId::Review, 0) => Some("Songs with no sure match wait here for you to pick."),
                (TaskId::Review, _) => Some("Each needs you to pick its match."),
                _ => None,
            };
            let needs_you = held > 0
                || result
                    .as_ref()
                    .is_some_and(|line| matches!(line.tone, Tone::Attention | Tone::Failed));
            CardView {
                task: row.task,
                name: row.name,
                what,
                note,
                result,
                action,
                run: runs.iter().find(|run| run.task == row.task).cloned(),
                needs_you,
            }
        })
        .collect()
}

/// The running functions' progress, in function order; `elapsed` is read from the clock of `app`.
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
                // No total yet (a link or a list still being read): no bar and no estimate, only
                // what is done once something is.
                None => (
                    if counts.done == 0 {
                        String::new()
                    } else {
                        format!("{} done", thousands(counts.done))
                    },
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
        let app = app();
        let rows = main_rows(&app);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].selection, "");
        assert_eq!(rows[2].held, Some(0));
        assert_eq!(rows[2].selection, "");
        assert_eq!(rows[4].selection, "MP3 High to ~/Music-converted");
        assert!(rows.iter().all(|row| !row.running));
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
                count: String::new(),
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
    fn the_device_row_names_the_remembered_device_after_a_restart() {
        // Nothing has been listed yet and no form was opened: the name comes from the saved state.
        let mut state = State {
            last_device: Some("uuid".into()),
            ..State::default()
        };
        state.devices.insert(
            "uuid".into(),
            sdl_core::DeviceChoice {
                name: "SNOWSKY".into(),
                format: Some(Format::Mp3),
                ..sdl_core::DeviceChoice::default()
            },
        );
        let mut app = app();
        app.settings.state = state;
        assert_eq!(app.devices, Vec::new());
        assert_eq!(main_rows(&app)[3].selection, "SNOWSKY · To device · MP3");
    }

    #[test]
    fn a_selection_line_names_a_library_that_is_not_the_owners_own() {
        let mut state = State {
            last_device: Some("uuid".into()),
            ..State::default()
        };
        state.devices.insert(
            "uuid".into(),
            sdl_core::DeviceChoice {
                name: "SNOWSKY".into(),
                library: sdl_core::LibraryId::Other("Sam".into()),
                ..sdl_core::DeviceChoice::default()
            },
        );
        state.convert.library = sdl_core::LibraryId::Other("Kim".into());
        assert_eq!(
            selection_text(TaskId::SyncDevice, &state, &[], 0),
            "SNOWSKY · To device · As is · library Sam"
        );
        assert_eq!(
            selection_text(TaskId::Convert, &state, &[], 0),
            "MP3 High to ~/Music-converted · library Kim"
        );
        // The owner's own is not named.
        state.convert.library = sdl_core::LibraryId::Own;
        assert_eq!(
            selection_text(TaskId::Convert, &state, &[], 0),
            "MP3 High to ~/Music-converted"
        );
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
            selection_text(TaskId::SyncDevice, &state, &[], 0),
            "To device · MP3"
        );
        let image = Device {
            name: "SDLTEST".into(),
            mount_point: "/Volumes/SDLTEST".into(),
            volume_uuid: Some("uuid".into()),
            free_bytes: 1,
            disk_image: true,
            not_mounted: None,
        };
        assert_eq!(
            selection_text(TaskId::SyncDevice, &state, &[image], 0),
            "SDLTEST (disk image) · To device · MP3"
        );
    }

    #[test]
    fn a_card_says_what_its_function_does_until_something_is_remembered() {
        let app = app();
        let shown = cards(&app);
        assert_eq!(shown[0].what.text, "Liked Songs and your playlists");
        assert_eq!(shown[0].what.tone, Tone::Secondary);
        assert_eq!(shown[1].action, Some("paste a link"));
        // Nothing is held: Review has nothing to do, and no card shows an empty result.
        assert_eq!(shown[2].action, None);
        assert!(
            shown
                .iter()
                .all(|card| card.result.is_none() && card.run.is_none() && !card.needs_you)
        );
    }

    #[test]
    fn a_sync_always_says_when_it_last_ran_and_a_download_only_while_fresh() {
        let mut app = app();
        for task in [TaskId::SyncSpotify, TaskId::DownloadLink] {
            app.settings.state.last_run.insert(
                task,
                LastRun {
                    line: "14 added".into(),
                    at: 400,
                    outcome: RunOutcome::Done,
                    attention: Vec::new(),
                },
            );
        }
        let shown = cards(&app);
        let sync = shown[0].result.clone().map(|line| line.text);
        assert!(
            sync.as_deref()
                .is_some_and(|text| text.starts_with("✓ 14 added · ") && text.contains('/')),
            "{sync:?}"
        );
        assert_eq!(shown[1].result, None);
        app.fresh.push(TaskId::DownloadLink);
        assert_eq!(
            cards(&app)[1].result,
            Some(CardLine {
                text: "✓ 14 added".into(),
                tone: Tone::Done
            })
        );
    }

    #[test]
    fn a_running_function_carries_its_progress_and_a_failed_sync_needs_you() {
        let mut app = app();
        feed(&mut app, EventKind::Started);
        assert!(cards(&app)[0].run.is_some());
        let mut idle = self::app();
        idle.settings.state.last_run.insert(
            TaskId::SyncSpotify,
            LastRun {
                line: "409 added, 2 failed".into(),
                at: 400,
                outcome: RunOutcome::Failed,
                attention: Vec::new(),
            },
        );
        let card = &cards(&idle)[0];
        assert!(card.needs_you);
        assert_eq!(
            card.result.as_ref().map(|line| line.tone),
            Some(Tone::Failed)
        );
    }
}
