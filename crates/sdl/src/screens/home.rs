//! Home screen: Sync Spotify, Download link (text input), Sync card, Quit.

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, HighlightSpacing, List, ListState, Paragraph};
use tui_input::backend::crossterm::EventHandler;

use super::{BORDERS, Hint, hints_and_body};
use crate::app::{Action, App};

/// Menu entries in display order.
const MENU: [&str; 4] = ["Sync Spotify", "Download link", "Sync card", "Quit"];
/// Index of the Sync Spotify entry.
const SYNC_SPOTIFY: usize = 0;
/// Index of the Download link entry.
const LINK: usize = 1;
/// Index of the Sync card entry.
const SYNC_CARD: usize = 2;
/// Index of the Quit entry.
const QUIT: usize = 3;
/// Marker before the highlighted menu entry.
const MENU_MARKER: &str = "> ";
/// Text rows inside the link input box.
const INPUT_ROWS: u16 = 1;
/// Column kept free at the end of the input so the cursor never sits on the border (the layout
/// tui-input's own example uses).
const CURSOR_COLUMNS: u16 = 1;

/// Key hints while a menu entry other than Download link is highlighted.
const MENU_HINTS: &[Hint] = &[("Up/Down", "move"), ("Enter", "start"), ("q/Esc", "quit")];
/// Key hints while Download link is highlighted (typing goes into the link field).
const LINK_HINTS: &[Hint] = &[
    ("type or paste", "a link"),
    ("Enter", "download"),
    ("Up/Down", "move"),
    ("Esc", "quit"),
];

/// Draw the Home screen.
pub fn render(app: &App, frame: &mut Frame) {
    let focused = app.home_selected == LINK;
    let body = hints_and_body(frame, if focused { LINK_HINTS } else { MENU_HINTS });
    let menu_rows = u16::try_from(MENU.len()).unwrap_or(u16::MAX);
    let [menu_area, input_area] = Layout::vertical([
        Constraint::Length(menu_rows.saturating_add(BORDERS)),
        Constraint::Length(INPUT_ROWS.saturating_add(BORDERS)),
    ])
    .areas(body);

    // A stateful list scrolls to the highlighted entry, so it stays visible in a short terminal.
    let mut menu_state = ListState::default().with_selected(Some(app.home_selected));
    frame.render_stateful_widget(
        List::new(MENU)
            .block(Block::bordered().title(" sdl "))
            .highlight_symbol(MENU_MARKER)
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_style(Style::new().add_modifier(Modifier::BOLD)),
        menu_area,
        &mut menu_state,
    );

    let inner_width = input_area
        .width
        .saturating_sub(BORDERS)
        .saturating_sub(CURSOR_COLUMNS);
    let scroll = app.link_input.visual_scroll(usize::from(inner_width));
    let scroll_u16 = u16::try_from(scroll).unwrap_or(0);
    let block = Block::bordered().title(" Link (track or playlist) ");
    let block = if focused {
        block.border_style(Style::new().add_modifier(Modifier::BOLD))
    } else {
        block
    };
    frame.render_widget(
        Paragraph::new(app.link_input.value())
            .scroll((0, scroll_u16))
            .block(block),
        input_area,
    );
    if focused {
        // One column in from the input box's left border.
        let cursor = u16::try_from(app.link_input.visual_cursor().saturating_sub(scroll))
            .unwrap_or(0)
            .saturating_add(1);
        frame.set_cursor_position(Position::new(
            input_area.x.saturating_add(cursor),
            input_area.y.saturating_add(1),
        ));
    }
}

/// Handle a key on the Home screen.
pub fn on_key(app: &mut App, key: KeyEvent) -> Action {
    let on_link = app.home_selected == LINK;
    match key.code {
        KeyCode::Down | KeyCode::Tab => {
            app.home_selected = step(app.home_selected, true);
            Action::None
        }
        KeyCode::Up | KeyCode::BackTab => {
            app.home_selected = step(app.home_selected, false);
            Action::None
        }
        KeyCode::Esc => {
            app.quit = true;
            Action::None
        }
        KeyCode::Char('q') if !on_link && key.modifiers.is_empty() => {
            app.quit = true;
            Action::None
        }
        KeyCode::Enter => activate(app),
        _ if on_link => {
            app.link_input.handle_event(&TermEvent::Key(key));
            Action::None
        }
        _ => Action::None,
    }
}

fn step(selected: usize, forward: bool) -> usize {
    if forward {
        if selected >= QUIT {
            0
        } else {
            selected.saturating_add(1)
        }
    } else if selected == 0 {
        QUIT
    } else {
        selected.saturating_sub(1)
    }
}

fn activate(app: &mut App) -> Action {
    match app.home_selected {
        SYNC_SPOTIFY => {
            app.begin_run("Starting Spotify sync");
            Action::SyncSpotify
        }
        LINK => {
            let link = app.link_input.value().trim().to_owned();
            if link.is_empty() {
                return Action::None;
            }
            app.link_input.reset();
            app.begin_run("Starting link download");
            Action::DownloadLink(link)
        }
        SYNC_CARD => {
            app.begin_run("Starting card sync");
            Action::SyncCard
        }
        _ => {
            app.quit = true;
            Action::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::Screen;
    use crossterm::event::KeyModifiers;
    use sdl_core::Config;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn app() -> Result<App, Box<dyn std::error::Error>> {
        let cfg = Config::parse(
            "client_id = \"a\"\ncard_uuid = \"u\"\nmusic_root = \"/m\"\n",
            Path::new("/c.toml"),
        )?;
        Ok(App::new(cfg))
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn selection_wraps() {
        assert_eq!(step(3, true), 0);
        assert_eq!(step(0, false), 3);
        assert_eq!(step(1, true), 2);
    }

    #[test]
    fn enter_on_sync_starts_run() -> TestResult {
        let mut app = app()?;
        assert_eq!(app.on_key(press(KeyCode::Enter)), Action::SyncSpotify);
        assert_eq!(app.screen, Screen::Run);
        assert!(app.running);
        Ok(())
    }

    #[test]
    fn link_needs_text_and_q_types_there() -> TestResult {
        let mut app = app()?;
        app.on_key(press(KeyCode::Down));
        assert_eq!(app.on_key(press(KeyCode::Enter)), Action::None);
        app.on_key(press(KeyCode::Char('q')));
        assert!(!app.quit);
        assert_eq!(app.link_input.value(), "q");
        assert_eq!(
            app.on_key(press(KeyCode::Enter)),
            Action::DownloadLink("q".into())
        );
        assert_eq!(app.link_input.value(), "");
        Ok(())
    }

    #[test]
    fn q_and_esc_quit_from_menu() -> TestResult {
        let mut app = app()?;
        app.on_key(press(KeyCode::Char('q')));
        assert!(app.quit);
        let mut app = self::app()?;
        app.on_key(press(KeyCode::Esc));
        assert!(app.quit);
        Ok(())
    }

    #[test]
    fn ctrl_c_quits_even_in_input() -> TestResult {
        let mut app = app()?;
        app.on_key(press(KeyCode::Down));
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit);
        Ok(())
    }
}
