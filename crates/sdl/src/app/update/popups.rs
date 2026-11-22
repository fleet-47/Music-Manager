//! Plain popups: the questions, the waiting popup, errors, help and the attention list. Keys and
//! clicks that mean the same thing reach the same method here.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Size;
use sdl_core::TaskId;
use sdl_tasks::Command;

use crate::app::{App, Cmd, Confirm, Popup, Waiting};
use crate::screen::popups::{Extent, extent};

/// Where a scroll key or click moves the list.
#[derive(Debug, Clone, Copy)]
pub(super) enum Scroll {
    /// Up or down by this many lines.
    By(isize),
    /// A page up or down.
    Page(isize),
    /// To this wrapped line.
    To(usize),
    /// To the end.
    End,
}

impl App {
    /// A key for the popup on top, which is not a dialog.
    pub(super) fn on_popup_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let Some(top) = self.popups.last() else {
            return Vec::new();
        };
        match top {
            Popup::Confirm(_, focused) => {
                let focused = *focused;
                match key.code {
                    KeyCode::Char('y' | 'Y') => self.answer(true),
                    KeyCode::Char('n' | 'N') | KeyCode::Esc => self.answer(false),
                    KeyCode::Enter => self.answer(focused),
                    // Two buttons: any of these moves the focus to the other one.
                    KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Tab
                    | KeyCode::BackTab
                    | KeyCode::Char('h' | 'l') => {
                        if let Some(Popup::Confirm(_, yes)) = self.popups.last_mut() {
                            *yes = !focused;
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            Popup::Waiting(Waiting::Browser) if key.code == KeyCode::Esc => self.answer(false),
            Popup::Waiting(Waiting::Eject(_)) if key.code == KeyCode::Esc => {
                self.popups.pop();
                Vec::new()
            }
            Popup::Error(_) | Popup::Ejected(_) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter) {
                    self.popups.pop();
                }
                Vec::new()
            }
            Popup::Help { .. } | Popup::Attention { .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                    self.popups.pop();
                    Vec::new()
                }
                KeyCode::Up | KeyCode::Char('k') => self.scroll(Scroll::By(-1)),
                KeyCode::Down | KeyCode::Char('j') => self.scroll(Scroll::By(1)),
                KeyCode::PageUp => self.scroll(Scroll::Page(-1)),
                KeyCode::PageDown => self.scroll(Scroll::Page(1)),
                KeyCode::Home => self.scroll(Scroll::To(0)),
                KeyCode::End => self.scroll(Scroll::End),
                _ => Vec::new(),
            },
            Popup::Waiting(_) | Popup::Dialog(_) => Vec::new(),
        }
    }

