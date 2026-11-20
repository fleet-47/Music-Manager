//! The main screen: the main panel (one row per function), the running panel and the footer, laid
//! out by the size rules of F1.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, LineGauge, Paragraph, Widget};
use sdl_core::RunOutcome;

use super::keys;
use super::theme::Theme;
use crate::app::rows::{RowView, RunView, main_rows, running_rows};
use crate::app::{App, Hits, MIN_DETAIL_ROWS, MIN_SIDE_BY_SIDE_COLS, MainTarget};

/// Rows the running panel asks for per task: name and headline, bar, estimate and attention.
const RUN_TASK_ROWS: u16 = 4;

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
        let [main, running] = body.layout(&Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(wanted.min(body.height.saturating_sub(5))),
        ]));
        (main, running)
    };
    Areas {
        main,
        running: Some(running),
        footer,
    }
}

/// Draw the main screen and record what each rectangle means into `hits`.
pub fn draw(frame: &mut Frame, app: &App, theme: Theme, hits: &mut Hits) {
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
    let tall = area.height >= MIN_DETAIL_ROWS;
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
        Span::styled(format!("  {}", row.selection), theme.secondary()),
    ];
    if let Some(held) = row.held.filter(|held| *held > 0) {
        first.push(Span::styled(format!("  {held} held"), theme.attention()));
    }
    if rect.height < 2
        && let Some(run) = running_rows(app).iter().find(|run| run.task == row.task)
    {
        first.push(Span::styled(format!("  {}", run.count), theme.running()));
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
    for (index, task) in tasks.iter().enumerate() {
        let y = inner.y.saturating_add(
            u16::try_from(index)
                .unwrap_or(u16::MAX)
                .saturating_mul(RUN_TASK_ROWS),
        );
        if y.saturating_add(RUN_TASK_ROWS) > inner.bottom() {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, RUN_TASK_ROWS);
        hits.main.push(rect, MainTarget::Running(task.task));
        let line = |n: u16| Rect::new(rect.x, rect.y.saturating_add(n), rect.width, 1);
        Line::from(vec![
            Span::styled(task.name, theme.running().add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", task.headline), theme.secondary()),
        ])
        .render(line(0), buf);
        if let Some(percent) = task.percent {
            LineGauge::default()
                .ratio(f64::from(percent) / 100.0)
                .label(Line::styled(task.count.as_str(), theme.emphasis()))
                .filled_style(theme.bar())
                .unfilled_style(theme.border())
                .render(line(1), buf);
        } else {
            Line::from(vec![
                Span::styled(format!("{} ", sdl_form::spinner(spin)), theme.running()),
                Span::styled(task.count.as_str(), theme.emphasis()),
            ])
            .render(line(1), buf);
        }
        let mut third = vec![Span::styled(task.estimate.as_str(), theme.secondary())];
        if let Some(attention) = &task.attention {
            third.push(Span::styled(format!("  {attention}"), theme.attention()));
        }
        Line::from(third).render(line(2), buf);
    }
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, theme: Theme) {
    let mut spans = Vec::new();
    for hint in keys::footer(app) {
        spans.push(Span::styled(hint.key, theme.emphasis()));
        spans.push(Span::styled(
            format!(" {}   ", hint.what),
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
    fn a_short_terminal_shows_only_rows_and_footer() {
        let places = areas(Rect::new(0, 0, 100, 11), 2);
        assert_eq!(places.running, None);
        assert_eq!(places.main.height, 10);
    }
}
