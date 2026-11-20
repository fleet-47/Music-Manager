//! Centred popups (F1), built with `Clear` and a bordered `Block`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use sdl_core::{AttentionKind, ErrorInfo, SignInState, TaskId};

use crate::app::{App, Popup};
use crate::fmt;
use crate::form;
use crate::keys::{self, HELP, Hint, KEY_WIDTH};
use crate::tasks::task_name;
use crate::theme::Theme;
use crate::ui::spinner;

const NARROW: u16 = 56;
const WIDE: u16 = 72;

/// The key hints for the screen as it is now: the footer line and the line at a popup's foot.
/// When they do not fit in `width` columns, the first hints go (moving keys come first), so the
/// keys that start, stop, cancel and quit, and the note on what is missing, stay visible.
#[must_use]
pub fn hint_line(app: &App, theme: Theme, width: u16) -> Line<'static> {
    let hints = app.hints();
    let len = |hints: &[Hint]| {
        hints
            .iter()
            .map(|hint| match hint {
                Hint::Key(key, what) => key
                    .chars()
                    .count()
                    .saturating_add(what.chars().count())
                    .saturating_add(3),
                Hint::Missing(text) => text.chars().count().saturating_add(2),
            })
            .fold(1, usize::saturating_add)
            // The last hint needs no gap after it.
            .saturating_sub(2)
    };
    let mut from = 0;
    while from < hints.len() && len(hints.get(from..).unwrap_or_default()) > usize::from(width) {
        from = from.saturating_add(1);
    }
    let mut spans = vec![Span::raw(" ")];
    for hint in hints.into_iter().skip(from) {
        match hint {
            Hint::Key(key, what) => {
                spans.push(Span::styled(key, theme.emphasis()));
                spans.push(Span::styled(format!(" {what}  "), theme.secondary()));
            }
            Hint::Missing(text) => spans.push(Span::styled(text, theme.attention())),
        }
    }
    Line::from(spans)
}

/// Draw the open popup, if any.
pub fn render(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let Some(popup) = app.popup() else {
        return;
    };
    match popup {
        Popup::SignIn(state) => text_popup(
            frame,
            area,
            app,
            theme,
            "Sign in to Spotify",
            sign_in_lines(state, app, theme),
        ),
        Popup::Looking(task) => {
            let text = match task {
                TaskId::SyncDevice => "Looking for devices…",
                _ => "Looking at your music folders…",
            };
            text_popup(
                frame,
                area,
                app,
                theme,
                task_name(*task),
                vec![busy(text, app, theme)],
            );
        }
        Popup::Form(dialog) => {
            let width = form::view::WIDTH.min(area.width).saturating_sub(4);
            let hints = hint_line(app, theme, width);
            form::view::render(
                frame,
                area,
                theme,
                dialog.form(),
                hints,
                &spinner(app, theme),
            );
        }
        Popup::EjectConfirm(name) => text_popup(
            frame,
            area,
            app,
            theme,
            &format!("Clean up and eject {name}?"),
            vec![Line::styled(
                "Remove .Spotlight-V100, .Trashes and .fseventsd from the device, then unmount it.",
                theme.body(),
            )],
        ),
        Popup::Ejecting => text_popup(
            frame,
            area,
            app,
            theme,
            "Sync device",
            vec![busy("Cleaning up and unmounting the device…", app, theme)],
        ),
        Popup::Attention { task, cursor } => {
            render_attention(frame, area, app, theme, *task, *cursor);
        }
        Popup::ConfirmStop(task) => text_popup(
            frame,
            area,
            app,
            theme,
            &format!("Stop {}?", task_name(*task)),
            vec![Line::styled(
                "The task stops now. Files already finished stay.",
                theme.body(),
            )],
        ),
        Popup::ConfirmQuit => {
            let names: Vec<&str> = app.running().into_iter().map(task_name).collect();
            let them = if names.len() == 1 { "it" } else { "them" };
            let lines = vec![Line::styled(
                format!(
                    "Still running: {}. Quitting stops {them}.",
                    names.join(", ")
                ),
                theme.body(),
            )];
            text_popup(frame, area, app, theme, "Quit?", lines);
        }
        Popup::Error(task, info) => text_popup(
            frame,
            area,
            app,
            theme,
            &format!("{}: something went wrong", task_name(*task)),
            error_lines(info, theme),
        ),
        Popup::Help { scroll } => render_help(frame, area, app, theme, *scroll),
    }
}

