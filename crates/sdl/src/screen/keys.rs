//! Every key and mouse action, once (F1). The footer and the help popup are both read from
//! [`BINDINGS`], so a key is named and described in one place. A form's own footer hints come
//! from `sdl_form::Form::hints` (the form knows its own keys); the help lists them under "Forms".
//! A question's footer is its own two answers ([`super::popups::question`]), so a button and its
//! hint always say the same. Every footer hint is also a button: a click on it is its key
//! ([`key_of`]), which gives every key that has a hint a mouse action.

use crossterm::event::KeyCode;
use sdl_form::Hint;

use super::popups::{extent, question};
use crate::app::{App, FunctionView, Popup, Waiting};

/// Where a binding applies; also the help popup's block title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The main screen.
    Main,
    /// Inside a form (its footer comes from the form).
    Form,
    /// Help and the attention list.
    Scroll,
    /// A message popup.
    Notice,
    /// A yes-or-no question.
    Question,
    /// A popup that waits: for the browser, or for an eject.
    Wait,
    /// The mouse.
    Mouse,
}

impl Scope {
    /// The help block's title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Main => "Main screen",
            Self::Form => "Forms",
            Self::Scroll => "Long lists",
            Self::Notice => "Messages",
            Self::Question => "Questions",
            Self::Wait => "Waiting",
            Self::Mouse => "Mouse",
        }
    }

    /// The scopes in the order help lists them.
    pub const ALL: [Self; 7] = [
        Self::Main,
        Self::Form,
        Self::Scroll,
        Self::Notice,
        Self::Question,
        Self::Wait,
        Self::Mouse,
    ];
}

/// One key or mouse action.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    /// Where it applies.
    pub scope: Scope,
    /// The keys, as help shows them (at most 13 columns, so a space always follows).
    pub key: &'static str,
    /// What it does, in plain words (at most 21 columns).
    pub what: &'static str,
    /// How the footer shows it, when it does: its key and words.
    pub foot: Option<(&'static str, &'static str)>,
    /// The footer shows it only while this holds.
    pub when: Option<fn(&App) -> bool>,
}

const fn bind(scope: Scope, key: &'static str, what: &'static str) -> Binding {
    Binding {
        scope,
        key,
        what,
        foot: None,
        when: None,
    }
}

impl Binding {
    const fn foot(mut self, key: &'static str, what: &'static str) -> Self {
        self.foot = Some((key, what));
        self
    }

    const fn when(mut self, when: fn(&App) -> bool) -> Self {
        self.when = Some(when);
        self
    }
}

fn selected_runs(app: &App) -> bool {
    app.view(app.selected).is_some_and(FunctionView::is_running)
}

/// Enter runs the chosen function only when it is not already open or running.
fn selected_can_run(app: &App) -> bool {
    app.view(app.selected).is_some_and(|view| !view.is_open())
}

/// Whether the scrolling popup on top has more than it shows.
fn scrolls(app: &App) -> bool {
    app.top()
        .and_then(|top| extent(app, top, app.size))
        .is_some_and(|extent| extent.max_scroll() > 0)
}

/// All keys and mouse actions. The order is the footer's, and the help's.
pub const BINDINGS: &[Binding] = &[
    bind(Scope::Main, "Enter", "run it")
        .foot("Enter", "run")
        .when(selected_can_run),
    bind(Scope::Main, "←→↑↓ hjkl", "choose a function").foot("←→↑↓", "choose"),
    bind(Scope::Main, "i", "install missing tools"),
    bind(Scope::Main, "s", "stop it while it runs")
        .foot("s", "stop")
        .when(selected_runs),
    bind(Scope::Main, "a", "what needs attention").foot("a", "attention"),
    bind(Scope::Main, "?", "this help").foot("?", "help"),
    bind(Scope::Main, "q, Ctrl-C", "quit").foot("q", "quit"),
    bind(Scope::Form, "Tab Shift-Tab", "next, previous field"),
    bind(Scope::Form, "↑↓", "move inside a field"),
    bind(Scope::Form, "Space", "tick, switch, open"),
    bind(Scope::Form, "type", "filter a list"),
    bind(Scope::Form, "Enter", "continue, then start"),
    bind(Scope::Form, "Esc", "clear filter / cancel"),
    bind(Scope::Form, "r", "look for devices"),
    bind(Scope::Form, "o", "open in browser"),
    bind(Scope::Scroll, "↑↓ j k", "scroll")
        .foot("↑↓", "scroll")
        .when(scrolls),
    bind(Scope::Scroll, "PgUp PgDn", "page up, page down"),
    bind(Scope::Scroll, "Home End", "top, end"),
    bind(Scope::Scroll, "Esc q Enter", "close").foot("Esc", "close"),
    bind(Scope::Notice, "Esc q Enter", "close").foot("Esc", "close"),
    bind(Scope::Question, "y", "yes"),
    bind(Scope::Question, "n, Esc", "no"),
    bind(Scope::Question, "←→ Tab", "the other answer"),
    bind(Scope::Question, "Enter", "the highlighted one"),
    bind(Scope::Wait, "Esc", "cancel the sign-in")
        .foot("Esc", "cancel")
        .when(signing_in),
    bind(Scope::Wait, "Esc", "hide; eject goes on")
        .foot("Esc", "hide")
        .when(ejecting),
    bind(Scope::Mouse, "click", "press what is there"),
    bind(Scope::Mouse, "click a key", "same as the key"),
    bind(Scope::Mouse, "second click", "run the chosen one"),
    bind(Scope::Mouse, "wheel", "scroll the list"),
    bind(Scope::Mouse, "drag", "move a scrollbar"),
];

