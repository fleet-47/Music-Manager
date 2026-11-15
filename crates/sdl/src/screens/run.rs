//! Run screen: song table (title, artist, list, stage, status), status filter, overall progress gauge,
//! current-activity line, sign-in state and 429 wait.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Gauge, Paragraph, Row, Table, Wrap};
use sdl_core::{SignInState, Stage, Status};

use super::{BORDERS, Hint, hints_and_body};
use crate::app::{Action, App, SongRow};
use crate::screens::Screen;

/// Fewest song rows the table keeps when long status lines need room.
const MIN_TABLE_ROWS: u16 = 1;
/// Rows inside the progress gauge box.
const GAUGE_ROWS: u16 = 1;
/// Height of the current-activity line.
const ACTIVITY_ROWS: u16 = 1;
/// Rows the table header takes.
const HEADER_ROWS: u16 = 1;
/// Relative widths of the Title, Artist and List columns; Stage and Status are as wide as their
/// longest label.
const TEXT_COLUMN_WEIGHTS: [u16; 3] = [3, 2, 2];

/// Column headers, in display order.
const HEADERS: [&str; 5] = ["Title", "Artist", "List", "Stage", "Status"];
/// Every stage, for sizing the Stage column.
const STAGES: [Stage; 5] = [
    Stage::Listed,
    Stage::Matching,
    Stage::Downloading,
    Stage::Enriching,
    Stage::Tagging,
];

const QUEUED: &str = "queued";
const WORKING: &str = "working";
const SKIPPED: &str = "skipped (exists)";
const UNMATCHED: &str = "unmatched";
const DONE: &str = "done";
const FAILED: &str = "failed";
/// Every status label, for sizing the Status column.
const STATUS_LABELS: [&str; 6] = [QUEUED, WORKING, SKIPPED, UNMATCHED, DONE, FAILED];

/// Key hints while a run is in progress.
const RUNNING_HINTS: &[Hint] = &[
    ("f", "filter"),
    ("Up/Down", "select a song to see why it failed"),
    ("q/Esc", "stop and go back"),
    ("Ctrl+C", "quit"),
];
/// Key hints after a run has ended.
const ENDED_HINTS: &[Hint] = &[
    ("f", "filter"),
    ("Up/Down", "select a song to see why it failed"),
    ("q/Esc", "back"),
    ("Ctrl+C", "quit"),
];
/// Key hints while the card cleanup-and-unmount question is open.
const CARD_HINTS: &[Hint] = &[
    ("y", "clean up and unmount"),
    ("n/Esc", "leave mounted"),
    ("Ctrl+C", "quit"),
];

/// Draw the Run screen.
pub fn render(app: &mut App, frame: &mut Frame) {
    let body = hints_and_body(frame, hints(app));
    // Laid out in order of importance, so a short terminal loses the table before anything else:
    // key hints, then the gauge and activity line, then the head lines, then the table and the
    // failure detail.
    let [top, gauge_area, activity_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(GAUGE_ROWS.saturating_add(BORDERS)),
        Constraint::Length(ACTIVITY_ROWS),
    ])
    .areas(body);
    let width = top.width;
    // Sign-in, rate-limit, error and card lines wrap, so a long error is shown in full.
    let head = Paragraph::new(head_lines(app)).wrap(Wrap { trim: false });
    let head_height = u16::try_from(head.line_count(width)).unwrap_or(u16::MAX);
    // The status cell has no room for a failure reason, so the selected song's reason is shown in
    // full under the table.
    let detail = Paragraph::new(
        selected_failure(app)
            .into_iter()
            .map(Line::from)
            .collect::<Vec<_>>(),
    )
    .style(Style::new().fg(Color::Red))
    .wrap(Wrap { trim: false });
    let detail_height = u16::try_from(detail.line_count(width)).unwrap_or(u16::MAX);
    let [head_area, rest] =
        Layout::vertical([Constraint::Length(head_height), Constraint::Fill(1)]).areas(top);
    let [table_area, detail_area] = Layout::vertical([
        Constraint::Min(
            MIN_TABLE_ROWS
                .saturating_add(HEADER_ROWS)
                .saturating_add(BORDERS),
        ),
        Constraint::Length(detail_height),
    ])
    .areas(rest);

    frame.render_widget(head, head_area);

    let filter = app.filter;
    let rows: Vec<Row> = app
        .rows
        .iter()
        .filter(|row| filter.matches(&row.status))
        .map(song_row)
        .collect();
    let [title_weight, artist_weight, list_weight] = TEXT_COLUMN_WEIGHTS;
    let [_, _, _, stage_header, status_header] = HEADERS;
    let table = Table::new(
        rows,
        [
            Constraint::Fill(title_weight),
            Constraint::Fill(artist_weight),
            Constraint::Fill(list_weight),
            Constraint::Length(label_width(
                STAGES.iter().map(|stage| stage_text(*stage)),
                stage_header,
            )),
            Constraint::Length(label_width(STATUS_LABELS.into_iter(), status_header)),
        ],
    )
    .header(Row::new(HEADERS).style(Style::new().add_modifier(Modifier::BOLD)))
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
    .block(Block::bordered().title(format!(
        " Songs: {} ({}) ",
        filter.label(),
        app.visible_rows().len()
    )));
    frame.render_stateful_widget(table, table_area, &mut app.table_state);
    frame.render_widget(detail, detail_area);

    let (done, total) = app.progress;
    frame.render_widget(
        Gauge::default()
            .block(Block::bordered().title(run_title(app)))
            .percent(percent(done, total))
            .label(format!("{done}/{total}")),
        gauge_area,
    );

    frame.render_widget(Paragraph::new(app.activity.as_str()), activity_area);
}

