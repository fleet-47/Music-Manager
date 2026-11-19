//! Centred popups (F1), built with `Clear` and a bordered `Block`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use sdl_core::{ErrorInfo, SignInState, TaskId};

use crate::app::{App, ListPopup, ListStage, Popup, task_name};
use crate::fmt;
use crate::keys::{HELP, KEY_WIDTH};
use crate::theme::Theme;
use crate::ui::{numbered, spinner};

const NARROW: u16 = 56;
const WIDE: u16 = 72;

/// The key hints for the screen as it is now: the footer line and the line at a popup's foot.
/// When they do not fit in `width` columns, the first hints go (moving keys come first), so the
/// keys that start, stop, cancel and quit stay visible.
#[must_use]
pub fn hint_line(app: &App, theme: Theme, width: u16) -> Line<'static> {
    let hints = app.hints();
    let len = |hints: &[(&str, &str)]| {
        hints
            .iter()
            .map(|(key, what)| {
                key.chars()
                    .count()
                    .saturating_add(what.chars().count())
                    .saturating_add(3)
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
    for (key, what) in hints.into_iter().skip(from) {
        spans.push(Span::styled(key, theme.emphasis()));
        spans.push(Span::styled(format!(" {what}  "), theme.secondary()));
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
        Popup::Lists(list) => render_lists(frame, area, app, theme, list),
        Popup::Link(input) => render_link(frame, area, app, theme, input),
        Popup::CardPlan(None) => text_popup(
            frame,
            area,
            app,
            theme,
            "Sync card",
            vec![busy("Mounting the card…", app, theme)],
        ),
        Popup::CardPlan(Some(plan)) => {
            let text = format!(
                "{} files to copy, {} already on the card",
                fmt::thousands(plan.to_copy),
                fmt::thousands(plan.already)
            );
            let lines = vec![Line::from(numbered(&text, theme.body(), theme))];
            text_popup(frame, area, app, theme, "Sync card", lines);
        }
        Popup::UnmountConfirm => text_popup(
            frame,
            area,
            app,
            theme,
            "Clean up and unmount?",
            vec![Line::styled(
                "Remove .Spotlight-V100, .Trashes and .fseventsd from the card, then unmount it.",
                theme.body(),
            )],
        ),
        Popup::Unmounting => text_popup(
            frame,
            area,
            app,
            theme,
            "Sync card",
            vec![busy("Cleaning up and unmounting the card…", app, theme)],
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
        SignInState::SignedIn => vec![Line::from(vec![
            Span::styled("✓ ", theme.done()),
            Span::styled("Signed in. Loading your lists…", theme.body()),
        ])],
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

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    rect
}

/// Clear the area and draw the popup's frame; returns the room inside.
fn frame_popup(frame: &mut Frame, rect: Rect, theme: Theme, title: &str) -> Rect {
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

fn scrollbar(frame: &mut Frame, area: Rect, theme: Theme, len: usize, at: usize) {
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

fn render_lists(frame: &mut Frame, area: Rect, app: &App, theme: Theme, list: &ListPopup) {
    let rows = u16::try_from(list.choices.len().min(10)).unwrap_or(10);
    let rect = centred(area, NARROW, rows.saturating_add(6));
    let inner = frame_popup(frame, rect, theme, "Sync Spotify: choose lists");
    let [intro, body, _, status, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(Span::styled("Tick the lists to sync.", theme.secondary())),
        intro,
    );
    let [choices, bar] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(body);
    // Counts line up on the right: names get the room left by the marker, the tick box, a gap
    // and the widest count.
    let count_width = list
        .choices
        .iter()
        .map(|choice| fmt::thousands(choice.count).chars().count())
        .max()
        .unwrap_or(0);
    let name_width = usize::from(choices.width)
        .saturating_sub(8)
        .saturating_sub(count_width);
    let items: Vec<ListItem> = list
        .choices
        .iter()
        .zip(&list.ticked)
        .map(|(choice, on)| {
            let (mark, style) = if *on {
                ("[x] ", theme.done())
            } else {
                ("[ ] ", theme.secondary())
            };
            let name: String = choice.name.chars().take(name_width).collect();
            ListItem::new(Line::from(vec![
                Span::styled(mark, style),
                Span::styled(format!("{name:<name_width$}  "), theme.body()),
                Span::styled(
                    format!("{:>count_width$}", fmt::thousands(choice.count)),
                    theme.emphasis(),
                ),
            ]))
        })
        .collect();
    let widget = List::new(items)
        .highlight_style(theme.selected())
        .highlight_symbol(Line::styled("▶ ", theme.emphasis()));
    let mut state = ListState::default().with_selected(Some(list.cursor));
    frame.render_stateful_widget(widget, choices, &mut state);
    if list.choices.len() > usize::from(choices.height) {
        scrollbar(frame, bar, theme, list.choices.len(), list.cursor);
    }
    frame.render_widget(Paragraph::new(list_status(list, app, theme)), status);
    render_hints(frame, hint, app, theme);
}

fn list_status(list: &ListPopup, app: &App, theme: Theme) -> Line<'static> {
    match list.stage {
        ListStage::Choosing => {
            let ticked = list.chosen().len();
            let text = format!(
                "{ticked} ticked, {} songs",
                fmt::thousands(list.ticked_songs())
            );
            Line::from(numbered(&text, theme.body(), theme))
        }
        ListStage::Listing => busy("Listing the songs…", app, theme),
        ListStage::Ready { new, total } => {
            let text = format!("{} new of {}", fmt::thousands(new), fmt::thousands(total));
            Line::from(numbered(&text, theme.body(), theme))
        }
    }
}

fn render_link(frame: &mut Frame, area: Rect, app: &App, theme: Theme, input: &tui_input::Input) {
    let rect = centred(area, WIDE, 6);
    let inner = frame_popup(frame, rect, theme, "Download link");
    let [intro, field, _, hint] = Layout::vertical([Constraint::Length(1); 4]).areas(inner);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "Paste or type a link, then press Enter.",
            theme.secondary(),
        )),
        intro,
    );
    let [prompt, text] =
        Layout::horizontal([Constraint::Length(2), Constraint::Fill(1)]).areas(field);
    frame.render_widget(Paragraph::new(Span::styled("> ", theme.emphasis())), prompt);
    let scroll = input.visual_scroll(usize::from(text.width));
    let offset = u16::try_from(scroll).unwrap_or(0);
    frame.render_widget(
        Paragraph::new(input.value())
            .style(theme.body().patch(theme.selected()))
            .scroll((0, offset)),
        text,
    );
    let cursor = u16::try_from(input.visual_cursor().saturating_sub(scroll)).unwrap_or(0);
    frame.set_cursor_position((
        text.x
            .saturating_add(cursor)
            .min(text.right().saturating_sub(1)),
        text.y,
    ));
    render_hints(frame, hint, app, theme);
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
    let rect = centred(area, WIDE, 18);
    let title = format!(
        "{}: {} need attention",
        task_name(task),
        fmt::thousands(items.len())
    );
    let inner = frame_popup(frame, rect, theme, &title);
    let [body, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let [rows, bar] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(body);
    let list_items: Vec<ListItem> = items
        .iter()
        .map(|item| {
            let mut first = vec![Span::styled(item.title.clone(), theme.body())];
            if !item.artist.is_empty() {
                first.push(Span::styled(
                    format!(" · {}", item.artist),
                    theme.secondary(),
                ));
            }
            ListItem::new(vec![
                Line::from(first),
                Line::styled(format!("  {}", item.reason), theme.attention()),
            ])
        })
        .collect();
    let list = List::new(list_items)
        .highlight_style(theme.selected())
        .highlight_symbol(Line::styled("▶ ", theme.emphasis()));
    frame.render_stateful_widget(
        list,
        rows,
        &mut ListState::default().with_selected(Some(cursor)),
    );
    scrollbar(frame, bar, theme, items.len(), cursor);
    render_hints(frame, hint, app, theme);
}

fn help_lines(theme: Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for group in HELP {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(group.title, theme.heading()));
        for (key, what) in group.keys {
            lines.push(Line::from(vec![
                Span::styled(format!("{key:<KEY_WIDTH$}"), theme.emphasis()),
                Span::styled(*what, theme.body()),
            ]));
        }
    }
    lines
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
