//! Drawing the screen (F1). Every function here is pure over [`App`]: it reads state and writes to
//! the frame, nothing else.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph, Wrap};
use sdl_core::TaskId;
use throbber_widgets_tui::{BRAILLE_SIX, Throbber, WhichUse};

use crate::app::{App, FUNCTIONS, Phase, Tone, task_name, tone};
use crate::fmt;
use crate::popups;
use crate::tasks;
use crate::theme::Theme;

/// Under this many columns the running panel moves below the main panel.
pub const NARROW_COLUMNS: u16 = 60;
/// Under this many rows only the function rows and the running counts show.
pub const COMPACT_ROWS: u16 = 12;

/// Draw the whole screen.
pub fn render(frame: &mut Frame, app: &App, theme: Theme) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme.background()), area);
    if area.height < COMPACT_ROWS {
        render_compact(frame, area, app, theme);
    } else {
        let [body, footer] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        render_body(frame, body, app, theme);
        render_footer(frame, footer, app, theme);
    }
    popups::render(frame, area, app, theme);
}

/// `text` as spans, with numbers in the emphasis colour and the rest in `base`.
#[must_use]
pub fn numbered(text: &str, base: Style, theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_is_number = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let is_number = c.is_ascii_digit()
            || (c == ',' && run_is_number && chars.peek().is_some_and(char::is_ascii_digit));
        if is_number != run_is_number && !run.is_empty() {
            spans.push(run_span(&run, run_is_number, base, theme));
            run.clear();
        }
        run_is_number = is_number;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(run_span(&run, run_is_number, base, theme));
    }
    spans
}

fn run_span(text: &str, is_number: bool, base: Style, theme: Theme) -> Span<'static> {
    let style = if is_number { theme.emphasis() } else { base };
    Span::styled(text.to_owned(), style)
}

/// The spinner glyph in the running colour.
#[must_use]
pub fn spinner(app: &App, theme: Theme) -> Span<'static> {
    Throbber::default()
        .throbber_set(BRAILLE_SIX)
        .use_type(WhichUse::Spin)
        .throbber_style(theme.running())
        .to_symbol_span(&app.throbber)
}

/// A bordered panel with a heading-styled title.
#[must_use]
pub fn panel(title: &str, theme: Theme) -> Block<'static> {
    Block::bordered()
        .title(Line::styled(format!(" {title} "), theme.heading()))
        .border_style(theme.border())
}

fn render_body(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let running = app.running();
    if running.is_empty() {
        render_main(frame, area, app, theme);
        return;
    }
    let [main, panel] = if area.width < NARROW_COLUMNS {
        // The running panel takes the room it needs; the main panel gets the rest.
        let panel_height = running
            .iter()
            .map(|task| block_lines(app, *task).saturating_add(1))
            .fold(1, u16::saturating_add);
        Layout::vertical([Constraint::Fill(1), Constraint::Length(panel_height)]).areas(area)
    } else {
        Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)])
            .flex(Flex::Start)
            .areas(area)
    };
    render_main(frame, main, app, theme);
    render_running(frame, panel, app, theme, &running);
}

/// Width of the selected row's marker column.
const MARKER: u16 = 2;

/// One function's row: its name and selection, then its last run. Wrapped to the panel's width.
fn function_row(app: &App, task: TaskId, theme: Theme, one_line: bool) -> Paragraph<'static> {
    let mut first = vec![
        Span::styled(task_name(task), theme.heading()),
        Span::raw("  "),
    ];
    let selection = app.selection_text(task);
    if task == TaskId::SyncSpotify {
        first.extend(numbered(&selection, theme.body(), theme));
    } else {
        first.push(Span::styled(selection, theme.body()));
    }
    if app.view(task).phase == Phase::Running {
        first.push(Span::styled("  running", theme.running()));
    }
    if one_line {
        return Paragraph::new(Line::from(first));
    }
    let mut second = vec![Span::styled("Last run  ", theme.secondary())];
    match app.state.last_run.get(&task) {
        Some(run) => {
            second.extend(numbered(&run.line, tone_style(&run.line, theme), theme));
            second.push(Span::styled(
                format!("  {}", fmt::ago(app.now, run.at)),
                theme.secondary(),
            ));
        }
        None => second.push(Span::styled("never", theme.secondary())),
    }
    Paragraph::new(vec![Line::from(first), Line::from(second)]).wrap(Wrap { trim: false })
}

/// Rows each function's paragraph takes at `width` columns of panel (borders included).
fn row_heights(app: &App, width: u16, theme: Theme) -> [u16; 3] {
    let text_width = width.saturating_sub(2).saturating_sub(MARKER);
    FUNCTIONS.map(|task| {
        let lines = function_row(app, task, theme, false).line_count(text_width);
        u16::try_from(lines).unwrap_or(u16::MAX)
    })
}

