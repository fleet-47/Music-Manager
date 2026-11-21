//! The main screen: the main panel (one row per function), the running panel and the footer, laid
//! out by the size rules of F1.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, LineGauge, Paragraph, Widget};
use sdl_core::RunOutcome;
use sdl_core::words::thousands;

use super::keys;
use super::theme::Theme;
use crate::app::rows::{RowView, RunView, main_rows, running_rows};
use crate::app::{App, Hits, MIN_DETAIL_ROWS, MIN_SIDE_BY_SIDE_COLS, MainTarget};

/// Rows the running panel asks for per task: name, headline, bar, estimate and attention.
const RUN_TASK_ROWS: u16 = 5;

/// Rows per task when the panel is too short for [`RUN_TASK_ROWS`]: the attention count then
/// shares the estimate's row.
const RUN_TASK_ROWS_SHORT: u16 = 4;

/// Rows the main panel needs to show every function: two lines each, and the border.
const MAIN_ROWS: u16 = 12;

/// Rows the main panel needs with one line per function.
const SHORT_ROWS: u16 = 5;

/// Where the parts of the main screen go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    /// The main panel.
    pub main: Rect,
    /// The running panel; `None` when nothing runs, or the terminal is too short for it.
    pub running: Option<Rect>,
    /// The one-line footer.
    pub footer: Rect,
}

/// Lay the main screen out in `area` (F1 Size): the footer takes the last row; under
/// [`MIN_DETAIL_ROWS`] rows only the function rows and the footer show; the running panel sits
/// right of the main panel, or below it under [`MIN_SIDE_BY_SIDE_COLS`] columns.
#[must_use]
pub fn areas(area: Rect, running_tasks: usize) -> Areas {
    let [body, footer] = area.layout(&Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
    ]));
    if running_tasks == 0 || area.height < MIN_DETAIL_ROWS {
        return Areas {
            main: body,
            running: None,
            footer,
        };
    }
    let wanted = u16::try_from(running_tasks)
        .unwrap_or(u16::MAX)
        .saturating_mul(RUN_TASK_ROWS)
        .saturating_add(2);
    let (main, running) = if area.width >= MIN_SIDE_BY_SIDE_COLS {
        let [main, running] = body.layout(
            &Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).flex(Flex::Start),
        );
        (main, running)
    } else {
        // Below the main panel: the main panel keeps the rows it needs for two lines per
        // function; a screen too short for that gives it one line per function instead.
        let constraints = if body.height >= MAIN_ROWS.saturating_add(3) {
            [
                Constraint::Fill(1),
                Constraint::Length(wanted.min(body.height.saturating_sub(MAIN_ROWS))),
            ]
        } else {
            [Constraint::Length(SHORT_ROWS), Constraint::Fill(1)]
        };
        let [main, running] = body.layout(&Layout::vertical(constraints));
        (main, running)
    };
    Areas {
        main,
        running: Some(running),
        footer,
    }
}

/// Draw the main screen and record what each rectangle means into `hits`. Returns where the
/// running panel went, so an effect can slide it in.
pub fn draw(frame: &mut Frame, app: &App, theme: Theme, hits: &mut Hits) -> Option<Rect> {
    let area = frame.area();
    let rows = running_rows(app);
    let places = areas(area, rows.len());
    let buf = frame.buffer_mut();
    buf.set_style(area, theme.background());
    draw_main(buf, places.main, app, theme, hits);
    if let Some(running) = places.running {
        draw_running(buf, running, &rows, app.spin, theme, hits);
    }
    draw_footer(buf, places.footer, app, theme);
    places.running
}

fn outcome_style(theme: Theme, outcome: RunOutcome) -> ratatui::style::Style {
    match outcome {
        RunOutcome::Done => theme.done(),
        RunOutcome::NeedsAttention => theme.attention(),
        RunOutcome::Failed => theme.failed(),
        RunOutcome::Stopped => theme.skipped(),
    }
}

