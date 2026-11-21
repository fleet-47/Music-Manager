//! Every key and mouse action, once (F1). The footer and the help popup are both read from
//! [`BINDINGS`], so a key is named and described in one place. A form's own footer hints come
//! from `sdl_form::Form::hints` (the form knows its own keys); the help lists them under "Forms".

use sdl_form::Hint;

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
    /// How the footer shows it, when it does.
    pub foot: Option<Hint>,
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
        self.foot = Some(Hint { key, what });
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

/// All keys and mouse actions. The order is the footer's, and the help's.
pub const BINDINGS: &[Binding] = &[
    bind(Scope::Main, "Enter", "run it").foot("Enter", "run"),
    bind(Scope::Main, "↑↓ j k", "choose a function").foot("↑↓", "choose"),
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
    bind(Scope::Form, "r", "find devices again"),
    bind(Scope::Form, "o", "open match in browser"),
    bind(Scope::Scroll, "↑↓ j k", "scroll").foot("↑↓", "scroll"),
    bind(Scope::Scroll, "PgUp PgDn", "page up, page down"),
    bind(Scope::Scroll, "Home End", "top, end"),
    bind(Scope::Scroll, "Esc q Enter", "close").foot("Esc", "close"),
    bind(Scope::Notice, "Esc q Enter", "close").foot("Esc", "close"),
    bind(Scope::Question, "y, Enter", "yes").foot("y", "yes"),
    bind(Scope::Question, "n, Esc", "no").foot("n", "no"),
    bind(Scope::Wait, "Esc", "cancel the sign-in")
        .foot("Esc", "cancel")
        .when(signing_in),
    bind(Scope::Wait, "Esc", "hide; eject goes on")
        .foot("Esc", "hide")
        .when(ejecting),
    bind(Scope::Mouse, "click", "press what is there"),
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
        Some(Popup::Error(_)) => Scope::Notice,
        Some(Popup::Confirm(_)) => Scope::Question,
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
    if let Some(Popup::Dialog(dialog)) = app.top() {
        return dialog.hints();
    }
    let here = scope(app);
    BINDINGS
        .iter()
        .filter(|binding| binding.scope == here && binding.when.is_none_or(|holds| holds(app)))
        .filter_map(|binding| binding.foot)
        .collect()
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
        shown.push(*hint);
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
        assert_eq!(keys(&app), ["Enter", "↑↓", "a", "?", "q"]);
        let clock = app.clock;
        let _ = app.update(
            Msg::Task(Event {
                task: TaskId::SyncSpotify,
                kind: EventKind::Started,
            }),
            clock,
        );
        assert_eq!(keys(&app), ["Enter", "↑↓", "s", "a", "?", "q"]);
    }

    #[test]
    fn each_popup_footer_says_how_to_answer_or_leave() {
        let mut app = app();
        app.popups.push(Popup::Confirm(Confirm::Quit));
        assert_eq!(keys(&app), ["y", "n"]);
        app.popups.push(Popup::Waiting(Waiting::Browser));
        assert_eq!(keys(&app), ["Esc"]);
        app.popups
            .push(Popup::Waiting(Waiting::Eject("DISC".into())));
        assert_eq!(keys(&app), ["Esc"]);
        app.popups.push(Popup::Help { scroll: 0 });
        assert_eq!(keys(&app), ["↑↓", "Esc"]);
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