/// Width of the widest of `labels` and `header`, for a column that must never truncate.
fn label_width<'a>(labels: impl Iterator<Item = &'a str>, header: &str) -> u16 {
    let widest = labels.map(str::len).fold(header.len(), usize::max);
    u16::try_from(widest).unwrap_or(u16::MAX)
}

/// Gauge title: whether the run is still going, ended normally, or was stopped.
fn run_title(app: &App) -> &'static str {
    if app.running {
        " Running "
    } else if app.stopped() {
        " Stopped "
    } else {
        " Finished "
    }
}

/// `<title>: <reason>` for the selected row when it failed.
fn selected_failure(app: &App) -> Option<String> {
    let visible = app.visible_rows();
    let last = visible.len().checked_sub(1)?;
    let row = visible.get(app.table_state.selected()?.min(last))?;
    match &row.status {
        Status::Failed(reason) => Some(format!("{}: {reason}", row.title)),
        _ => None,
    }
}

fn head_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(state) = &app.sign_in {
        let style = match state {
            SignInState::Failed(_) => Style::new().fg(Color::Red),
            _ => Style::new(),
        };
        lines.push(Line::from(Span::styled(
            format!("Spotify: {}", sign_in_text(state)),
            style,
        )));
    }
    if app.rate_limited {
        lines.push(Line::from(Span::styled(
            format!(
                "Spotify rate limit reached, run stopped. {}",
                wait_text(app.retry_after)
            ),
            Style::new().fg(Color::Red),
        )));
    }
    if let Some(error) = &app.error {
        lines.push(Line::from(Span::styled(
            format!("Error: {error}"),
            Style::new().fg(Color::Red),
        )));
    }
    if let Some(mount) = &app.card_mount {
        lines.push(Line::from(Span::styled(
            format!(
                "Card synced ({}). Remove the macOS dot-folders and unmount it? y/n",
                mount.display()
            ),
            Style::new().fg(Color::Yellow),
        )));
    }
    lines
}

fn hints(app: &App) -> &'static [Hint] {
    if app.card_mount.is_some() {
        CARD_HINTS
    } else if app.running {
        RUNNING_HINTS
    } else {
        ENDED_HINTS
    }
}

fn song_row(row: &SongRow) -> Row<'_> {
    let (text, color) = status_text(row.stage, &row.status);
    Row::new([
        Cell::from(row.title.as_str()),
        Cell::from(row.artist.as_str()),
        Cell::from(row.list.as_str()),
        Cell::from(stage_text(row.stage)),
        Cell::from(Span::styled(text, Style::new().fg(color))),
    ])
}

/// Display text for a stage.
#[must_use]
fn stage_text(stage: Stage) -> &'static str {
    match stage {
        Stage::Listed => "listed",
        Stage::Matching => "matching",
        Stage::Downloading => "download",
        Stage::Enriching => "genre",
        Stage::Tagging => "tagging",
    }
}

/// Display text and colour for a status. A listed song that no worker has picked up yet is
/// queued, not working. The failure reason is shown under the table for the selected song.
#[must_use]
fn status_text(stage: Stage, status: &Status) -> (&'static str, Color) {
    match status {
        Status::InProgress if stage == Stage::Listed => (QUEUED, Color::DarkGray),
        Status::InProgress => (WORKING, Color::Cyan),
        Status::SkippedExists => (SKIPPED, Color::DarkGray),
        Status::Unmatched => (UNMATCHED, Color::Yellow),
        Status::Done => (DONE, Color::Green),
        Status::Failed(_) => (FAILED, Color::Red),
    }
}

fn sign_in_text(state: &SignInState) -> String {
    match state {
        SignInState::OpeningBrowser => "opening the sign-in page in your browser".to_owned(),
        SignInState::WaitingForBrowser => "waiting for you to sign in in the browser".to_owned(),
        SignInState::SignedIn => "signed in".to_owned(),
        SignInState::Failed(reason) => format!("sign-in failed: {reason}"),
    }
}

