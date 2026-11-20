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
    /// The focused field's label and the buttons when focused.
    pub focus: Style,
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
        body_height(form, width).saturating_add(foot_height(form))
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
        .map(field_height)
        .fold(notes, u16::saturating_add)
}

/// Rows of the part that never scrolls: the summary row (always kept, so the form does not jump
/// when a summary arrives), what is missing, and the buttons.
fn foot_height(form: &Form) -> u16 {
    u16::try_from(form.missing().len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
}

/// Rows a field takes: its label row, then its body.
fn field_height(field: &Field) -> u16 {
    let body = match &field.kind {
        Kind::Text(text) => 1_u16.saturating_add(u16::from(text.complaint().is_some())),
        Kind::Folder(Folder { text, .. }) => {
            1_u16.saturating_add(u16::from(text.complaint().is_some()))
        }
        Kind::Dropdown(pick) if pick.open => open_rows(pick),
        Kind::Toggle(_) | Kind::Dropdown(_) => 0,
        Kind::Radio(pick) => u16::try_from(pick.options.len().max(1)).unwrap_or(u16::MAX),
        Kind::Ticks(_) | Kind::List(_) => LIST_ROWS.saturating_add(2),
    };
    1_u16.saturating_add(body)
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
    let foot_rows = foot_height(form).min(area.height);
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
        let height = field_height(field).min(bottom.saturating_sub(y));
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
    let mut y = area.y;
    let bottom = area.bottom();
    let mut row = || -> Option<Rect> {
        (y < bottom).then(|| {
            let rect = Rect::new(area.x, y, area.width, 1);
            y = y.saturating_add(1);
            rect
        })
    };
    if let Some(rect) = row() {
        let line = match (form.busy(), form.summary()) {
            (Some(busy), _) => {
                let frame = crate::spinner(form.spin());
                Line::from(vec![
                    Span::styled(format!("{frame} "), style.emphasis),
                    Span::styled(busy, style.secondary),
                ])
            }
            (None, Some(summary)) => Line::styled(summary, style.body),
            (None, None) => Line::default(),
        };
        line.render(rect, buf);
    }
    for missing in form.missing() {
        if let Some(rect) = row() {
            Line::styled(format!("· {missing}"), style.error).render(rect, buf);
        }
    }
    if let Some(rect) = row() {
        draw_buttons(form, rect, buf, style, hits);
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
            Line::styled(" Browse… ", if at { style.focus } else { style.emphasis })
                .render(browse, buf);
            hits.push(browse, Target::Browse(field.key));
        }
        Kind::Dropdown(pick) if pick.open => {
            draw_options(pick, body, field.key, buf, style, hits, true);
        }
        Kind::Toggle(_) | Kind::Dropdown(_) => {}
        Kind::Radio(pick) => draw_options(pick, body, field.key, buf, style, hits, false),
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
    if let Some(complaint) = text.complaint() {
        let below = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        if below.bottom() <= area.bottom() {
            Line::styled(complaint, style.error).render(below, buf);
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

fn draw_options(
    pick: &Pick,
    area: Rect,
    key: crate::Key,
    buf: &mut Buffer,
    style: &FormStyle,
    hits: &mut HitMap<Target>,
    dropdown: bool,
) {
    if pick.options.is_empty() {
        Line::styled(pick.empty.as_str(), style.secondary).render(area, buf);
        return;
    }
    let rows = if dropdown {
        open_rows(pick)
    } else {
        area.height
    };
    let first = if dropdown {
        pick.cursor
            .saturating_add(1)
            .saturating_sub(usize::from(rows))
    } else {
        0
    };
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
        let on = if dropdown {
            pick.cursor == index
        } else {
            pick.chosen.as_ref() == Some(&option.id)
        };
        let lead = match (dropdown, on) {
            (true, true) => "› ",
            (true, false) => "  ",
            (false, true) => "(•) ",
            (false, false) => "( ) ",
        };
        if on {
            buf.set_style(row, style.selected);
        }
        option_line(option, lead, style).render(row, buf);
        hits.push(row, Target::Row { field: key, index });
    }
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
        Line::styled(text, style.secondary).render(list, buf);
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
        option_line(option, lead, style).render(row, buf);
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
        format!("{} of {} ticked", rows.ticked.len(), rows.options.len())
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
            style.focus
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
    Line::styled(browser.cwd().display().to_string(), style.label).render(head, buf);
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
    fn the_width_is_clamped_to_the_room() {
        assert_eq!(size(&Form::new("F"), 30).width, 30);
    }
}
