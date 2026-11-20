//! How a [`Form`] looks: a centred popup with one line per row, a tick list's entries under its
//! label, the summary line, and the hint line at the foot. Pure over the form.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::{Body, Form, Row, Summary};
use crate::popups::{centred, frame_popup, scrollbar, window_top};
use crate::theme::Theme;
use crate::ui::numbered;

/// Width of the popup, at most.
pub const WIDTH: u16 = 64;
/// Entries of a tick list shown at once, at most.
const WINDOW: usize = 8;
/// Width of the focus marker column.
const MARKER: u16 = 2;
/// Widest label column.
const LABEL_MAX: u16 = 22;

/// What every row is drawn with.
struct Ctx<'a> {
    theme: Theme,
    spinner: &'a Span<'static>,
}

/// One visible row with the room it takes.
struct Placed<'a> {
    row: &'a Row,
    height: u16,
}

/// Draw `form` centred in `area`, with `hints` under it. `spinner` is the busy glyph.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    form: &Form,
    hints: Line<'static>,
    spinner: &Span<'static>,
) {
    let width = WIDTH.min(area.width);
    let inner_width = width.saturating_sub(4);
    let label = label_width(form, inner_width);
    let value = inner_width.saturating_sub(MARKER).saturating_sub(label);
    let mut placed = place(form, value);
    let summary = summary_paragraph(&form.summary, theme, spinner);
    let summary_height = height_of(&summary, inner_width);

    // Rows, a gap, the summary and the hints, inside a border.
    let fixed = summary_height.saturating_add(2).saturating_add(2);
    let wanted = placed
        .iter()
        .fold(fixed, |sum, p| sum.saturating_add(p.height));
    shrink(&mut placed, wanted.saturating_sub(area.height));
    let total = placed
        .iter()
        .fold(fixed, |sum, p| sum.saturating_add(p.height));

    let rect = centred(area, width, total);
    let inner = frame_popup(frame, rect, theme, form.title());
    let ctx = Ctx { theme, spinner };
    let mut y = inner.y;
    for p in &placed {
        let rows = Rect::new(inner.x, y, inner.width, p.height).intersection(inner);
        draw_row(frame, rows, &ctx, form, p, label);
        y = y.saturating_add(p.height);
    }
    // The gap, then the summary, then the hints in the last line.
    y = y.saturating_add(1);
    let summary_rect = Rect::new(inner.x, y, inner.width, summary_height).intersection(inner);
    frame.render_widget(summary, summary_rect);
    let hint_rect =
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1).intersection(inner);
    frame.render_widget(Paragraph::new(hints), hint_rect);
}

fn label_width(form: &Form, inner_width: u16) -> u16 {
    let longest = form
        .rows
        .iter()
        .filter(|row| row.visible)
        .map(|row| u16::try_from(row.label.chars().count()).unwrap_or(LABEL_MAX))
        .max()
        .unwrap_or(0);
    longest
        .saturating_add(2)
        .min(LABEL_MAX)
        .min(inner_width / 2)
}

fn place(form: &Form, value_width: u16) -> Vec<Placed<'_>> {
    form.rows
        .iter()
        .filter(|row| row.visible)
        .map(|row| {
            let height = match &row.body {
                Body::Choice { options, .. } if options.is_empty() => {
                    let text = Paragraph::new(row.empty_text()).wrap(Wrap { trim: false });
                    height_of(&text, value_width)
                }
                Body::Ticks { items, .. } => {
                    let window = items.len().clamp(1, WINDOW);
                    1_u16.saturating_add(u16::try_from(window).unwrap_or(1))
                }
                _ => 1,
            };
            Placed { row, height }
        })
        .collect()
}

/// Take `excess` lines back from the tick lists, never below one entry each.
fn shrink(placed: &mut [Placed<'_>], mut excess: u16) {
    for p in placed.iter_mut() {
        if excess == 0 {
            break;
        }
        if matches!(p.row.body, Body::Ticks { .. }) {
            let take = excess.min(p.height.saturating_sub(2));
            p.height = p.height.saturating_sub(take);
            excess = excess.saturating_sub(take);
        }
    }
}

fn height_of(paragraph: &Paragraph<'_>, width: u16) -> u16 {
    u16::try_from(paragraph.line_count(width.max(1)))
        .unwrap_or(u16::MAX)
        .max(1)
}

fn summary_paragraph(
    summary: &Summary,
    theme: Theme,
    spinner: &Span<'static>,
) -> Paragraph<'static> {
    let line = match summary {
        Summary::None => Line::default(),
        Summary::Info(text) => Line::from(numbered(text, theme.body(), theme)),
        Summary::Busy(text) => Line::from(vec![
            spinner.clone(),
            Span::styled(text.clone(), theme.body()),
        ]),
        Summary::Warn(text) => Line::styled(text.clone(), theme.attention()),
    };
    Paragraph::new(line).wrap(Wrap { trim: false })
}

