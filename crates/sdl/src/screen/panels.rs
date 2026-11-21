//! The home screen (F1): one card per function, laid out for the terminal's size, and the footer.
//!
//! A card shows what its function will act on and, only where it helps, how its last run went.
//! While the function runs, that content gives way to its progress. What each card says is
//! decided in `app::rows`; this file only places and paints it.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, LineGauge, Paragraph, Widget, Wrap};
use sdl_core::TaskId;

use super::keys;
use super::theme::Theme;
use crate::app::rows::{CardView, RunView, Tone, cards};
use crate::app::{App, Hits, Home, MainTarget};

/// Columns between cards in the grid.
const GAP: u16 = 2;

/// Rows of a card in the grid: its border and five lines.
const CARD_ROWS: u16 = 7;

/// Rows of a card in the stack: its border and two lines.
const STACK_CARD_ROWS: u16 = 4;

/// The widest a grid card grows, so a wide terminal gets air around the grid, not long lines.
const CARD_MAX_COLS: u16 = 36;

/// The widest a stacked card grows.
const STACK_MAX_COLS: u16 = 60;

/// Draw the home screen and record what each rectangle means into `hits`.
pub fn draw(frame: &mut Frame, app: &App, theme: Theme, hits: &mut Hits) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    buf.set_style(area, theme.background());
    let [body, footer] = area.layout(&Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
    ]));
    let body = draw_tools_line(buf, body, app, theme);
    let cards = cards(app);
    match app.home() {
        Home::Grid => draw_grid(buf, body, &cards, app, theme, hits),
        Home::Stack => draw_stack(buf, body, &cards, app, theme, hits),
        Home::List => draw_list(buf, body, &cards, app, theme, hits),
    }
    draw_footer(buf, footer, app, theme, hits);
}

/// The line about missing tools, on the first row when there is something to say. Returns what
/// is left of `body`.
fn draw_tools_line(buf: &mut Buffer, body: Rect, app: &App, theme: Theme) -> Rect {
    let Some(text) = app.tools_banner() else {
        return body;
    };
    if body.height < 2 {
        return body;
    }
    clipped(
        Line::styled(format!(" ! {text}"), theme.attention()),
        Rect::new(body.x, body.y, body.width, 1),
        buf,
    );
    Rect::new(
        body.x,
        body.y.saturating_add(1),
        body.width,
        body.height.saturating_sub(1),
    )
}

/// Where a function's card sits in the grid: its column, its row, and how many rows it spans.
/// Bring music in on the left, check in the middle, take music out on the right.
const fn grid_cell(task: TaskId) -> (u16, u16, u16) {
    match task {
        TaskId::SyncSpotify => (0, 0, 1),
        TaskId::DownloadLink => (0, 1, 1),
        TaskId::Review => (1, 0, 2),
        TaskId::SyncDevice => (2, 0, 1),
        TaskId::Convert => (2, 1, 1),
    }
}

/// The offset that centres `inner` cells inside `outer`.
const fn centred(outer: u16, inner: u16) -> u16 {
    outer.saturating_sub(inner) / 2
}

fn draw_grid(
    buf: &mut Buffer,
    body: Rect,
    cards: &[CardView],
    app: &App,
    theme: Theme,
    hits: &mut Hits,
) {
    let gaps = GAP.saturating_mul(2);
    let wide = (body.width.saturating_sub(gaps) / 3).min(CARD_MAX_COLS);
    let grid_cols = wide.saturating_mul(3).saturating_add(gaps);
    let grid_rows = CARD_ROWS.saturating_mul(2);
    let left = body.x.saturating_add(centred(body.width, grid_cols));
    let top = body.y.saturating_add(centred(body.height, grid_rows));
    for card in cards {
        let (column, row, span) = grid_cell(card.task);
        let rect = Rect::new(
            left.saturating_add(column.saturating_mul(wide.saturating_add(GAP))),
            top.saturating_add(row.saturating_mul(CARD_ROWS)),
            wide,
            CARD_ROWS.saturating_mul(span),
        );
        draw_card(buf, rect, card, app, theme, hits);
    }
}

