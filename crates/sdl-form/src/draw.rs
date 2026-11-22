//! Drawing: a pure function of the form's state. It returns the rectangles it drew.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text as Lines};
use ratatui::widgets::{Paragraph, StatefulWidget, Widget, Wrap};
use tui_scrollbar::{ScrollBar, ScrollLengths};
use tui_scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};

use crate::field::{Choice, Field, Folder, Kind, Pick, Rows, Text};
use crate::form::{Focus, Form, scrollbar};
use crate::{DROPDOWN_ROWS, HitMap, LIST_ROWS, Target};

/// The styles a form is drawn in; the screen builds them from its theme. The default is plain.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormStyle {
    /// Title and field labels.
    pub label: Style,
    /// Body text and option labels.
    pub body: Style,
    /// Placeholders, option details, notes, counts and the summary.
    pub secondary: Style,
    /// The focus marker and the highlighted row's marker.
    pub emphasis: Style,
    /// The focused field's label.
    pub focus: Style,
    /// The button with the focus (Continue, Cancel, Browse): filled, so it cannot be missed.
    pub button: Style,
    /// The highlighted row's background.
    pub selected: Style,
    /// Flagged parts of an option's label.
    pub flagged: Style,
    /// The message under a field whose value is not acceptable, and what is missing.
    pub error: Style,
}

/// Width the form asks for, in columns, before the popup clamps it.
const WIDTH: u16 = 64;

/// Rows of the folder browser's list.
const BROWSER_ROWS: u16 = 10;

/// The size the form wants: its width (at most `max_width`) and the height of everything it
/// draws. A popup of this size shows the whole form; [`render`] into a shorter area scrolls.
#[must_use]
pub fn size(form: &Form, max_width: u16) -> Size {
    let width = WIDTH.min(max_width);
    let height = if form.explorer().is_some() {
        BROWSER_ROWS.saturating_add(3)
    } else {
        body_height(form, width).saturating_add(foot_height(form, width))
    };
    Size::new(width, height)
}

/// The notes, wrapped to the width they are drawn at.
fn notes<'a>(form: &'a Form, style: &FormStyle) -> Paragraph<'a> {
    let lines: Vec<Line<'a>> = form
        .notes()
        .iter()
        .map(|note| Line::styled(note.as_str(), style.secondary))
        .collect();
    Paragraph::new(Lines::from(lines)).wrap(Wrap { trim: true })
}

/// `text` wrapped at the width it is drawn at (F1: a field's message, the summary and what is
/// missing wrap and are never cut off).
fn wrapped<'a>(text: impl Into<Line<'a>>) -> Paragraph<'a> {
    Paragraph::new(text.into()).wrap(Wrap { trim: true })
}

/// Rows `text` takes wrapped at `width` (at least one).
fn wrapped_rows(text: &str, width: u16) -> u16 {
    u16::try_from(wrapped(text).line_count(width.max(1)))
        .unwrap_or(u16::MAX)
        .max(1)
}

/// The message under a text or folder field, if its value is not acceptable.
fn complaint(field: &Field) -> Option<String> {
    match &field.kind {
        Kind::Text(text) | Kind::Folder(Folder { text, .. }) => text.complaint(),
        _ => None,
    }
}

/// The summary or busy line as drawn: the spinner frame first while busy.
fn status_line<'a>(form: &'a Form, style: &FormStyle) -> Line<'a> {
    match (form.busy(), form.summary()) {
        (Some(busy), _) => Line::from(vec![
            Span::styled(format!("{} ", crate::spinner(form.spin())), style.emphasis),
            Span::styled(busy, style.secondary),
        ]),
        (None, Some(summary)) => Line::styled(summary, style.body),
        (None, None) => Line::default(),
    }
}

/// Rows of the scrolling part: the notes wrapped at `width`, then the visible fields.
fn body_height(form: &Form, width: u16) -> u16 {
    let notes = if form.notes().is_empty() {
        0
    } else {
        u16::try_from(notes(form, &FormStyle::default()).line_count(width)).unwrap_or(u16::MAX)
    };
    form.fields()
        .iter()
        .filter(|field| field.visible)
        .map(|field| field_height(field, width))
        .fold(notes, u16::saturating_add)
}