fn draw_row(
    frame: &mut Frame,
    rect: Rect,
    ctx: &Ctx<'_>,
    form: &Form,
    placed: &Placed<'_>,
    label_width: u16,
) {
    let theme = ctx.theme;
    let row = placed.row;
    let focused = form.focused() == Some(row.id);
    let [marker, label, value] = Layout::horizontal([
        Constraint::Length(MARKER),
        Constraint::Length(label_width),
        Constraint::Fill(1),
    ])
    .areas(rect);
    let label = Rect::new(label.x, label.y, label.width, label.height.min(1));
    if matches!(row.body, Body::Ticks { .. }) {
        let header =
            Rect::new(marker.x.saturating_add(MARKER), rect.y, rect.width, 1).intersection(rect);
        frame.render_widget(
            Paragraph::new(Span::styled(row.label.clone(), theme.body())),
            header,
        );
        let list = Rect::new(
            rect.x,
            rect.y.saturating_add(1),
            rect.width,
            rect.height.saturating_sub(1),
        )
        .intersection(rect);
        draw_ticks(frame, list, ctx, row, focused);
        return;
    }
    if focused {
        frame.render_widget(Block::new().style(theme.selected()), rect);
        frame.render_widget(Paragraph::new(Span::styled("▶", theme.emphasis())), marker);
    }
    frame.render_widget(
        Paragraph::new(Span::styled(row.label.clone(), theme.body())),
        label,
    );
    match &row.body {
        Body::Text(input) => {
            let scroll = input.visual_scroll(usize::from(value.width));
            let offset = u16::try_from(scroll).unwrap_or(0);
            frame.render_widget(
                Paragraph::new(input.value())
                    .style(theme.body())
                    .scroll((0, offset)),
                value,
            );
            if focused {
                let cursor =
                    u16::try_from(input.visual_cursor().saturating_sub(scroll)).unwrap_or(0);
                frame.set_cursor_position((
                    value
                        .x
                        .saturating_add(cursor)
                        .min(value.right().saturating_sub(1)),
                    value.y,
                ));
            }
        }
        Body::Choice { options, at, .. } => {
            if options.is_empty() {
                frame.render_widget(
                    Paragraph::new(Span::styled(row.empty_text(), theme.attention()))
                        .wrap(Wrap { trim: false }),
                    value,
                );
            } else {
                let arrow = if focused {
                    theme.emphasis()
                } else {
                    theme.secondary()
                };
                let shown = options.get(*at).cloned().unwrap_or_default();
                let line = if options.len() > 1 {
                    Line::from(vec![
                        Span::styled("‹ ", arrow),
                        Span::styled(shown, theme.body()),
                        Span::styled(" ›", arrow),
                    ])
                } else {
                    Line::styled(shown, theme.body())
                };
                frame.render_widget(Paragraph::new(line), value);
            }
        }
        Body::Toggle(on) => {
            let (mark, style) = if *on {
                ("[x]", theme.done())
            } else {
                ("[ ]", theme.secondary())
            };
            frame.render_widget(Paragraph::new(Span::styled(mark, style)), value);
        }
        Body::Ticks { .. } => {}
    }
}

/// The entries of a tick list, in the room under its label.
fn draw_ticks(frame: &mut Frame, area: Rect, ctx: &Ctx<'_>, row: &Row, focused: bool) {
    let theme = ctx.theme;
    let Body::Ticks {
        items,
        cursor,
        loading,
    } = &row.body
    else {
        return;
    };
    let indent = Rect::new(
        area.x.saturating_add(MARKER),
        area.y,
        area.width.saturating_sub(MARKER),
        area.height,
    );
    if *loading {
        let line = Line::from(vec![
            ctx.spinner.clone(),
            Span::styled("Looking…", theme.body()),
        ]);
        frame.render_widget(Paragraph::new(line), indent);
        return;
    }
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(row.empty_text(), theme.attention())),
            indent,
        );
        return;
    }
    let visible = usize::from(area.height);
    let scrolls = items.len() > visible;
    let [list, bar] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(u16::from(scrolls))])
            .areas(area);
    let top = window_top(items.len(), *cursor, visible, |_| 1);
    let shown = items.get(top..).unwrap_or_default();
    let detail_width = shown
        .iter()
        .take(visible)
        .filter_map(|item| item.detail.as_ref())
        .map(|detail| detail.chars().count())
        .max()
        .unwrap_or(0);
    let gap = if detail_width > 0 { 2 } else { 0 };
    // Marker, tick box and a space come first.
    let name_width = usize::from(list.width)
        .saturating_sub(usize::from(MARKER))
        .saturating_sub(4)
        .saturating_sub(detail_width)
        .saturating_sub(gap);
    for (offset, item) in shown.iter().take(visible).enumerate() {
        let at = top.saturating_add(offset);
        let here = focused && at == *cursor;
        let (mark, mark_style) = if item.on {
            ("[x] ", theme.done())
        } else {
            ("[ ] ", theme.secondary())
        };
        let name: String = item.label.chars().take(name_width).collect();
        let mut spans = vec![
            Span::styled(if here { "▶ " } else { "  " }, theme.emphasis()),
            Span::styled(mark, mark_style),
            Span::styled(format!("{name:<name_width$}"), theme.body()),
        ];
        if let Some(detail) = &item.detail {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{detail:>detail_width$}"),
                theme.emphasis(),
            ));
        }
        let mut line = Line::from(spans);
        if here {
            line = line.style(theme.selected());
        }
        let y = list.y.saturating_add(u16::try_from(offset).unwrap_or(0));
        frame.render_widget(Paragraph::new(line), Rect::new(list.x, y, list.width, 1));
    }
    if scrolls {
        scrollbar(frame, bar, theme, items.len(), *cursor);
    }
}