fn draw_stack(
    buf: &mut Buffer,
    body: Rect,
    cards: &[CardView],
    app: &App,
    theme: Theme,
    hits: &mut Hits,
) {
    let wide = body.width.min(STACK_MAX_COLS);
    let count = u16::try_from(cards.len()).unwrap_or(u16::MAX);
    let rows = STACK_CARD_ROWS.saturating_mul(count);
    let left = body.x.saturating_add(centred(body.width, wide));
    let top = body.y.saturating_add(centred(body.height, rows));
    for (index, card) in cards.iter().enumerate() {
        let down = u16::try_from(index)
            .unwrap_or(u16::MAX)
            .saturating_mul(STACK_CARD_ROWS);
        let rect = Rect::new(left, top.saturating_add(down), wide, STACK_CARD_ROWS);
        if rect.bottom() > body.bottom() {
            break;
        }
        draw_card(buf, rect, card, app, theme, hits);
    }
}

/// One line per function, for a terminal too short for cards: the name, then its count while it
/// runs (the thing that must not be cut off), else what it will act on.
fn draw_list(
    buf: &mut Buffer,
    body: Rect,
    cards: &[CardView],
    app: &App,
    theme: Theme,
    hits: &mut Hits,
) {
    for (index, card) in cards.iter().enumerate() {
        let y = body
            .y
            .saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
        if y >= body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, body.width, 1);
        let selected = card.task == app.selected;
        if selected {
            buf.set_style(rect, theme.selected());
        }
        let name = if selected {
            theme.heading()
        } else if card.run.is_some() {
            theme.running()
        } else {
            theme.body()
        };
        let mut spans = vec![
            Span::styled(if selected { "› " } else { "  " }, theme.emphasis()),
            Span::styled(card.name, name),
        ];
        match &card.run {
            Some(run) => {
                let count = if run.count.is_empty() {
                    "running"
                } else {
                    run.count.as_str()
                };
                spans.push(Span::styled(format!("  {count}"), theme.running()));
            }
            None => spans.push(Span::styled(
                format!("  {}", card.what.text),
                tone(theme, card.what.tone),
            )),
        }
        clipped(Line::from(spans), rect, buf);
        hits.main.push(rect, MainTarget::Row(card.task));
    }
}

fn tone(theme: Theme, tone: Tone) -> Style {
    match tone {
        Tone::Body => theme.body(),
        Tone::Secondary => theme.secondary(),
        Tone::Done => theme.done(),
        Tone::Attention => theme.attention(),
        Tone::Failed => theme.failed(),
        Tone::Skipped => theme.skipped(),
    }
}

/// One card. The border tells its state apart at a glance: heavy for the chosen one, double
/// for one that waits for the owner, coloured as running while it runs.
fn draw_card(
    buf: &mut Buffer,
    rect: Rect,
    card: &CardView,
    app: &App,
    theme: Theme,
    hits: &mut Hits,
) {
    let selected = card.task == app.selected;
    let running = card.run.is_some();
    let (border, kind) = if selected {
        (theme.border_focus(), BorderType::Thick)
    } else if card.needs_you {
        (theme.attention(), BorderType::Double)
    } else if running {
        (theme.running(), BorderType::Rounded)
    } else {
        (theme.border(), BorderType::Rounded)
    };
    let name = if running {
        theme.running()
    } else if selected {
        theme.heading()
    } else {
        theme.body()
    };
    let mut title = vec![Span::raw(" ")];
    if running {
        // The spinner moves while the function runs, so a long single song never looks hung.
        title.push(Span::styled(
            format!("{} ", sdl_form::spinner(app.spin)),
            theme.running(),
        ));
    }
    title.push(Span::styled(card.name, name.add_modifier(Modifier::BOLD)));
    title.push(Span::raw(" "));
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(kind)
        .border_style(border)
        .title(Line::from(title));
    let inside = block.inner(rect);
    block.render(rect, buf);
    hits.main.push(rect, MainTarget::Row(card.task));
    // One column of air inside the border.
    let inner = Rect::new(
        inside.x.saturating_add(1),
        inside.y,
        inside.width.saturating_sub(2),
        inside.height,
    );
    if inner.is_empty() {
        return;
    }
    match &card.run {
        Some(run) => draw_progress(buf, inner, run, selected, theme),
        None => draw_rest(buf, inner, card, selected, theme),
    }
}

/// The `n`th row of `area`.
const fn row(area: Rect, n: u16) -> Rect {
    Rect::new(area.x, area.y.saturating_add(n), area.width, 1)
}

