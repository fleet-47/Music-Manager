//! Popups (F1): centred with `tui-popup`; long content scrolls with `tui-scrollview` and
//! `tui-scrollbar`. A dialog's form is drawn by `sdl_form::render` inside the popup; every other
//! popup is wrapped text with buttons that name their key. Only the top popup records rectangles
//! to click.
//!
//! [`extent`] is the pure half of a scrolling popup: how long its content is and how much of it
//! shows, so keys and clicks clamp the scroll against the same numbers the drawing uses.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Position, Rect, Size};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, StatefulWidget, Widget, Wrap};
use sdl_core::{Attention, AttentionKind, TaskId};
use sdl_form::HitMap;
use tui_popup::{KnownSizeWrapper, Popup as Chrome};
use tui_scrollbar::{ScrollBar, ScrollLengths};
use tui_scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};

use super::keys::{BINDINGS, Scope};
use super::theme::Theme;
use crate::app::{App, Confirm, Hits, Popup, PopupTarget, Waiting};

/// Widest a plain popup gets, in columns.
const PLAIN_WIDTH: u16 = 60;

/// Rows a scrolling popup shows at most.
const SCROLL_ROWS: u16 = 14;

/// Rows around a popup's body that are not the body: two borders and a margin on each side.
const AROUND: u16 = 5;

/// How long a scrolling popup's content is, and how much of it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extent {
    /// Wrapped lines of content.
    pub content: usize,
    /// Rows shown at once.
    pub viewport: usize,
    /// First wrapped line and wrapped length of each clickable item (attention items).
    pub items: Vec<(usize, usize)>,
}

impl Extent {
    /// The largest first line: showing the last line at the bottom.
    #[must_use]
    pub fn max_scroll(&self) -> usize {
        self.content.saturating_sub(self.viewport)
    }
}

/// The content of a scrolling popup as blocks of lines. A block is never split by a wrapped
/// line count of another, so a block's wrapped length is its own.
struct Content {
    title: String,
    blocks: Vec<Vec<Line<'static>>>,
    clickable: bool,
}

fn help_content(theme: Theme) -> Content {
    let blocks = Scope::ALL
        .into_iter()
        .map(|scope| {
            let mut lines = vec![Line::styled(scope.title(), theme.heading())];
            lines.extend(
                BINDINGS
                    .iter()
                    .filter(|binding| binding.scope == scope)
                    .map(|binding| {
                        Line::from(vec![
                            Span::styled(format!("{:<13} ", binding.key), theme.emphasis()),
                            Span::styled(binding.what, theme.body()),
                        ])
                    }),
            );
            lines.push(Line::default());
            lines
        })
        .collect();
    Content {
        title: "Help".to_owned(),
        blocks,
        clickable: false,
    }
}

/// The items of the function's current run while it runs, else of its saved last run.
fn attention_items(app: &App, task: TaskId) -> Vec<Attention> {
    let running = app.view(task).filter(|view| view.is_running());
    match running {
        Some(view) => view.state.attention.clone(),
        None => app
            .settings
            .state
            .last_run
            .get(&task)
            .map(|run| run.attention.clone())
            .unwrap_or_default(),
    }
}

fn attention_content(app: &App, task: TaskId, theme: Theme) -> Content {
    let items = attention_items(app, task);
    let clickable = !items.is_empty();
    let blocks = if items.is_empty() {
        vec![vec![Line::styled(
            "Nothing needs attention.",
            theme.secondary(),
        )]]
    } else {
        items
            .into_iter()
            .map(|item| {
                let style = match item.kind {
                    AttentionKind::Held => theme.attention(),
                    AttentionKind::Failed => theme.failed(),
                };
                let who = if item.artist.is_empty() {
                    item.title
                } else {
                    format!("{} · {}", item.title, item.artist)
                };
                vec![
                    Line::styled(who, style),
                    Line::styled(item.reason, theme.secondary()),
                    Line::default(),
                ]
            })
            .collect()
    };
    Content {
        title: format!("{} needs attention", task.name()),
        blocks,
        clickable,
    }
}

fn content_of(app: &App, popup: &Popup, theme: Theme) -> Option<Content> {
    match popup {
        Popup::Help { .. } => Some(help_content(theme)),
        Popup::Attention { task, .. } => Some(attention_content(app, *task, theme)),
        _ => None,
    }
}