fn draw_main(buf: &mut Buffer, area: Rect, app: &App, theme: Theme, hits: &mut Hits) {
    let tall = area.height >= MAIN_ROWS;
    let inner = if tall {
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(theme.border())
            .title(Line::styled(" sdl ", theme.heading()));
        let inner = block.inner(area);
        block.render(area, buf);
        inner
    } else {
        area
    };
    let per_row: u16 = if tall { 2 } else { 1 };
    for (index, row) in main_rows(app).iter().enumerate() {
        let y = inner.y.saturating_add(
            u16::try_from(index)
                .unwrap_or(u16::MAX)
                .saturating_mul(per_row),
        );
        if y.saturating_add(per_row) > inner.bottom() {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, per_row);
        let selected = row.task == app.selected;
        if selected {
            buf.set_style(rect, theme.selected());
        }
        draw_row(buf, rect, row, app, selected, theme);
        hits.main.push(rect, MainTarget::Row(row.task));
    }
}

fn draw_row(buf: &mut Buffer, rect: Rect, row: &RowView, app: &App, selected: bool, theme: Theme) {
    let name_style = if selected {
        theme.heading()
    } else if row.running {
        theme.running()
    } else {
        theme.body()
    };
    let mut first = vec![
        Span::styled(if selected { "› " } else { "  " }, theme.emphasis()),
        Span::styled(row.name, name_style),
    ];
    // On a short screen the running count comes first: it is what must not be cut off.
    if rect.height < 2
        && let Some(run) = running_rows(app).iter().find(|run| run.task == row.task)
    {
        first.push(Span::styled(format!("  {}", run.count), theme.running()));
    }
    if !row.selection.is_empty() {
        first.push(Span::styled(
            format!("  {}", row.selection),
            theme.secondary(),
        ));
    }
    if let Some(held) = row.held.filter(|held| *held > 0) {
        let noun = if held == 1 { "song" } else { "songs" };
        first.push(Span::styled(
            format!("  {} held {noun} waiting", thousands(held)),
            theme.attention(),
        ));
    }
    Line::from(first).render(Rect::new(rect.x, rect.y, rect.width, 1), buf);
    if rect.height >= 2 {
        let line = match &row.last {
            Some(last) => Line::styled(
                format!("    {}", last.text),
                outcome_style(theme, last.outcome),
            ),
            None => Line::styled("    not run yet", theme.secondary()),
        };
        line.render(
            Rect::new(rect.x, rect.y.saturating_add(1), rect.width, 1),
            buf,
        );
    }
}

