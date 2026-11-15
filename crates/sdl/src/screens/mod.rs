//! The two screens. No others.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::App;

pub mod home;
pub mod run;

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Sync Spotify, Download link, Sync card, Quit.
    Home,
    /// Song table, filter, progress gauge, activity line, sign-in and rate-limit state.
    Run,
}

/// Cells a bordered block adds on each axis (one border on each side).
const BORDERS: u16 = 2;
/// Gap between two key hints.
const HINT_GAP: &str = "   ";

/// One key hint: the key(s) and what they do.
type Hint = (&'static str, &'static str);

/// Draw the current screen.
pub fn render(app: &mut App, frame: &mut Frame) {
    match app.screen {
        Screen::Home => home::render(app, frame),
        Screen::Run => run::render(app, frame),
    }
}

/// Draw the key hints on the bottom rows of the frame and return the area above them for the
/// screen body. The hints wrap and are laid out before the body, so what to press stays visible
/// however small the terminal is.
fn hints_and_body(frame: &mut Frame, hints: &[Hint]) -> Rect {
    let area = frame.area();
    let hint = hint_line(hints).wrap(Wrap { trim: true });
    let rows = u16::try_from(hint.line_count(area.width)).unwrap_or(u16::MAX);
    let [body, hint_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(rows)]).areas(area);
    frame.render_widget(hint, hint_area);
    body
}

/// The key hints, drawn the same way on every screen: each key in bold, its action dimmed.
fn hint_line(hints: &[Hint]) -> Paragraph<'static> {
    let mut spans = Vec::new();
    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(HINT_GAP));
        }
        spans.push(Span::styled(
            *key,
            Style::new().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {action}"),
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    Paragraph::new(Line::from(spans))
}