    /// Yes or no (a key, or a click on the button) to the question on top; "no" is also Cancel on
    /// the waiting popup.
    pub(super) fn answer(&mut self, yes: bool) -> Vec<Cmd> {
        match self.popups.pop() {
            Some(Popup::Confirm(Confirm::Stop(task), _)) => {
                if yes {
                    vec![Cmd::Stop(task)]
                } else {
                    Vec::new()
                }
            }
            Some(Popup::Confirm(Confirm::Quit, _)) => {
                if yes {
                    self.quit_now()
                } else {
                    Vec::new()
                }
            }
            Some(Popup::Confirm(Confirm::Eject { device }, _)) => {
                if yes {
                    self.popups.push(Popup::Waiting(Waiting::Eject(device)));
                    vec![Cmd::Eject]
                } else {
                    Vec::new()
                }
            }
            Some(Popup::Confirm(Confirm::Browser, _)) => {
                if yes {
                    vec![Cmd::send(TaskId::SyncSpotify, Command::Go)]
                } else {
                    self.cancel_sign_in()
                }
            }
            Some(Popup::Waiting(Waiting::Browser)) if !yes => self.cancel_sign_in(),
            Some(other) => {
                self.popups.push(other);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// The extent of the scrolling popup on top, with its first line shown.
    pub(super) fn scrolled(&self) -> Option<(Extent, usize)> {
        let top = self.popups.last()?;
        let at = match top {
            Popup::Help { scroll } | Popup::Attention { scroll, .. } => *scroll,
            _ => return None,
        };
        let size = Size::new(self.size.width, self.size.height);
        extent(self, top, size).map(|extent| (extent, at))
    }

    /// Move the scrolling popup on top; never past its content.
    pub(super) fn scroll(&mut self, to: Scroll) -> Vec<Cmd> {
        let Some((extent, at)) = self.scrolled() else {
            return Vec::new();
        };
        let max = extent.max_scroll();
        let page = extent.viewport.saturating_sub(1).max(1);
        let at = at.min(max);
        let next = match to {
            Scroll::By(n) => step(at, n.unsigned_abs(), n < 0),
            Scroll::Page(n) => step(at, page, n < 0),
            Scroll::To(line) => line,
            Scroll::End => max,
        }
        .min(max);
        if let Some(Popup::Help { scroll } | Popup::Attention { scroll, .. }) =
            self.popups.last_mut()
        {
            *scroll = next;
        }
        Vec::new()
    }
}

fn step(at: usize, by: usize, up: bool) -> usize {
    if up {
        at.saturating_sub(by)
    } else {
        at.saturating_add(by)
    }
}

#[cfg(test)]
mod tests {
    use sdl_core::{Attention, EventKind, LastRun, RunOutcome};

    use super::super::tests::{app, clock, key, running, task};
    use super::*;

    fn with_items(app: &mut App, count: usize) {
        let items = (0..count)
            .map(|n| {
                Attention::held(
                    format!("Song {n}"),
                    "Artist".to_owned(),
                    "No YouTube Music match was close enough, so nothing was downloaded."
                        .to_owned(),
                )
            })
            .collect();
        app.settings.state.last_run.insert(
            TaskId::SyncSpotify,
            LastRun {
                line: "x".into(),
                at: 0,
                outcome: RunOutcome::NeedsAttention,
                attention: items,
            },
        );
    }

    fn scroll_of(app: &App) -> Option<usize> {
        match app.top() {
            Some(Popup::Help { scroll } | Popup::Attention { scroll, .. }) => Some(*scroll),
            _ => None,
        }
    }

    #[test]
    fn scrolling_keys_stay_inside_the_content() {
        let mut app = app();
        with_items(&mut app, 12);
        let _ = app.update(key(KeyCode::Char('a')), clock());
        assert_eq!(scroll_of(&app), Some(0));
        let _ = app.update(key(KeyCode::Up), clock());
        assert_eq!(scroll_of(&app), Some(0));
        let _ = app.update(key(KeyCode::End), clock());
        let end = scroll_of(&app).unwrap_or(0);
        assert!(end > 0);
        let _ = app.update(key(KeyCode::Down), clock());
        let _ = app.update(key(KeyCode::PageDown), clock());
        assert_eq!(scroll_of(&app), Some(end));
        // Up answers at once: the position never ran past the end.
        let _ = app.update(key(KeyCode::Up), clock());
        assert_eq!(scroll_of(&app), Some(end.saturating_sub(1)));
        let _ = app.update(key(KeyCode::Home), clock());
        assert_eq!(scroll_of(&app), Some(0));
        let _ = app.update(key(KeyCode::PageDown), clock());
        assert!(scroll_of(&app).is_some_and(|at| at > 1));
    }

    #[test]
    fn an_empty_attention_list_says_so_and_does_not_scroll() {
        let mut app = app();
        let _ = app.update(key(KeyCode::Char('a')), clock());
        let _ = app.update(key(KeyCode::End), clock());
        assert_eq!(scroll_of(&app), Some(0));
    }

    #[test]
    fn the_attention_list_of_a_running_function_is_its_current_run() {
        let mut app = app();
        with_items(&mut app, 1);
        running(&mut app, TaskId::SyncSpotify);
        let one = EventKind::Attention(Attention::held("A".into(), String::new(), "r".into()));
        let _ = task(&mut app, TaskId::SyncSpotify, one);
        let top = Popup::Attention {
            task: TaskId::SyncSpotify,
            scroll: 0,
        };
        let size = Size::new(100, 30);
        let current = extent(&app, &top, size).map(|each| each.items.len());
        assert_eq!(current, Some(1));
    }

    #[test]
    fn enter_gives_the_safe_answer_where_yes_does_damage() {
        let mut app = app();
        running(&mut app, TaskId::Convert);
        for question in [
            Confirm::Stop(TaskId::Convert),
            Confirm::Quit,
            Confirm::Eject {
                device: "DISC".into(),
            },
        ] {
            app.popups.push(Popup::ask(question));
            assert_eq!(app.update(key(KeyCode::Enter), clock()), Vec::new());
            assert!(app.popups.is_empty() && !app.quit);
        }
        app.popups.push(Popup::ask(Confirm::Browser));
        assert_eq!(
            app.update(key(KeyCode::Enter), clock()),
            vec![Cmd::send(TaskId::SyncSpotify, Command::Go)]
        );
    }

    #[test]
    fn the_focus_moves_between_the_two_answers_and_enter_presses_the_focused_one() {
        let mut app = app();
        running(&mut app, TaskId::Convert);
        for move_focus in [
            KeyCode::Tab,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::BackTab,
        ] {
            app.popups.push(Popup::ask(Confirm::Stop(TaskId::Convert)));
            assert_eq!(app.update(key(move_focus), clock()), Vec::new());
            assert!(matches!(app.top(), Some(Popup::Confirm(_, true))));
            assert_eq!(
                app.update(key(KeyCode::Enter), clock()),
                vec![Cmd::Stop(TaskId::Convert)]
            );
        }
        // Moving twice is back on the safe answer.
        app.popups.push(Popup::ask(Confirm::Stop(TaskId::Convert)));
        let _ = app.update(key(KeyCode::Tab), clock());
        let _ = app.update(key(KeyCode::Tab), clock());
        assert_eq!(app.update(key(KeyCode::Enter), clock()), Vec::new());
        assert!(app.popups.is_empty());
    }

    #[test]
    fn a_question_ignores_other_keys_and_keeps_asking() {
        let mut app = app();
        app.popups.push(Popup::ask(Confirm::Quit));
        assert_eq!(app.update(key(KeyCode::Char('x')), clock()), Vec::new());
        assert_eq!(app.update(key(KeyCode::Char('q')), clock()), Vec::new());
        assert_eq!(app.popups.len(), 1);
    }
}