/// The scope of what is on top of the screen now.
fn scope(app: &App) -> Scope {
    match app.top() {
        None => Scope::Main,
        Some(Popup::Dialog(_)) => Scope::Form,
        Some(Popup::Help { .. } | Popup::Attention { .. }) => Scope::Scroll,
        Some(Popup::Error(_) | Popup::Ejected(_)) => Scope::Notice,
        Some(Popup::Confirm(..)) => Scope::Question,
        Some(Popup::Waiting(_)) => Scope::Wait,
    }
}

fn signing_in(app: &App) -> bool {
    matches!(app.top(), Some(Popup::Waiting(Waiting::Browser)))
}

fn ejecting(app: &App) -> bool {
    matches!(app.top(), Some(Popup::Waiting(Waiting::Eject(_))))
}

/// The footer's hints for what is on screen now: the open form's, or the table's for the scope.
#[must_use]
pub fn footer(app: &App) -> Vec<Hint> {
    match app.top() {
        Some(Popup::Dialog(dialog)) => return dialog.hints(),
        Some(Popup::Confirm(confirm, focused)) => {
            let words = question(confirm);
            let answer = |key, label: &str| Hint {
                key,
                what: label.to_lowercase().into(),
            };
            let enter = if *focused { words.yes } else { words.no };
            // The focus keys and Enter first: a narrow footer keeps the last two, and `y` and
            // `n` are the ones that must never go.
            return vec![
                Hint::new("←→", "choose"),
                answer("Enter", enter),
                answer("y", words.yes),
                answer("n", words.no),
            ];
        }
        _ => {}
    }
    let here = scope(app);
    BINDINGS
        .iter()
        .filter(|binding| binding.scope == here && binding.when.is_none_or(|holds| holds(app)))
        .filter_map(|binding| binding.foot.map(|(key, what)| Hint::new(key, what)))
        .collect()
}

/// The key a click on a footer hint presses; `None` for hints that name a kind of key (`↑↓`,
/// `type`) rather than one key.
#[must_use]
pub fn key_of(hint: &Hint) -> Option<KeyCode> {
    match hint.key {
        "Enter" => Some(KeyCode::Enter),
        "Esc" => Some(KeyCode::Esc),
        "Tab" => Some(KeyCode::Tab),
        "Space" => Some(KeyCode::Char(' ')),
        "→" => Some(KeyCode::Right),
        "←" => Some(KeyCode::Left),
        key => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_graphic() => Some(KeyCode::Char(c)),
                _ => None,
            }
        }
    }
}

/// Columns between footer hints.
pub const GAP: usize = 3;

/// How many columns a hint takes in the footer, with the gap after it.
fn width(hint: &Hint) -> usize {
    hint.key
        .chars()
        .count()
        .saturating_add(1)
        .saturating_add(hint.what.chars().count())
        .saturating_add(GAP)
}

fn sum_width(hints: &[Hint]) -> usize {
    hints.iter().map(width).fold(0, usize::saturating_add)
}