/// Rows of the part that never scrolls at `width`: the summary (at least one row, kept so the
/// form does not jump when a summary arrives), what is missing, each wrapped, and the buttons.
fn foot_height(form: &Form, width: u16) -> u16 {
    let status =
        u16::try_from(wrapped(status_line(form, &FormStyle::default())).line_count(width.max(1)))
            .unwrap_or(u16::MAX)
            .max(1);
    form.missing()
        .iter()
        .map(|missing| wrapped_rows(&format!("· {missing}"), width))
        .fold(status, u16::saturating_add)
        .saturating_add(1)
}

/// Rows a field takes at `width`: its label row, then its body (a text's message wrapped).
fn field_height(field: &Field, width: u16) -> u16 {
    let message = complaint(field).map_or(0, |text| wrapped_rows(&text, width.saturating_sub(2)));
    let body = match &field.kind {
        Kind::Text(_) | Kind::Folder(_) => 1_u16.saturating_add(message),
        Kind::Dropdown(pick) if pick.open => open_rows(pick),
        Kind::Toggle(_) | Kind::Dropdown(_) => 0,
        Kind::Radio(pick) => radio_rows(pick, width.saturating_sub(2)),
        Kind::Ticks(_) | Kind::List(_) => LIST_ROWS.saturating_add(2),
    };
    1_u16.saturating_add(body)
}

/// Columns of a radio option's lead (`(•) `); its label wraps beside it.
const RADIO_LEAD: u16 = 4;

/// Rows a radio group takes at `width`: each option's label wrapped beside its lead, so a long
/// candidate is read whole, never cut (one row when there are no options).
fn radio_rows(pick: &Pick, width: u16) -> u16 {
    if pick.options.is_empty() {
        return 1;
    }
    pick.options
        .iter()
        .map(|option| option_rows(option, width))
        .fold(0, u16::saturating_add)
}

/// Rows one radio option's label takes wrapped at `width` (its lead included).
fn option_rows(option: &Choice, width: u16) -> u16 {
    let label = width.saturating_sub(RADIO_LEAD).max(1);
    u16::try_from(wrapped(option_line(option, "", &FormStyle::default())).line_count(label))
        .unwrap_or(u16::MAX)
        .max(1)
}

fn open_rows(pick: &Pick) -> u16 {
    u16::try_from(pick.options.len().max(1))
        .unwrap_or(u16::MAX)
        .min(DROPDOWN_ROWS)
}

/// Draw `form` into `area` of `buf`, and return what was drawn where.
///
/// The summary, what is missing and the buttons are always drawn at the bottom. When `area` is
/// shorter than [`size`] asks for, the notes and fields above them scroll (`tui-scrollview`, with
/// a `tui-scrollbar`) so that the focused field is in view; the wheel and Tab move the focus, and
/// the view follows it.
#[must_use]
pub fn render(form: &Form, area: Rect, buf: &mut Buffer, style: &FormStyle) -> HitMap<Target> {
    let mut hits = HitMap::new();
    if area.is_empty() {
        return hits;
    }
    if let Some(browser) = form.explorer() {
        draw_browser(browser, area, buf, style, &mut hits);
        return hits;
    }
    let foot_rows = foot_height(form, area.width).min(area.height);
    let room = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(foot_rows),
    );
    let foot = Rect::new(area.x, room.bottom(), area.width, foot_rows);
    if body_height(form, room.width) <= room.height {
        draw_body(form, room, buf, style, &mut hits);
    } else if !room.is_empty() {
        let width = room.width.saturating_sub(1);
        let full = body_height(form, width);
        let mut view = ScrollView::new(Size::new(width, full))
            .scrollbars_visibility(ScrollbarVisibility::Never);
        let mut inner = HitMap::new();
        draw_body(
            form,
            Rect::new(0, 0, width, full),
            view.buf_mut(),
            style,
            &mut inner,
        );
        let offset = focus_offset(form, &inner, room.height);
        let mut state = ScrollViewState::with_offset(Position::new(0, offset));
        view.render(
            Rect::new(room.x, room.y, width, room.height),
            buf,
            &mut state,
        );
        let bar = ScrollBar::vertical(ScrollLengths {
            content_len: usize::from(full),
            viewport_len: usize::from(room.height),
        })
        .offset(usize::from(offset));
        (&bar).render(
            Rect::new(room.x.saturating_add(width), room.y, 1, room.height),
            buf,
        );
        hits.append_scrolled(inner, offset, Rect::new(room.x, room.y, width, room.height));
    }
    draw_foot(form, foot, buf, style, &mut hits);
    hits
}