fn render_main(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let outer = panel("sdl", theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let mut heights = row_heights(app, area.width, theme);
    // Short of room, each function keeps one line: its name and selection.
    let one_line = heights.iter().fold(2_u16, |sum, h| sum.saturating_add(*h)) > inner.height;
    if one_line {
        heights = [1; 3];
    }
    let rows = Layout::vertical(heights.map(Constraint::Length))
        .spacing(1)
        .flex(Flex::Start)
        .split(inner);
    for (task, rect) in FUNCTIONS.into_iter().zip(rows.iter()) {
        let selected = task == app.selected;
        let [marker, text] =
            Layout::horizontal([Constraint::Length(MARKER), Constraint::Fill(1)]).areas(*rect);
        let mut row = function_row(app, task, theme, one_line);
        if selected {
            frame.render_widget(Block::new().style(theme.selected()), *rect);
            frame.render_widget(Paragraph::new(Span::styled("▶", theme.emphasis())), marker);
            row = row.style(theme.selected());
        }
        frame.render_widget(row, text);
    }
}

fn tone_style(line: &str, theme: Theme) -> Style {
    match tone(line) {
        Tone::Done => theme.done(),
        Tone::Attention => theme.attention(),
        Tone::Stopped => theme.skipped(),
        Tone::Failed => theme.failed(),
    }
}

fn render_running(frame: &mut Frame, area: Rect, app: &App, theme: Theme, running: &[TaskId]) {
    let outer = panel("Running", theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let heights = running
        .iter()
        .map(|task| Constraint::Length(block_lines(app, *task)));
    let blocks = Layout::vertical(heights)
        .spacing(1)
        .flex(Flex::Start)
        .split(inner);
    for (task, rect) in running.iter().zip(blocks.iter()) {
        render_task_block(frame, *rect, app, theme, *task);
    }
}

/// Lines of a running block: name, step, count, bar, estimate, and attention when there is any.
fn block_lines(app: &App, task: TaskId) -> u16 {
    if app.view(task).state.counts.need_attention() > 0 {
        6
    } else {
        5
    }
}

fn render_task_block(frame: &mut Frame, area: Rect, app: &App, theme: Theme, task: TaskId) {
    let view = app.view(task);
    let counts = view.state.counts;
    let rows = Layout::vertical(vec![
        Constraint::Length(1);
        usize::from(block_lines(app, task))
    ])
    .split(area);
    let row = |i: usize| rows.get(i).copied().unwrap_or_default();

    let name = Line::from(vec![
        spinner(app, theme),
        Span::styled(task_name(task), theme.heading()),
    ]);
    frame.render_widget(Paragraph::new(name), row(0));
    let step = Span::styled(format!("  {}", view.state.headline), theme.running());
    frame.render_widget(Paragraph::new(step), row(1));

    let count = format!("  {}", fmt::x_of_y(counts.done, counts.total));
    frame.render_widget(
        Paragraph::new(Line::from(numbered(&count, theme.body(), theme))),
        row(2),
    );

    let [_, bar] = Layout::horizontal([Constraint::Length(2), Constraint::Fill(1)]).areas(row(3));
    let percent = fmt::percent(counts.done, counts.total);
    let gauge = Gauge::default()
        .percent(percent)
        .gauge_style(theme.bar())
        .label(Span::styled(format!("{percent}%"), theme.body()));
    frame.render_widget(gauge, bar);

    let estimate = fmt::estimate_text(tasks::estimate_remaining(
        view.elapsed,
        counts.done,
        counts.total,
    ));
    frame.render_widget(
        Paragraph::new(Span::styled(format!("  {estimate}"), theme.secondary())),
        row(4),
    );
    if counts.need_attention() > 0 {
        let text = format!(
            "  {} need attention",
            fmt::thousands(counts.need_attention())
        );
        let line = Line::from(numbered(&text, theme.attention(), theme));
        frame.render_widget(Paragraph::new(line), row(5));
    }
}

fn render_compact(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let lines: Vec<Line> = FUNCTIONS
        .into_iter()
        .map(|task| {
            let selected = task == app.selected;
            let marker = if selected { "▶ " } else { "  " };
            let mut spans = vec![
                Span::styled(marker, theme.emphasis()),
                Span::styled(task_name(task), theme.heading()),
            ];
            let view = app.view(task);
            if view.phase == Phase::Running {
                spans.push(Span::raw("  "));
                spans.extend(numbered(
                    &fmt::x_of_y(view.state.counts.done, view.state.counts.total),
                    theme.running(),
                    theme,
                ));
            }
            let line = Line::from(spans);
            if selected {
                line.style(theme.selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(
        Paragraph::new(popups::hint_line(app, theme, area.width)),
        area,
    );
}