/// The hints that fit in `columns`. When not all fit, the last two stay (the way out and the way
/// to help) and the middle ones go, so the footer never ends in half a word.
#[must_use]
pub fn fit(hints: &[Hint], columns: u16) -> Vec<Hint> {
    let room = usize::from(columns).saturating_add(GAP);
    if sum_width(hints) <= room {
        return hints.to_vec();
    }
    let keep = hints.len().saturating_sub(2);
    let (front, back) = hints.split_at(keep);
    let mut used = sum_width(back);
    let mut shown: Vec<Hint> = Vec::new();
    for hint in front {
        used = used.saturating_add(width(hint));
        if used > room {
            break;
        }
        shown.push(hint.clone());
    }
    shown.extend_from_slice(back);
    shown
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::layout::Size;
    use sdl_core::{Event, EventKind, TaskId};

    use super::*;
    use crate::app::{Clock, Confirm, Msg, Settings};

    fn app() -> App {
        let clock = Clock {
            instant: Instant::now(),
            epoch: 0,
        };
        App::new(Settings::default(), false, Size::new(100, 30), clock)
    }

    fn keys(app: &App) -> Vec<&'static str> {
        footer(app).iter().map(|hint| hint.key).collect()
    }

    #[test]
    fn help_fits_forty_columns() {
        for binding in BINDINGS {
            assert!(binding.key.chars().count() <= 13, "{}", binding.key);
            // 14 columns for the key and its space; the popup's text is 35 wide at 40 columns.
            assert!(binding.what.chars().count() <= 21, "{}", binding.what);
        }
    }

    #[test]
    fn every_scope_has_help() {
        for scope in Scope::ALL {
            assert!(BINDINGS.iter().any(|binding| binding.scope == scope));
        }
    }

    #[test]
    fn the_main_footer_offers_stop_only_for_a_running_function() {
        let mut app = app();
        assert_eq!(keys(&app), ["Enter", "←→↑↓", "a", "?", "q"]);
        let clock = app.clock;
        let _ = app.update(
            Msg::Task(Event {
                task: TaskId::SyncSpotify,
                kind: EventKind::Started,
            }),
            clock,
        );
        // It runs: Enter would do nothing, so it is not offered; Stop is.
        assert_eq!(keys(&app), ["←→↑↓", "s", "a", "?", "q"]);
    }

    #[test]
    fn each_popup_footer_says_how_to_answer_or_leave() {
        let mut app = app();
        app.popups.push(Popup::ask(Confirm::Quit));
        assert_eq!(keys(&app), ["←→", "Enter", "y", "n"]);
        let enter = |app: &App| footer(app).get(1).map(|hint| hint.what.to_string());
        assert_eq!(enter(&app).as_deref(), Some("keep going"));
        // Enter is named for the button that has the focus.
        if let Some(Popup::Confirm(_, yes)) = app.popups.last_mut() {
            *yes = true;
        }
        assert_eq!(enter(&app).as_deref(), Some("quit"));
        app.popups.push(Popup::Waiting(Waiting::Browser));
        assert_eq!(keys(&app), ["Esc"]);
        app.popups
            .push(Popup::Waiting(Waiting::Eject("DISC".into())));
        assert_eq!(keys(&app), ["Esc"]);
        app.popups.push(Popup::Help { scroll: 0 });
        assert_eq!(keys(&app), ["↑↓", "Esc"]);
        // An attention list with nothing to scroll offers only the way out.
        app.popups.push(Popup::Attention {
            task: sdl_core::TaskId::Convert,
            scroll: 0,
        });
        assert_eq!(keys(&app), ["Esc"]);
    }

    #[test]
    fn a_click_on_a_hint_is_its_key() {
        let key = |key, what| key_of(&Hint::new(key, what));
        assert_eq!(key("Enter", "start"), Some(KeyCode::Enter));
        assert_eq!(key("r", "look for devices"), Some(KeyCode::Char('r')));
        assert_eq!(key("?", "help"), Some(KeyCode::Char('?')));
        assert_eq!(key("↑↓", "choose"), None);
        assert_eq!(key("type", "filter"), None);
    }

    #[test]
    fn a_narrow_footer_drops_the_middle_and_keeps_help_and_quit() {
        let hints = footer(&app());
        assert_eq!(fit(&hints, 100).len(), hints.len());
        let narrow = fit(&hints, 24);
        let names: Vec<_> = narrow.iter().map(|hint| hint.key).collect();
        assert_eq!(names.last(), Some(&"q"));
        assert!(names.contains(&"?"));
        assert!(sum_width(&narrow) <= 24 + GAP, "{narrow:?}");
    }
}