/// A card at rest: what it will act on, a note where there is room, its last result where that
/// helps, and what Enter does on the last row.
fn draw_rest(buf: &mut Buffer, inner: Rect, card: &CardView, selected: bool, theme: Theme) {
    let last = inner.height.saturating_sub(1);
    let action = card.action.map(|verb| {
        if selected {
            Line::from(vec![
                Span::styled("Enter ", theme.emphasis()),
                Span::styled(verb, theme.body()),
            ])
        } else {
            Line::styled(verb, theme.secondary())
        }
    });
    if inner.height <= 2 {
        clipped(
            Line::styled(card.what.text.as_str(), tone(theme, card.what.tone)),
            row(inner, 0),
            buf,
        );
        // The stack has one more row: the result when there is one, else the action.
        if last > 0 {
            match (&card.result, action) {
                (Some(result), _) => clipped(
                    Line::styled(result.text.as_str(), tone(theme, result.tone)),
                    row(inner, last),
                    buf,
                ),
                (None, Some(action)) => clipped(action, row(inner, last), buf),
                (None, None) => {}
            }
        }
        return;
    }
    // What it will act on wraps onto a second row before it is ever cut; the rows between it
    // and the last hold the note, then the result, wrapped.
    let what = Paragraph::new(card.what.text.as_str())
        .style(tone(theme, card.what.tone))
        .wrap(Wrap { trim: true });
    let what_rows = u16::try_from(what.line_count(inner.width))
        .unwrap_or(1)
        .clamp(1, 2)
        .min(last.max(1));
    what.render(Rect::new(inner.x, inner.y, inner.width, what_rows), buf);
    let mut y = what_rows;
    let free = |y: u16| last.saturating_sub(y);
    if let Some(note) = card.note
        && inner.height > CARD_ROWS
    {
        let rows = free(y).min(3);
        Paragraph::new(note)
            .style(theme.secondary())
            .wrap(Wrap { trim: true })
            .render(
                Rect::new(inner.x, inner.y.saturating_add(y), inner.width, rows),
                buf,
            );
        y = y.saturating_add(rows).saturating_add(1);
    }
    if let Some(result) = &card.result {
        let rows = free(y).min(2);
        Paragraph::new(result.text.as_str())
            .style(tone(theme, result.tone))
            .wrap(Wrap { trim: true })
            .render(
                Rect::new(inner.x, inner.y.saturating_add(y), inner.width, rows),
                buf,
            );
    }
    if let Some(action) = action {
        clipped(action, row(inner, last), buf);
    }
}

/// A running card: its progress takes the place of everything else.
fn draw_progress(buf: &mut Buffer, inner: Rect, run: &RunView, selected: bool, theme: Theme) {
    let bar = |area: Rect, buf: &mut Buffer| {
        if let Some(percent) = run.percent {
            LineGauge::default()
                .ratio(f64::from(percent) / 100.0)
                .label(Line::styled(run.count.as_str(), theme.emphasis()))
                .filled_style(theme.bar())
                .unfilled_style(theme.border())
                .render(area, buf);
        } else {
            Line::styled(run.count.as_str(), theme.emphasis()).render(area, buf);
        }
    };
    let headline = Line::styled(fit(&run.headline, inner.width), theme.secondary());
    // What needs attention comes first: on a narrow card it is the part that must not be cut.
    let mut estimate = Vec::new();
    if let Some(attention) = &run.attention {
        estimate.push(Span::styled(attention.as_str(), theme.attention()));
        estimate.push(Span::styled(" · ", theme.secondary()));
    }
    estimate.push(Span::styled(run.estimate.as_str(), theme.secondary()));
    if inner.height <= 2 {
        bar(row(inner, 0), buf);
        if inner.height == 2 {
            // The stack has one more row: what needs attention when something does, else what
            // it is doing now.
            if run.attention.is_some() {
                clipped(Line::from(estimate), row(inner, 1), buf);
            } else {
                headline.render(row(inner, 1), buf);
            }
        }
        return;
    }
    headline.render(row(inner, 0), buf);
    bar(row(inner, 1), buf);
    clipped(Line::from(estimate), row(inner, 2), buf);
    if selected && inner.height > 3 {
        Line::from(vec![
            Span::styled("s ", theme.emphasis()),
            Span::styled("stop", theme.body()),
        ])
        .render(row(inner, inner.height.saturating_sub(1)), buf);
    }
}