fn busy(text: &str, app: &App, theme: Theme) -> Line<'static> {
    Line::from(vec![
        spinner(app, theme),
        Span::styled(text.to_owned(), theme.body()),
    ])
}

fn sign_in_lines(state: &SignInState, app: &App, theme: Theme) -> Vec<Line<'static>> {
    match state {
        SignInState::OpeningBrowser => vec![busy("Opening your browser…", app, theme)],
        SignInState::WaitingForBrowser => {
            vec![busy(
                "Waiting for you to approve in the browser…",
                app,
                theme,
            )]
        }
        SignInState::SignedIn => vec![
            Line::from(vec![
                Span::styled("✓ ", theme.done()),
                Span::styled("Signed in.", theme.body()),
            ]),
            busy("Loading your lists…", app, theme),
        ],
        SignInState::Failed(reason) => vec![
            Line::from(vec![
                Span::styled("✗ ", theme.failed()),
                Span::styled("Sign-in failed.", theme.body()),
            ]),
            Line::styled(reason.clone(), theme.secondary()),
        ],
    }
}

fn error_lines(info: &ErrorInfo, theme: Theme) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("✗ ", theme.failed()),
            Span::styled(info.message.clone(), theme.body()),
        ]),
        Line::default(),
        Line::from(vec![
            Span::styled("What to do: ", theme.secondary()),
            Span::styled(info.advice.clone(), theme.body()),
        ]),
    ]
}

pub(crate) fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    rect
}

/// Clear the area and draw the popup's frame; returns the room inside.
pub(crate) fn frame_popup(frame: &mut Frame, rect: Rect, theme: Theme, title: &str) -> Rect {
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(Line::styled(format!(" {title} "), theme.heading()))
        .border_style(theme.border_focus())
        .style(theme.background())
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// The hint line in its own row at a popup's foot.
fn render_hints(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    frame.render_widget(Paragraph::new(hint_line(app, theme, area.width)), area);
}

pub(crate) fn scrollbar(frame: &mut Frame, area: Rect, theme: Theme, len: usize, at: usize) {
    let mut state = ScrollbarState::new(len).position(at);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight).style(theme.border()),
        area,
        &mut state,
    );
}

/// A popup of wrapped text with the key hints under it.
fn text_popup(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: Theme,
    title: &str,
    mut lines: Vec<Line<'static>>,
) {
    let width = NARROW.min(area.width);
    if !app.hints().is_empty() {
        lines.push(Line::default());
        lines.push(hint_line(app, theme, width.saturating_sub(4)));
    }
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .style(theme.body());
    let rows = u16::try_from(paragraph.line_count(width.saturating_sub(4))).unwrap_or(u16::MAX);
    let rect = centred(area, width, rows.saturating_add(2));
    let inner = frame_popup(frame, rect, theme, title);
    frame.render_widget(paragraph, inner);
}

/// Unmatched items are yellow, failed ones red.
fn reason_style(kind: AttentionKind, theme: Theme) -> Style {
    match kind {
        AttentionKind::Unmatched => theme.attention(),
        AttentionKind::Failed => theme.failed(),
    }
}