/// Text for the 429 wait.
#[must_use]
fn wait_text(wait: Option<Duration>) -> String {
    let Some(wait) = wait else {
        return "Wait unknown (no Retry-After sent).".to_owned();
    };
    let secs = wait.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if hours > 0 {
        format!("Retry after {hours}h {minutes}m {seconds}s.")
    } else if minutes > 0 {
        format!("Retry after {minutes}m {seconds}s.")
    } else {
        format!("Retry after {seconds}s.")
    }
}

/// Gauge percentage, 0 to 100; 0 when there is nothing to do.
#[must_use]
fn percent(done: usize, total: usize) -> u16 {
    let value = done
        .saturating_mul(100)
        .checked_div(total)
        .unwrap_or(0)
        .min(100);
    u16::try_from(value).unwrap_or(100)
}

/// Handle a key on the Run screen.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    if let Some(mount) = app.card_mount.clone() {
        return match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.card_mount = None;
                app.running = true;
                "Removing dot-folders and unmounting the card".clone_into(&mut app.activity);
                Action::FinishCard(mount)
            }
            KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => {
                app.card_mount = None;
                "Card left mounted".clone_into(&mut app.activity);
                Action::None
            }
            _ => Action::None,
        };
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.screen = Screen::Home;
            app.running = false;
            Action::Back
        }
        KeyCode::Char('f') => {
            app.filter = app.filter.next();
            app.table_state.select(None);
            Action::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.table_state.select_next();
            Action::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.table_state.select_previous();
            Action::None
        }
        KeyCode::Home => {
            app.table_state.select_first();
            Action::None
        }
        KeyCode::End => {
            app.table_state.select_last();
            Action::None
        }
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use sdl_core::{Config, Event};
    use std::path::{Path, PathBuf};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn app() -> Result<App, Box<dyn std::error::Error>> {
        let cfg = Config::parse(
            "client_id = \"a\"\ncard_uuid = \"u\"\nmusic_root = \"/m\"\n",
            Path::new("/c.toml"),
        )?;
        Ok(App::new(cfg))
    }

    fn song(key: &str, status: Status) -> Event {
        Event::Song {
            key: key.into(),
            stage: Stage::Matching,
            status,
        }
    }

    #[test]
    fn card_confirmation_flow() -> TestResult {
        let mut app = app()?;
        app.screen = Screen::Run;
        app.on_worker_event(Event::CardSynced {
            mount: PathBuf::from("/Volumes/X"),
        });
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(app.on_key(key('x')), Action::None);
        assert!(app.card_mount.is_some());
        assert_eq!(
            app.on_key(key('y')),
            Action::FinishCard(PathBuf::from("/Volumes/X"))
        );
        assert!(app.card_mount.is_none());
        app.on_worker_event(Event::CardSynced {
            mount: PathBuf::from("/Volumes/X"),
        });
        assert_eq!(app.on_key(key('n')), Action::None);
        assert!(app.card_mount.is_none());
        assert_eq!(app.screen, Screen::Run);
        Ok(())
    }

    #[test]
    fn esc_goes_back_and_stops() -> TestResult {
        let mut app = app()?;
        app.screen = Screen::Run;
        app.running = true;
        let action = app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, Action::Back);
        assert_eq!(app.screen, Screen::Home);
        assert!(!app.running);
        Ok(())
    }

    #[test]
    fn selected_failure_shows_the_full_reason() -> TestResult {
        let mut app = app()?;
        app.on_worker_event(song("a", Status::Done));
        app.on_worker_event(song("b", Status::Failed("no audio".into())));
        assert_eq!(selected_failure(&app), None);
        app.table_state.select(Some(0));
        assert_eq!(selected_failure(&app), None);
        app.table_state.select(Some(9));
        assert_eq!(selected_failure(&app).as_deref(), Some("b: no audio"));
        Ok(())
    }

    #[test]
    fn listed_song_is_queued_until_a_worker_takes_it() {
        assert_eq!(status_text(Stage::Listed, &Status::InProgress).0, QUEUED);
        assert_eq!(status_text(Stage::Matching, &Status::InProgress).0, WORKING);
        assert_eq!(
            status_text(Stage::Listed, &Status::SkippedExists).0,
            SKIPPED
        );
    }

    #[test]
    fn fixed_columns_fit_their_longest_label() {
        assert_eq!(label_width(STATUS_LABELS.into_iter(), "Status"), 16);
        assert_eq!(
            label_width(STAGES.iter().map(|stage| stage_text(*stage)), "Stage"),
            8
        );
    }

    #[test]
    fn wait_and_percent_text() {
        assert_eq!(wait_text(None), "Wait unknown (no Retry-After sent).");
        assert_eq!(
            wait_text(Some(Duration::from_hours(24))),
            "Retry after 24h 0m 0s."
        );
        assert_eq!(
            wait_text(Some(Duration::from_secs(75))),
            "Retry after 1m 15s."
        );
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(9, 4), 100);
    }
}