/// The first content row shown so that the focused field is in view (its top, when it is taller
/// than the view). With the focus on a button, the top.
fn focus_offset(form: &Form, hits: &HitMap<Target>, rows: u16) -> u16 {
    let target = match form.focus() {
        Focus::Field(key) => Target::Field(key),
        Focus::Browse(key) => Target::Browse(key),
        Focus::Primary | Focus::Cancel => return 0,
    };
    hits.area_of(&target)
        .map_or(0, |rect| rect.bottom().saturating_sub(rows).min(rect.y))
}

/// The notes, then the visible fields, from the top of `area`.
fn draw_body(
    form: &Form,
    area: Rect,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    let mut y = area.y;
    let bottom = area.bottom();
    if !form.notes().is_empty() {
        let paragraph = notes(form, style);
        let rows = u16::try_from(paragraph.line_count(area.width))
            .unwrap_or(u16::MAX)
            .min(bottom.saturating_sub(y));
        paragraph.render(Rect::new(area.x, y, area.width, rows), buf);
        y = y.saturating_add(rows);
    }
    for field in form.fields().iter().filter(|field| field.visible) {
        let height = field_height(field, area.width).min(bottom.saturating_sub(y));
        let whole = Rect::new(area.x, y, area.width, height);
        hits.push(whole, Target::Field(field.key));
        let focused = form.focus() == &Focus::Field(field.key);
        draw_field(field, whole, focused, form.focus(), buf, style, hits);
        y = y.saturating_add(height);
    }
}

/// The summary (or the busy line), what is missing, and the buttons, from the top of `area`.
fn draw_foot(
    form: &Form,
    area: Rect,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    // The buttons keep the last row; the wrapped lines above take what is left.
    let buttons = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    let mut y = area.y;
    let bottom = buttons.y;
    let mut put = |paragraph: Paragraph<'_>, buf: &mut Buffer| {
        let rows = u16::try_from(paragraph.line_count(area.width.max(1)))
            .unwrap_or(u16::MAX)
            .max(1)
            .min(bottom.saturating_sub(y));
        if rows > 0 {
            paragraph.render(Rect::new(area.x, y, area.width, rows), buf);
            y = y.saturating_add(rows);
        }
    };
    put(wrapped(status_line(form, style)), buf);
    for missing in form.missing() {
        put(
            wrapped(Line::styled(format!("· {missing}"), style.error)),
            buf,
        );
    }
    if !area.is_empty() {
        draw_buttons(form, buttons, buf, style, hits);
    }
}

fn marker(focused: bool, style: &FormStyle) -> Span<'static> {
    if focused {
        Span::styled("› ", style.emphasis)
    } else {
        Span::raw("  ")
    }
}

fn draw_field(
    field: &Field,
    area: Rect,
    focused: bool,
    focus: &Focus,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    let label_style = if focused { style.focus } else { style.label };
    let label_row = Rect::new(area.x, area.y, area.width, 1);
    let mut label = vec![
        marker(focused, style),
        Span::styled(field.label.as_str(), label_style),
    ];
    if let Kind::Toggle(on) = &field.kind {
        label.push(Span::styled(
            if *on { "  [on]" } else { "  [off]" },
            style.emphasis,
        ));
    }
    if let Kind::Dropdown(pick) = &field.kind {
        let shown = pick
            .chosen
            .as_ref()
            .and_then(|id| pick.options.iter().find(|option| &option.id == id))
            .map_or_else(|| "choose…".to_owned(), Choice::label);
        label.push(Span::styled(format!("  {shown} ▾"), style.body));
    }
    Line::from(label).render(label_row, buf);
    let body = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(1),
    );
    if let Some(message) = complaint(field) {
        let below = Rect::new(
            body.x,
            body.y.saturating_add(1),
            body.width,
            body.height.saturating_sub(1),
        );
        if !below.is_empty() {
            wrapped(Line::styled(message, style.error)).render(below, buf);
        }
    }
    match &field.kind {
        Kind::Text(text) => draw_text(text, body, focused, buf, style),
        Kind::Folder(folder) => {
            let browse_width = 10.min(body.width);
            let text = Rect::new(
                body.x,
                body.y,
                body.width.saturating_sub(browse_width.saturating_add(1)),
                body.height,
            );
            draw_text(&folder.text, text, focused, buf, style);
            let browse = Rect::new(
                body.right().saturating_sub(browse_width),
                body.y,
                browse_width,
                1,
            );
            let at = focus == &Focus::Browse(field.key);
            Line::styled(" Browse… ", if at { style.button } else { style.emphasis })
                .render(browse, buf);
            hits.push(browse, Target::Browse(field.key));
        }
        Kind::Dropdown(pick) if pick.open => {
            draw_dropdown(pick, body, field.key, buf, style, hits);
        }
        Kind::Toggle(_) | Kind::Dropdown(_) => {}
        Kind::Radio(pick) => draw_radio(pick, body, field.key, buf, style, hits),
        Kind::Ticks(rows) => draw_rows(rows, body, field.key, true, buf, style, hits),
        Kind::List(rows) => draw_rows(rows, body, field.key, false, buf, style, hits),
    }
}