/// The width of a scrolling popup's body, and of its text beside the scrollbar.
fn scroll_widths(area: Size) -> (u16, u16) {
    let width = PLAIN_WIDTH.min(area.width.saturating_sub(4)).max(12);
    (width, width.saturating_sub(1))
}

fn wrapped(lines: &[Line<'static>], width: u16) -> usize {
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width)
}

fn extent_of(content: &Content, area: Size) -> Extent {
    let (_, text) = scroll_widths(area);
    let mut start = 0_usize;
    let mut items = Vec::new();
    for block in &content.blocks {
        let len = wrapped(block, text);
        if content.clickable {
            items.push((start, len));
        }
        start = start.saturating_add(len);
    }
    let rows = SCROLL_ROWS.min(area.height.saturating_sub(AROUND)).max(1);
    Extent {
        content: start,
        viewport: usize::from(rows).min(start.max(1)),
        items,
    }
}

/// The extent of a scrolling popup (help, attention list) on a terminal of `area`; `None` for
/// any other popup.
#[must_use]
pub fn extent(app: &App, popup: &Popup, area: Size) -> Option<Extent> {
    content_of(app, popup, Theme::new(false)).map(|content| extent_of(&content, area))
}

/// Where popups go: the screen above the footer, so the keys that work stay in view under any
/// popup.
fn room(frame: &Frame) -> Rect {
    let area = frame.area();
    Rect {
        height: area.height.saturating_sub(1),
        ..area
    }
}

/// Draw every open popup, bottom first, and record the top one's rectangles in `hits`. Returns
/// the top popup's frame, so an effect can play over just it.
pub fn draw(frame: &mut Frame, app: &App, theme: Theme, hits: &mut Hits) -> Option<Rect> {
    let area = room(frame);
    let last = app.popups.len().saturating_sub(1);
    let mut top = None;
    for (index, popup) in app.popups.iter().enumerate() {
        let on_top = index == last;
        let shown = match popup {
            Popup::Dialog(dialog) => {
                let form = dialog.form();
                let wanted = sdl_form::size(form, area.width.saturating_sub(4));
                let width = wanted.width.min(area.width.saturating_sub(2));
                let height = wanted.height.min(area.height.saturating_sub(2));
                let body = FormBody {
                    form,
                    style: theme.form_style(),
                    hits: on_top.then_some(&mut hits.form),
                };
                show(frame, theme, form.title(), body, (width, height), on_top)
            }
            Popup::Confirm(confirm) => {
                let (title, text, yes, no) = confirm_words(confirm);
                let buttons = [
                    Button::new("y", yes, PopupTarget::Yes),
                    Button::new("n", no, PopupTarget::No),
                ];
                plain(
                    frame,
                    theme,
                    title,
                    &[text],
                    &buttons,
                    on_top.then_some(&mut hits.popup),
                )
            }
            Popup::Error(info) => plain(
                frame,
                theme,
                "Something went wrong",
                &[info.message.clone(), info.advice.clone()],
                &[Button::new("Esc", "Close", PopupTarget::Close)],
                on_top.then_some(&mut hits.popup),
            ),
            Popup::Waiting(waiting) => {
                let spinner = sdl_form::spinner(app.spin);
                let (text, buttons) = match waiting {
                    Waiting::Browser => (
                        format!("{spinner} Waiting for you to allow access in the browser"),
                        vec![Button::new("Esc", "Cancel", PopupTarget::No)],
                    ),
                    Waiting::Eject(device) => (
                        format!(
                            "{spinner} Ejecting {device}. Hiding this does not stop it; a problem still shows here."
                        ),
                        vec![Button::new("Esc", "Hide", PopupTarget::Close)],
                    ),
                };
                plain(
                    frame,
                    theme,
                    "Please wait",
                    &[text],
                    &buttons,
                    on_top.then_some(&mut hits.popup),
                )
            }
            Popup::Help { scroll } | Popup::Attention { scroll, .. } => {
                let Some(content) = content_of(app, popup, theme) else {
                    continue;
                };
                scrolling(
                    frame,
                    theme,
                    content,
                    *scroll,
                    on_top.then_some(&mut hits.popup),
                )
            }
        };
        if on_top {
            top = Some(shown);
        }
    }
    top
}