/// Draw `line` in the one-row `area`; when it does not fit, its last cell shows "…", so a cut
/// line never looks complete.
fn clipped(line: Line<'_>, area: Rect, buf: &mut Buffer) {
    let too_long = line.width() > usize::from(area.width);
    line.render(area, buf);
    if too_long && let Some(cell) = buf.cell_mut((area.right().saturating_sub(1), area.y)) {
        cell.set_symbol("…");
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

/// The footer: the keys that work now. Each hint is recorded where it was drawn, so a click on
/// it presses its key.
fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, theme: Theme, hits: &mut Hits) {
    let mut spans = Vec::new();
    let mut x = area.x;
    for hint in keys::fit(&keys::footer(app), area.width) {
        let words = format!(" {}", hint.what);
        let wide = Line::raw(format!("{}{words}", hint.key)).width();
        let wide = u16::try_from(wide).unwrap_or(u16::MAX);
        if let Some(code) = keys::key_of(&hint) {
            let rect = Rect::new(x, area.y, wide.min(area.right().saturating_sub(x)), 1);
            hits.footer.push(rect, code);
        }
        let gap = u16::try_from(keys::GAP).unwrap_or(u16::MAX);
        x = x.saturating_add(wide).saturating_add(gap);
        spans.push(Span::styled(hint.key, theme.emphasis()));
        spans.push(Span::styled(
            format!("{words}{}", " ".repeat(keys::GAP)),
            theme.secondary(),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Size;

    use super::*;
    use crate::app::{Clock, Settings};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn drawn(width: u16, height: u16) -> Result<Hits, Box<dyn std::error::Error>> {
        let app = App::new(
            Settings::default(),
            false,
            Size::new(width, height),
            Clock {
                instant: Instant::now(),
                epoch: 10_000,
            },
        );
        let mut hits = Hits::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| draw(frame, &app, Theme::new(true), &mut hits))?;
        Ok(hits)
    }

    fn card(hits: &Hits, task: TaskId) -> Result<Rect, &'static str> {
        hits.main
            .area_of(&MainTarget::Row(task))
            .ok_or("a function has no card")
    }

    #[test]
    fn the_grid_is_centred_with_review_between_bringing_in_and_taking_out() -> TestResult {
        let hits = drawn(80, 24)?;
        let spotify = card(&hits, TaskId::SyncSpotify)?;
        let link = card(&hits, TaskId::DownloadLink)?;
        let review = card(&hits, TaskId::Review)?;
        let device = card(&hits, TaskId::SyncDevice)?;
        let convert = card(&hits, TaskId::Convert)?;
        // Three columns, the same space left and right, and above and below.
        assert!(spotify.x.abs_diff(80_u16.saturating_sub(convert.right())) <= 1);
        assert!(spotify.y.abs_diff(23_u16.saturating_sub(link.bottom())) <= 1);
        assert!(spotify.right() < review.x && review.right() < device.x);
        assert_eq!((link.x, link.y), (spotify.x, spotify.bottom()));
        assert_eq!((convert.x, convert.y), (device.x, device.bottom()));
        // Review spans both rows.
        assert_eq!((review.y, review.bottom()), (spotify.y, link.bottom()));
        Ok(())
    }

    #[test]
    fn a_wide_terminal_keeps_cards_narrow_and_a_narrow_one_stacks_them() -> TestResult {
        let wide = drawn(160, 40)?;
        assert_eq!(card(&wide, TaskId::SyncSpotify)?.width, CARD_MAX_COLS);
        let narrow = drawn(50, 24)?;
        let first = card(&narrow, TaskId::SyncSpotify)?;
        let second = card(&narrow, TaskId::DownloadLink)?;
        assert_eq!((second.x, second.y), (first.x, first.bottom()));
        assert_eq!(first.height, STACK_CARD_ROWS);
        Ok(())
    }

    #[test]
    fn a_short_terminal_gives_one_line_to_each_function() -> TestResult {
        let hits = drawn(80, 11)?;
        for (index, task) in TaskId::ALL.into_iter().enumerate() {
            let line = card(&hits, task)?;
            assert_eq!(line.height, 1);
            assert_eq!(usize::from(line.y), index);
        }
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
}
