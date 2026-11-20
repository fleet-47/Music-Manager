//! Popups (F1): centred with `tui-popup`; long content scrolls with `tui-scrollview` and
//! `tui-scrollbar`. A dialog's form is drawn by `sdl_form::render` inside the popup; every other
//! popup is plain text with buttons. Only the top popup records rectangles to click.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use sdl_core::{AttentionKind, TaskId};
use sdl_form::HitMap;
use tui_popup::{KnownSizeWrapper, Popup as Chrome};
use tui_scrollbar::{ScrollBar, ScrollLengths};
use tui_scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};

use super::keys::HELP;
use super::theme::Theme;
use crate::app::{App, Confirm, Hits, Popup, PopupTarget, Waiting};

/// Widest a plain popup gets, in columns.
const PLAIN_WIDTH: u16 = 60;

/// Rows a scrolling popup shows at most.
const SCROLL_ROWS: u16 = 14;

/// Draw every open popup, bottom first, and record the top one's rectangles in `hits`.
pub fn draw(frame: &mut Frame, app: &App, theme: Theme, hits: &mut Hits) {
    let area = frame.area();
    let last = app.popups.len().saturating_sub(1);
    for (index, popup) in app.popups.iter().enumerate() {
        let on_top = index == last;
        match popup {
            Popup::Dialog(dialog) => {
                let form = dialog.form();
                let wanted = sdl_form::size(form, area.width.saturating_sub(4));
                let body = FormBody {
                    form,
                    style: theme.form_style(),
                    hits: on_top.then_some(&mut hits.form),
                };
                let widget = KnownSizeWrapper::new(
                    body,
                    usize::from(wanted.width),
                    usize::from(wanted.height),
                );
                chrome(form.title(), theme)
                    .body(widget)
                    .render_in(frame, area);
            }
            Popup::Confirm(confirm) => {
                let (title, text, yes, no) = confirm_words(confirm);
                plain(
                    frame,
                    theme,
                    title,
                    vec![text],
                    &[(yes, PopupTarget::Yes), (no, PopupTarget::No)],
                    on_top.then_some(&mut hits.popup),
                );
            }
            Popup::Error(info) => plain(
                frame,
                theme,
                "Something went wrong",
                vec![info.message.clone(), info.advice.clone()],
                &[("Close", PopupTarget::Close)],
                on_top.then_some(&mut hits.popup),
            ),
            Popup::Waiting(waiting) => {
                let (text, buttons): (String, &[(&str, PopupTarget)]) = match waiting {
                    Waiting::Browser => (
                        "Waiting for you to allow access in the browser".to_owned(),
                        &[("Cancel", PopupTarget::No)],
                    ),
                    Waiting::Eject(device) => (format!("Ejecting {device}"), &[]),
                };
                plain(
                    frame,
                    theme,
                    "Please wait",
                    vec![text],
                    buttons,
                    on_top.then_some(&mut hits.popup),
                );
            }
            Popup::Help { scroll } => {
                let lines = help_lines(theme);
                scrolling(
                    frame,
                    theme,
                    "Help",
                    lines,
                    *scroll,
                    on_top.then_some(&mut hits.popup),
                );
            }
            Popup::Attention { task, scroll } => {
                let lines = attention_lines(app, *task, theme);
                scrolling(
                    frame,
                    theme,
                    &format!("{} needs attention", task.name()),
                    lines,
                    *scroll,
                    on_top.then_some(&mut hits.popup),
                );
            }
        }
    }
}

/// The popup chrome: title in the heading style, `lavender` border.
fn chrome(title: &str, theme: Theme) -> ChromeBuilder<'_> {
    ChromeBuilder { title, theme }
}

/// A popup waiting for its body.
struct ChromeBuilder<'a> {
    title: &'a str,
    theme: Theme,
}

impl<'a> ChromeBuilder<'a> {
    fn body<W>(self, body: W) -> Framed<'a, W> {
        Framed {
            popup: Chrome::new(body)
                .title(Line::styled(
                    format!(" {} ", self.title),
                    self.theme.heading(),
                ))
                .border_style(self.theme.border_focus())
                .style(self.theme.body()),
        }
    }
}

/// A popup with its body, ready to draw.
struct Framed<'a, W> {
    popup: Chrome<'a, W>,
}

impl<W> Framed<'_, W>
where
    W: tui_popup::KnownSize + Widget,
{
    fn render_in(self, frame: &mut Frame, area: Rect) {
        frame.render_widget(self.popup, area);
    }
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

/// A plain popup: wrapped text, then buttons. Records the buttons' rectangles.
fn plain(
    frame: &mut Frame,
    theme: Theme,
    title: &str,
    paragraphs: Vec<String>,
    buttons: &[(&str, PopupTarget)],
    hits: Option<&mut HitMap<PopupTarget>>,
) {
    let area = frame.area();
    let width = PLAIN_WIDTH.min(area.width.saturating_sub(4)).max(10);
    let text = Text::from(
        paragraphs
            .into_iter()
            .flat_map(|paragraph| [Line::from(paragraph), Line::default()])
            .collect::<Vec<_>>(),
    );
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true });
    let text_rows = u16::try_from(paragraph.line_count(width)).unwrap_or(u16::MAX);
    let height = text_rows.saturating_add(u16::from(!buttons.is_empty()));
    let body = PlainBody {
        paragraph,
        buttons,
        theme,
        hits,
    };
    let widget = KnownSizeWrapper::new(body, usize::from(width), usize::from(height));
    chrome(title, theme).body(widget).render_in(frame, area);
}