/// Draw `body` in a popup of `width` by `height`; the frame's rectangle. Only the popup on
/// top has the focused border; the ones waiting under it have the plain one.
fn show<W: Widget>(
    frame: &mut Frame,
    theme: Theme,
    title: &str,
    body: W,
    (width, height): (u16, u16),
    on_top: bool,
) -> Rect {
    let area = room(frame);
    let widget = KnownSizeWrapper::new(body, usize::from(width), usize::from(height));
    let popup = Chrome::new(widget)
        .title(Line::styled(format!(" {title} "), theme.heading()))
        .border_style(if on_top {
            theme.border_focus()
        } else {
            theme.border()
        })
        .style(theme.background());
    frame.render_widget(popup, area);
    // Where `tui-popup` put the frame: centred, the body plus a border on each side.
    area.centered(
        Constraint::Length(width.saturating_add(2)),
        Constraint::Length(height.saturating_add(2)),
    )
}

/// A dialog's form as a popup body.
struct FormBody<'a> {
    form: &'a sdl_form::Form,
    style: sdl_form::FormStyle,
    hits: Option<&'a mut HitMap<sdl_form::Target>>,
}

impl Widget for FormBody<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let drawn = sdl_form::render(self.form, area, buf, &self.style);
        if let Some(hits) = self.hits {
            *hits = drawn;
        }
    }
}

fn confirm_words(confirm: &Confirm) -> (&'static str, String, &'static str, &'static str) {
    match confirm {
        Confirm::Stop(task) => (
            "Stop?",
            format!("Stop {}? What is finished stays.", task.name()),
            "Stop",
            "Keep going",
        ),
        Confirm::Quit => (
            "Quit?",
            "A function is still running. Quit and stop it?".to_owned(),
            "Quit",
            "Keep going",
        ),
        Confirm::Eject { device } => (
            "Eject?",
            format!("Clean up and eject {device}? Clean-up also empties the device's Trash."),
            "Clean up and eject",
            "Leave it",
        ),
        Confirm::Browser => (
            "Sign in",
            "Sync Spotify opens your browser to sign in".to_owned(),
            "Continue",
            "Cancel",
        ),
    }
}

/// A popup button: the key that does the same, its words, and what a click means.
struct Button {
    key: &'static str,
    label: &'static str,
    target: PopupTarget,
}

impl Button {
    const fn new(key: &'static str, label: &'static str, target: PopupTarget) -> Self {
        Self { key, label, target }
    }
}

/// Draw the buttons along `row`, recording each rectangle.
fn draw_buttons(
    buf: &mut Buffer,
    row: Rect,
    buttons: &[Button],
    theme: Theme,
    drawn: &mut HitMap<PopupTarget>,
) {
    let mut x = row.x;
    for button in buttons {
        let text = Line::from(vec![
            Span::styled(format!("[{}] ", button.key), theme.emphasis()),
            Span::styled(button.label, theme.body()),
        ]);
        let wide = u16::try_from(text.width()).unwrap_or(u16::MAX);
        let rect = Rect::new(x, row.y, wide.min(row.right().saturating_sub(x)), 1);
        text.render(rect, buf);
        drawn.push(rect, button.target);
        x = rect.right().saturating_add(2);
    }
}

/// A plain popup: wrapped text, then buttons.
fn plain(
    frame: &mut Frame,
    theme: Theme,
    title: &str,
    paragraphs: &[String],
    buttons: &[Button],
    hits: Option<&mut HitMap<PopupTarget>>,
) -> Rect {
    let area = room(frame);
    let width = PLAIN_WIDTH.min(area.width.saturating_sub(4)).max(10);
    let text = Text::from(
        paragraphs
            .iter()
            .flat_map(|paragraph| [Line::from(paragraph.as_str()), Line::default()])
            .collect::<Vec<_>>(),
    );
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
    let text_rows = u16::try_from(paragraph.line_count(width)).unwrap_or(u16::MAX);
    let buttons_row = u16::from(!buttons.is_empty());
    let height = text_rows
        .saturating_add(buttons_row)
        .min(area.height.saturating_sub(2));
    let on_top = hits.is_some();
    let body = PlainBody {
        paragraph,
        buttons,
        theme,
        hits,
    };
    show(frame, theme, title, body, (width, height), on_top)
}