fn draw_text(text: &Text, area: Rect, focused: bool, buf: &mut Buffer, style: &FormStyle) {
    if area.is_empty() {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);
    let value = text.input.value();
    if value.is_empty() {
        Line::styled(text.placeholder.as_str(), style.secondary).render(row, buf);
    } else {
        let width = usize::from(row.width.saturating_sub(1));
        let scroll = text.input.visual_scroll(width);
        let shown: String = value.chars().skip(scroll).collect();
        Line::styled(shown, style.body).render(row, buf);
    }
    if focused {
        let width = usize::from(row.width.saturating_sub(1));
        let at = text
            .input
            .visual_cursor()
            .saturating_sub(text.input.visual_scroll(width));
        let column = row.x.saturating_add(u16::try_from(at).unwrap_or(u16::MAX));
        if let Some(cell) = buf
            .cell_mut((column, row.y))
            .filter(|_| column < row.right())
        {
            cell.set_style(Style::new().reversed());
        }
    }
}

fn option_line<'a>(option: &'a Choice, lead: &'a str, style: &FormStyle) -> Line<'a> {
    let mut spans = vec![Span::styled(lead, style.emphasis)];
    for (index, part) in option.parts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", style.secondary));
        }
        spans.push(Span::styled(
            part.text.as_str(),
            if part.flagged {
                style.flagged
            } else {
                style.body
            },
        ));
    }
    if !option.detail.is_empty() {
        spans.push(Span::styled(
            format!("  {}", option.detail),
            style.secondary,
        ));
    }
    Line::from(spans)
}

/// An open dropdown's list: at most [`DROPDOWN_ROWS`] rows, scrolled so the highlight shows.
fn draw_dropdown(
    pick: &Pick,
    area: Rect,
    key: crate::Key,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    if pick.options.is_empty() {
        Line::styled(pick.empty.as_str(), style.secondary).render(area, buf);
        return;
    }
    let rows = open_rows(pick);
    let first = pick
        .cursor
        .saturating_add(1)
        .saturating_sub(usize::from(rows));
    for (slot, (index, option)) in pick
        .options
        .iter()
        .enumerate()
        .skip(first)
        .take(usize::from(rows))
        .enumerate()
    {
        let y = area
            .y
            .saturating_add(u16::try_from(slot).unwrap_or(u16::MAX));
        let row = Rect::new(area.x, y, area.width, 1);
        if row.bottom() > area.bottom() {
            break;
        }
        let on = pick.cursor == index;
        if on {
            buf.set_style(row, style.selected);
        }
        let lead = if on { "› " } else { "  " };
        fit_line(option_line(option, lead, style), row.width, style).render(row, buf);
        hits.push(row, Target::Row { field: key, index });
    }
}

/// A radio group: every option shown, its label wrapped beside its lead.
fn draw_radio(
    pick: &Pick,
    area: Rect,
    key: crate::Key,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    if pick.options.is_empty() {
        Line::styled(pick.empty.as_str(), style.secondary).render(area, buf);
        return;
    }
    let mut y = area.y;
    for (index, option) in pick.options.iter().enumerate() {
        let rows = option_rows(option, area.width).min(area.bottom().saturating_sub(y));
        if rows == 0 {
            break;
        }
        let row = Rect::new(area.x, y, area.width, rows);
        let on = pick.chosen.as_ref() == Some(&option.id);
        if on {
            buf.set_style(row, style.selected);
        }
        let lead = if on { "(•) " } else { "( ) " };
        Span::styled(lead, style.emphasis).render(row, buf);
        let label = Rect::new(
            row.x.saturating_add(RADIO_LEAD),
            row.y,
            row.width.saturating_sub(RADIO_LEAD),
            rows,
        );
        wrapped(option_line(option, "", style)).render(label, buf);
        hits.push(row, Target::Row { field: key, index });
        y = y.saturating_add(rows);
    }
}