fn draw_running(
    buf: &mut Buffer,
    area: Rect,
    tasks: &[RunView],
    spin: usize,
    theme: Theme,
    hits: &mut Hits,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.border())
        .title(Line::styled(" Running ", theme.heading()));
    let inner = block.inner(area);
    block.render(area, buf);
    let per_task =
        if usize::from(inner.height) >= tasks.len().saturating_mul(usize::from(RUN_TASK_ROWS)) {
            RUN_TASK_ROWS
        } else {
            RUN_TASK_ROWS_SHORT
        };
    // What does not fit is counted, never silently dropped.
    let fits = usize::from(inner.height.checked_div(per_task).unwrap_or(0));
    let shown = if tasks.len() > fits {
        fits.saturating_sub(1)
    } else {
        tasks.len()
    };
    if shown < tasks.len() {
        let y = inner
            .y
            .saturating_add(u16::try_from(shown).unwrap_or(0).saturating_mul(per_task));
        let more = tasks.len().saturating_sub(shown);
        Line::styled(format!("+{more} more running"), theme.secondary())
            .render(Rect::new(inner.x, y, inner.width, 1), buf);
    }
    for (index, task) in tasks.iter().take(shown).enumerate() {
        let y = inner.y.saturating_add(
            u16::try_from(index)
                .unwrap_or(u16::MAX)
                .saturating_mul(per_task),
        );
        let rect = Rect::new(inner.x, y, inner.width, per_task);
        hits.main.push(rect, MainTarget::Running(task.task));
        let line = |n: u16| Rect::new(rect.x, rect.y.saturating_add(n), rect.width, 1);
        Line::styled(task.name, theme.running().add_modifier(Modifier::BOLD)).render(line(0), buf);
        // The headline has its own row; one too long for it ends in "…", so it never looks cut.
        Line::styled(fit(&task.headline, rect.width), theme.secondary()).render(line(1), buf);
        if let Some(percent) = task.percent {
            LineGauge::default()
                .ratio(f64::from(percent) / 100.0)
                .label(Line::styled(task.count.as_str(), theme.emphasis()))
                .filled_style(theme.bar())
                .unfilled_style(theme.border())
                .render(line(2), buf);
        } else {
            Line::from(vec![
                Span::styled(format!("{} ", sdl_form::spinner(spin)), theme.running()),
                Span::styled(task.count.as_str(), theme.emphasis()),
            ])
            .render(line(2), buf);
        }
        let mut estimate = vec![Span::styled(task.estimate.as_str(), theme.secondary())];
        if let Some(attention) = &task.attention {
            let attention = Span::styled(attention.as_str(), theme.attention());
            if per_task == RUN_TASK_ROWS {
                Line::from(attention).render(line(4), buf);
            } else {
                estimate.push(Span::styled(" · ", theme.secondary()));
                estimate.push(attention);
            }
        }
        Line::from(estimate).render(line(3), buf);
    }
}

/// `text` as it fits in `width` columns: whole, or cut with "…" at the end.
fn fit(text: &str, width: u16) -> String {
    let width = usize::from(width);
    if Line::raw(text).width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    for c in text.chars() {
        out.push(c);
        if Line::raw(out.as_str()).width().saturating_add(1) > width {
            out.pop();
            break;
        }
    }
    out.push('…');
    out
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, theme: Theme) {
    let mut spans = Vec::new();
    for hint in keys::fit(&keys::footer(app), area.width) {
        spans.push(Span::styled(hint.key, theme.emphasis()));
        spans.push(Span::styled(
            format!(" {}{}", hint.what, " ".repeat(keys::GAP)),
            theme.secondary(),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_running_gives_the_main_panel_the_whole_body() {
        let places = areas(Rect::new(0, 0, 100, 30), 0);
        assert_eq!(places.main, Rect::new(0, 0, 100, 29));
        assert_eq!(places.running, None);
        assert_eq!(places.footer, Rect::new(0, 29, 100, 1));
    }

    #[test]
    fn the_running_panel_sits_right_when_wide_and_below_when_narrow() -> Result<(), &'static str> {
        let wide = areas(Rect::new(0, 0, 100, 30), 1);
        let right = wide.running.ok_or("no running panel when wide")?;
        assert_eq!(right.y, wide.main.y);
        assert!(right.x >= wide.main.right());
        let narrow = areas(Rect::new(0, 0, 59, 30), 1);
        let below = narrow.running.ok_or("no running panel when narrow")?;
        assert_eq!(below.x, narrow.main.x);
        assert!(below.y >= narrow.main.bottom());
        Ok(())
    }

    #[test]
    fn a_long_headline_ends_in_an_ellipsis() {
        assert_eq!(fit("Reading the link", 30), "Reading the link");
        assert_eq!(fit("Getting Judy Is a Punk by Ramones", 12), "Getting Jud…");
        assert_eq!(
            Line::raw(fit("Getting Judy Is a Punk", 12).as_str()).width(),
            12
        );
    }

    #[test]
    fn a_short_terminal_shows_only_rows_and_footer() {
        let places = areas(Rect::new(0, 0, 100, 11), 2);
        assert_eq!(places.running, None);
        assert_eq!(places.main.height, 10);
    }
}