fn render_attention(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: Theme,
    task: TaskId,
    cursor: usize,
) {
    let items = &app.view(task).state.attention;
    let rect = centred(area, WIDE, 20);
    let title = format!(
        "{}: {} need attention",
        task_name(task),
        fmt::thousands(items.len())
    );
    let inner = frame_popup(frame, rect, theme, &title);
    let [body, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let [rows, bar] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(body);
    // Each item is its title line and its reason, wrapped to the width.
    let reason_width = rows.width.saturating_sub(2);
    let reasons: Vec<Paragraph> = items
        .iter()
        .map(|item| {
            Paragraph::new(Line::styled(
                item.reason.clone(),
                reason_style(item.kind, theme),
            ))
            .wrap(Wrap { trim: false })
        })
        .collect();
    let heights: Vec<usize> = reasons
        .iter()
        .map(|reason| {
            reason
                .line_count(reason_width.max(1))
                .max(1)
                .saturating_add(1)
        })
        .collect();
    let top = window_top(items.len(), cursor, usize::from(rows.height), |at| {
        heights.get(at).copied().unwrap_or(1)
    });
    let mut y = rows.y;
    for (at, (item, reason)) in items.iter().zip(reasons).enumerate().skip(top) {
        let height = u16::try_from(heights.get(at).copied().unwrap_or(1)).unwrap_or(u16::MAX);
        let slot = Rect::new(rows.x, y, rows.width, height).intersection(rows);
        if slot.is_empty() {
            break;
        }
        let here = at == cursor;
        if here {
            frame.render_widget(Block::new().style(theme.selected()), slot);
        }
        let mut first = vec![
            Span::styled(if here { "▶ " } else { "  " }, theme.emphasis()),
            Span::styled(item.title.clone(), theme.body()),
        ];
        if !item.artist.is_empty() {
            first.push(Span::styled(
                format!(" · {}", item.artist),
                theme.secondary(),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(first)),
            Rect::new(slot.x, slot.y, slot.width, 1),
        );
        let text = Rect::new(
            slot.x.saturating_add(2),
            slot.y.saturating_add(1),
            slot.width.saturating_sub(2),
            slot.height.saturating_sub(1),
        );
        frame.render_widget(reason, text);
        y = y.saturating_add(height);
    }
    scrollbar(frame, bar, theme, items.len(), cursor);
    render_hints(frame, hint, app, theme);
}

/// First entry shown, for a list of `len` entries (`height` gives each one's lines) in `room`
/// lines with the cursor on entry `cursor`: the cursor sits near the middle, and room left over at
/// the end of the list goes above it, so the window never shows blank space it could fill.
#[must_use]
pub fn window_top(
    len: usize,
    cursor: usize,
    room: usize,
    height: impl Fn(usize) -> usize,
) -> usize {
    let cursor = cursor.min(len.saturating_sub(1));
    let above_budget = room.saturating_sub(height(cursor)) / 2;
    let mut top = cursor;
    let mut above = 0_usize;
    while top > 0 && above.saturating_add(height(top.saturating_sub(1))) <= above_budget {
        top = top.saturating_sub(1);
        above = above.saturating_add(height(top));
    }
    let below = (cursor.saturating_add(1)..len)
        .map(&height)
        .fold(0, usize::saturating_add);
    let used = above.saturating_add(height(cursor));
    let mut spare = room.saturating_sub(used).saturating_sub(below);
    while top > 0 && height(top.saturating_sub(1)) <= spare {
        top = top.saturating_sub(1);
        spare = spare.saturating_sub(height(top));
    }
    top
}

fn help_lines(theme: Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for group in HELP {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(group.title, theme.heading()));
        for key in group.keys {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<KEY_WIDTH$}", key.help_key), theme.emphasis()),
                Span::styled(key.what, theme.body()),
            ]));
        }
    }
    lines
}

/// Lines of help text the popup shows at once on a screen `screen_height` rows tall: its own
/// height less the border and the hint line.
fn help_visible(screen_height: u16) -> usize {
    let wanted = keys::help_line_count().saturating_add(3);
    usize::from(screen_height).min(wanted).saturating_sub(3)
}

/// The furthest the help can scroll on a screen `screen_height` rows tall: the last full page.
#[must_use]
pub fn help_max_scroll(screen_height: u16) -> usize {
    keys::help_line_count().saturating_sub(help_visible(screen_height))
}

fn render_help(frame: &mut Frame, area: Rect, app: &App, theme: Theme, scroll: usize) {
    let lines = help_lines(theme);
    let total = lines.len();
    let height = u16::try_from(total.saturating_add(3)).unwrap_or(u16::MAX);
    let rect = centred(area, NARROW, height);
    let inner = frame_popup(frame, rect, theme, "Keys");
    let [body, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let [text, bar] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(body);
    // The last page stays full: scrolling past it shows the end, not blank lines.
    let visible = usize::from(text.height);
    let last_top = total.saturating_sub(visible);
    let top = scroll.min(last_top);
    let offset = u16::try_from(top).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), text);
    if last_top > 0 {
        scrollbar(frame, bar, theme, last_top.saturating_add(1), top);
    }
    render_hints(frame, hint, app, theme);
}