struct PlainBody<'a> {
    paragraph: Paragraph<'a>,
    buttons: &'a [Button],
    theme: Theme,
    hits: Option<&'a mut HitMap<PopupTarget>>,
}

impl Widget for PlainBody<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut drawn = HitMap::new();
        let below = u16::from(!self.buttons.is_empty());
        self.paragraph.render(
            Rect::new(
                area.x,
                area.y,
                area.width,
                area.height.saturating_sub(below),
            ),
            buf,
        );
        let row = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        draw_buttons(buf, row, self.buttons, self.theme, &mut drawn);
        if let Some(hits) = self.hits {
            *hits = drawn;
        }
    }
}

/// A popup whose content scrolls inside a fixed height, with a scrollbar and a Close button.
fn scrolling(
    frame: &mut Frame,
    theme: Theme,
    content: Content,
    scroll: usize,
    hits: Option<&mut HitMap<PopupTarget>>,
) -> Rect {
    let area = frame.area();
    let size = Size::new(area.width, area.height);
    let (width, _) = scroll_widths(size);
    let extent = extent_of(&content, size);
    let height = u16::try_from(extent.viewport)
        .unwrap_or(u16::MAX)
        .saturating_add(1);
    let title = content.title.clone();
    let on_top = hits.is_some();
    let body = ScrollBody {
        lines: content.blocks.into_iter().flatten().collect(),
        extent,
        scroll,
        theme,
        hits,
    };
    show(frame, theme, &title, body, (width, height), on_top)
}

struct ScrollBody<'a> {
    lines: Vec<Line<'static>>,
    extent: Extent,
    scroll: usize,
    theme: Theme,
    hits: Option<&'a mut HitMap<PopupTarget>>,
}

impl Widget for ScrollBody<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut drawn = HitMap::new();
        let text_width = area.width.saturating_sub(1);
        let view_rows = area.height.saturating_sub(1);
        let content = u16::try_from(self.extent.content).unwrap_or(u16::MAX);
        let offset = self.scroll.min(self.extent.max_scroll());
        let mut view = ScrollView::new(Size::new(text_width, content))
            .scrollbars_visibility(ScrollbarVisibility::Never);
        view.render_widget(
            Paragraph::new(Text::from(self.lines)).wrap(Wrap { trim: false }),
            Rect::new(0, 0, text_width, content),
        );
        let mut state = ScrollViewState::with_offset(Position::new(
            0,
            u16::try_from(offset).unwrap_or(u16::MAX),
        ));
        let text_area = Rect::new(area.x, area.y, text_width, view_rows);
        view.render(text_area, buf, &mut state);
        let bar_area = Rect::new(area.right().saturating_sub(1), area.y, 1, view_rows);
        let bar = ScrollBar::vertical(ScrollLengths {
            content_len: self.extent.content,
            viewport_len: self.extent.viewport,
        })
        .offset(offset)
        .track_style(self.theme.selected())
        .thumb_style(self.theme.emphasis().patch(self.theme.selected()));
        // A scrollbar only when there is something to scroll to.
        if self.extent.max_scroll() > 0 {
            (&bar).render(bar_area, buf);
            drawn.push(bar_area, PopupTarget::Scrollbar);
        }
        for (index, (start, len)) in self.extent.items.iter().enumerate() {
            let top = i64::try_from(*start)
                .unwrap_or(i64::MAX)
                .saturating_sub(i64::try_from(offset).unwrap_or(0));
            let first = top.max(0);
            let last = top
                .saturating_add(i64::try_from(*len).unwrap_or(0))
                .min(i64::from(view_rows));
            if last > first {
                let y = area.y.saturating_add(u16::try_from(first).unwrap_or(0));
                let rows = u16::try_from(last.saturating_sub(first)).unwrap_or(0);
                drawn.push(
                    Rect::new(area.x, y, text_width, rows),
                    PopupTarget::Item(index),
                );
            }
        }
        let row = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        draw_buttons(
            buf,
            row,
            &[Button::new("Esc", "Close", PopupTarget::Close)],
            self.theme,
            &mut drawn,
        );
        if let Some(hits) = self.hits {
            *hits = drawn;
        }
    }
}