struct PlainBody<'a> {
    paragraph: Paragraph<'a>,
    buttons: &'a [(&'a str, PopupTarget)],
    theme: Theme,
    hits: Option<&'a mut HitMap<PopupTarget>>,
}

impl Widget for PlainBody<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut drawn = HitMap::new();
        let text_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height
                .saturating_sub(u16::from(!self.buttons.is_empty())),
        );
        self.paragraph.render(text_area, buf);
        let mut x = area.x;
        for (label, target) in self.buttons {
            let text = format!("[ {label} ]");
            let width = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
            let rect = Rect::new(
                x,
                area.bottom().saturating_sub(1),
                width.min(area.right().saturating_sub(x)),
                1,
            );
            Line::styled(text, self.theme.emphasis()).render(rect, buf);
            drawn.push(rect, *target);
            x = rect.right().saturating_add(2);
        }
        if let Some(hits) = self.hits {
            *hits = drawn;
        }
    }
}

fn help_lines(theme: Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for group in HELP {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(group.title, theme.heading()));
        for entry in group.entries {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<14}", entry.key), theme.emphasis()),
                Span::styled(entry.what, theme.body()),
            ]));
        }
    }
    lines
}

/// The items of the selected function's current run while it runs, else of its saved last run.
fn attention_lines(app: &App, task: TaskId, theme: Theme) -> Vec<Line<'static>> {
    let running = app
        .view(task)
        .is_some_and(crate::app::FunctionView::is_running);
    let items = if running {
        app.view(task).map(|view| view.state.attention.clone())
    } else {
        app.settings
            .state
            .last_run
            .get(&task)
            .map(|run| run.attention.clone())
    }
    .unwrap_or_default();
    if items.is_empty() {
        return vec![Line::styled("Nothing needs attention.", theme.secondary())];
    }
    let mut lines = Vec::new();
    for item in items {
        let style = match item.kind {
            AttentionKind::Held => theme.attention(),
            AttentionKind::Failed => theme.failed(),
        };
        let who = if item.artist.is_empty() {
            item.title
        } else {
            format!("{} · {}", item.title, item.artist)
        };
        lines.push(Line::styled(who, style));
        lines.push(Line::styled(item.reason, theme.secondary()));
        lines.push(Line::default());
    }
    lines
}

/// A popup whose lines scroll inside a fixed height, with a scrollbar.
fn scrolling(
    frame: &mut Frame,
    theme: Theme,
    title: &str,
    lines: Vec<Line<'static>>,
    scroll: usize,
    hits: Option<&mut HitMap<PopupTarget>>,
) {
    let area = frame.area();
    let width = PLAIN_WIDTH.min(area.width.saturating_sub(4)).max(12);
    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let shown = total
        .min(SCROLL_ROWS)
        .min(area.height.saturating_sub(4))
        .max(1);
    let body = ScrollBody {
        lines,
        scroll,
        theme,
        hits,
    };
    let widget = KnownSizeWrapper::new(body, usize::from(width), usize::from(shown));
    chrome(title, theme).body(widget).render_in(frame, area);
}

struct ScrollBody<'a> {
    lines: Vec<Line<'static>>,
    scroll: usize,
    theme: Theme,
    hits: Option<&'a mut HitMap<PopupTarget>>,
}

impl Widget for ScrollBody<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let content = area.width.saturating_sub(1);
        let total = u16::try_from(self.lines.len()).unwrap_or(u16::MAX);
        let mut view = ScrollView::new(Size::new(content, total))
            .scrollbars_visibility(ScrollbarVisibility::Never);
        view.render_widget(
            Paragraph::new(Text::from(self.lines)),
            Rect::new(0, 0, content, total),
        );
        let mut state = ScrollViewState::with_offset(ratatui::layout::Position::new(
            0,
            u16::try_from(self.scroll).unwrap_or(u16::MAX),
        ));
        let text_area = Rect::new(area.x, area.y, content, area.height);
        ratatui::widgets::StatefulWidget::render(view, text_area, buf, &mut state);
        let bar_area = Rect::new(area.right().saturating_sub(1), area.y, 1, area.height);
        let bar = ScrollBar::vertical(ScrollLengths {
            content_len: usize::from(total),
            viewport_len: usize::from(area.height),
        })
        .offset(self.scroll)
        .thumb_style(self.theme.emphasis());
        (&bar).render(bar_area, buf);
        if let Some(hits) = self.hits {
            let mut drawn = HitMap::new();
            drawn.push(bar_area, PopupTarget::Scrollbar);
            *hits = drawn;
        }
    }
}