/// A cut row keeps its detail whole while the label keeps at least this many columns beside it.
const LABEL_MIN: usize = 12;

/// `line` as it fits in `width` columns on one row. When it is too long the label is cut with
/// "…" and the detail after it (a held song's choice, a list's song count) is kept whole, so
/// what was chosen is never the part that is lost.
fn fit_line<'a>(line: Line<'a>, width: u16, style: &FormStyle) -> Line<'a> {
    let width = usize::from(width);
    if line.width() <= width {
        return line;
    }
    let mut spans = line.spans;
    let detail = spans
        .last()
        .filter(|span| {
            span.content.starts_with("  ") && span.width().saturating_add(LABEL_MIN) <= width
        })
        .cloned();
    if detail.is_some() {
        spans.pop();
    }
    let room = width
        .saturating_sub(detail.as_ref().map_or(0, Span::width))
        .saturating_sub(1);
    let mut kept: Vec<Span<'a>> = Vec::new();
    let mut used = 0_usize;
    for span in spans {
        if used.saturating_add(span.width()) <= room {
            used = used.saturating_add(span.width());
            kept.push(span);
            continue;
        }
        let mut text = String::new();
        for c in span.content.chars() {
            let wide = Span::raw(c.to_string()).width();
            if used.saturating_add(wide) > room {
                break;
            }
            used = used.saturating_add(wide);
            text.push(c);
        }
        kept.push(Span::styled(text, span.style));
        break;
    }
    kept.push(Span::styled("…", style.secondary));
    kept.extend(detail);
    Line::from(kept)
}

fn draw_rows(
    rows: &Rows,
    area: Rect,
    key: crate::Key,
    ticks: bool,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    let filter = Rect::new(area.x, area.y, area.width, 1);
    let typed = rows.filter.value();
    let shown = if typed.is_empty() {
        "type to filter"
    } else {
        typed
    };
    Line::from(vec![
        Span::styled("⌕ ", style.emphasis),
        Span::styled(
            shown,
            if typed.is_empty() {
                style.secondary
            } else {
                style.body
            },
        ),
    ])
    .render(filter, buf);
    hits.push(filter, Target::Filter(key));
    let listed = rows.listed();
    let list = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width.saturating_sub(1),
        LIST_ROWS,
    );
    if listed.is_empty() {
        let text = if rows.options.is_empty() {
            rows.empty.as_str()
        } else {
            "Nothing matches"
        };
        wrapped(Line::styled(text, style.secondary)).render(list, buf);
    }
    for (slot, at) in listed
        .iter()
        .skip(rows.offset)
        .take(usize::from(LIST_ROWS))
        .enumerate()
    {
        let Some(option) = rows.options.get(*at) else {
            continue;
        };
        let index = rows.offset.saturating_add(slot);
        let y = list
            .y
            .saturating_add(u16::try_from(slot).unwrap_or(u16::MAX));
        let row = Rect::new(list.x, y, list.width, 1);
        if row.bottom() > area.bottom() {
            break;
        }
        if rows.cursor == index {
            buf.set_style(row, style.selected);
        }
        let lead = match (ticks, rows.ticked.contains(&option.id)) {
            (true, true) => "[x] ",
            (true, false) => "[ ] ",
            (false, _) => "",
        };
        fit_line(option_line(option, lead, style), row.width, style).render(row, buf);
        hits.push(row, Target::Row { field: key, index });
    }
    let bar = Rect::new(
        list.right(),
        list.y,
        1,
        LIST_ROWS.min(area.height.saturating_sub(1)),
    );
    if listed.len() > usize::from(LIST_ROWS) {
        (&scrollbar(rows)).render(bar, buf);
        hits.push(bar, Target::Scrollbar(key));
    }
    let count = Rect::new(area.x, list.y.saturating_add(LIST_ROWS), area.width, 1);
    let text = if ticks {
        format!("{} of {} ticked", rows.ticked_count(), rows.options.len())
    } else if typed.is_empty() {
        format!("{} in the list", rows.options.len())
    } else {
        format!("{} of {} shown", listed.len(), rows.options.len())
    };
    if count.bottom() <= area.bottom() {
        Line::styled(text, style.secondary).render(count, buf);
    }
}

fn draw_buttons(
    form: &Form,
    row: Rect,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    let primary = format!("[ {} ]", form.primary());
    let cancel = format!("[ {} ]", form.cancel());
    let primary_width = u16::try_from(Line::raw(primary.as_str()).width()).unwrap_or(u16::MAX);
    let cancel_width = u16::try_from(Line::raw(cancel.as_str()).width()).unwrap_or(u16::MAX);
    let first = Rect::new(row.x, row.y, primary_width.min(row.width), 1);
    let second = Rect::new(
        first.right().saturating_add(2),
        row.y,
        cancel_width.min(row.right().saturating_sub(first.right().saturating_add(2))),
        1,
    );
    let look = |focused: bool, enabled: bool| {
        if focused {
            style.button
        } else if enabled {
            style.body
        } else {
            style.secondary
        }
    };
    Line::styled(
        primary,
        look(form.focus() == &Focus::Primary, form.can_submit()),
    )
    .render(first, buf);
    Line::styled(cancel, look(form.focus() == &Focus::Cancel, true)).render(second, buf);
    hits.push(first, Target::Primary);
    hits.push(second, Target::Cancel);
}

fn draw_browser(
    browser: &ratatui_explorer::FileExplorer,
    area: Rect,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
) {
    let head = Rect::new(area.x, area.y, area.width, 1);
    // The end of the path names the folder shown; a path too long for the row loses its start
    // (a right-aligned line that does not fit is cut on the left).
    let path = Line::styled(browser.cwd().display().to_string(), style.label);
    if path.width() > usize::from(head.width) {
        path.right_aligned().render(head, buf);
    } else {
        path.render(head, buf);
    }
    let files = browser.files();
    let rows = usize::from(BROWSER_ROWS.min(area.height.saturating_sub(3)).max(1));
    let first = browser
        .selected_idx()
        .saturating_add(1)
        .saturating_sub(rows);
    for (slot, (index, file)) in files.iter().enumerate().skip(first).take(rows).enumerate() {
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(slot).unwrap_or(u16::MAX));
        let row = Rect::new(area.x, y, area.width, 1);
        if row.bottom() > area.bottom() {
            break;
        }
        if index == browser.selected_idx() {
            buf.set_style(row, style.selected);
        }
        let look = if file.is_dir {
            style.emphasis
        } else {
            style.secondary
        };
        Line::styled(file.name.as_str(), look).render(row, buf);
        hits.push(row, Target::BrowserRow(index));
    }
    let y = area
        .y
        .saturating_add(BROWSER_ROWS)
        .saturating_add(2)
        .min(area.bottom().saturating_sub(1));
    let choose = Rect::new(area.x, y, 22.min(area.width), 1);
    let back = Rect::new(choose.right().saturating_add(2), y, 8, 1);
    Line::styled("[ Choose this folder ]", style.focus).render(choose, buf);
    Line::styled("[ Back ]", style.body).render(back, buf);
    hits.push(choose, Target::BrowserChoose);
    hits.push(back, Target::BrowserBack);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Choice;

    fn text_of(buf: &Buffer) -> String {
        buf.content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn the_size_is_what_render_uses() {
        let form = Form::new("F")
            .field(Field::text("link", "Link").needs("Paste a link"))
            .field(Field::ticks("lists", "Lists").options(vec![Choice::new("a", "Alpha")]));
        let wanted = size(&form, 100);
        let area = Rect::new(0, 0, wanted.width, wanted.height);
        let mut buf = Buffer::empty(area);
        let hits = render(&form, area, &mut buf, &FormStyle::default());
        let drawn = text_of(&buf);
        assert!(drawn.contains("Paste a link"));
        assert!(drawn.contains("0 of 1 ticked"));
        assert!(drawn.contains("Continue"));
        assert!(hits.at(2, 0).is_some());
        assert!(matches!(
            hits.area_of(&Target::Primary),
            Some(rect) if rect.bottom() == area.bottom()
        ));
    }

    #[test]
    fn notes_wrap_and_are_counted_in_the_height() {
        let mut form = Form::new("F");
        form.set_notes(vec![
            "one two three four five six seven eight nine ten".into(),
        ]);
        let narrow = size(&form, 20);
        let wide = size(&form, 64);
        assert!(narrow.height > wide.height);
        let area = Rect::new(0, 0, narrow.width, narrow.height);
        let mut buf = Buffer::empty(area);
        let _ = render(&form, area, &mut buf, &FormStyle::default());
        assert!(text_of(&buf).contains("ten"));
    }

    #[test]
    fn a_form_taller_than_its_area_keeps_the_focus_and_the_buttons_in_view() {
        let mut form = Form::new("F")
            .field(Field::ticks("a", "First list").options(vec![Choice::new("x", "X")]))
            .field(Field::ticks("b", "Second list").options(vec![Choice::new("y", "Y")]))
            .field(Field::text("c", "Last field").needs("Fill the last field"));
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        let hits = render(&form, area, &mut buf, &FormStyle::default());
        let drawn = text_of(&buf);
        assert!(drawn.contains("First list") && !drawn.contains("Last field"));
        assert!(drawn.contains("Fill the last field") && drawn.contains("Continue"));
        assert!(hits.area_of(&Target::Field("c")).is_none());
        form.set_focus(Focus::Field("c"));
        let mut buf = Buffer::empty(area);
        let hits = render(&form, area, &mut buf, &FormStyle::default());
        let drawn = text_of(&buf);
        assert!(drawn.contains("Last field") && !drawn.contains("First list"));
        let field = hits.area_of(&Target::Field("c"));
        assert!(field.is_some_and(|rect| area.contains(Position::new(rect.x, rect.y))));
        assert!(hits.area_of(&Target::Primary).is_some());
    }

    #[test]
    fn the_summary_what_is_missing_and_a_message_wrap_and_are_never_cut_off() {
        let mut form =
            Form::new("F")
                .field(Field::text("link", "Link").validate(|_| {
                    Err("That does not look like a link at all, start again".into())
                }));
        form.set("link", crate::Value::Text("x".into()));
        form.set_summary(Some(
            "12 files, about 1.2 GB to copy · 3 already there".into(),
        ));
        form.set_blocked(Some(
            "Not enough room on SNOWSKY: 1.2 GB to copy, 0.4 GB free".into(),
        ));
        let narrow = size(&form, 24);
        let area = Rect::new(0, 0, narrow.width, narrow.height);
        let mut buf = Buffer::empty(area);
        let hits = render(&form, area, &mut buf, &FormStyle::default());
        let drawn = text_of(&buf);
        for word in ["again", "there", "free"] {
            assert!(drawn.contains(word), "{word} cut off: {drawn}");
        }
        assert!(hits.area_of(&Target::Primary).is_some());
        assert!(narrow.height > size(&form, 64).height);
    }

    #[test]
    fn a_long_radio_option_wraps_beside_its_lead_and_is_clicked_on_any_row() {
        let long = "Never Gonna Give You Up · Rick Astley · Whenever You Need Somebody · 3:34";
        let form = Form::new("F").field(Field::radio("choice", "What to do").options(vec![
            Choice::new("a", long),
            Choice::new("b", "Leave for later"),
        ]));
        let wanted = size(&form, 40);
        let area = Rect::new(0, 0, wanted.width, wanted.height);
        let mut buf = Buffer::empty(area);
        let hits = render(&form, area, &mut buf, &FormStyle::default());
        let drawn = text_of(&buf);
        for word in ["Somebody", "3:34", "Leave for later"] {
            assert!(drawn.contains(word), "{word} cut off: {drawn}");
        }
        let first = hits.area_of(&Target::Row {
            field: "choice",
            index: 0,
        });
        assert!(first.is_some_and(|rect| rect.height > 1));
    }

    #[test]
    fn a_cut_row_ends_in_an_ellipsis_and_keeps_its_detail() {
        let option = Choice::new(
            "x",
            "Rick Astley - Never Gonna Give You Up (Official Video)",
        )
        .with_detail("→ Never Gonna Give You Up");
        let style = FormStyle::default();
        let line = fit_line(option_line(&option, "", &style), 50, &style);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            text.contains('…') && text.ends_with("→ Never Gonna Give You Up"),
            "{text}"
        );
        assert!(line.width() <= 50);
    }

    #[test]
    fn the_width_is_clamped_to_the_room() {
        assert_eq!(size(&Form::new("F"), 30).width, 30);
    }
}
