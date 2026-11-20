//! Every key and mouse action, once (F1). The help popup and the footer are built from the
//! entries here, so a key is named and described in one place. The form's own footer hints come
//! from `sdl_form::Form::hints`.

use sdl_form::Hint;

use crate::app::{App, Popup};

/// One key or mouse action as the help popup shows it.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// The key or mouse action (`↑↓ j k`, `click`).
    pub key: &'static str,
    /// What it does.
    pub what: &'static str,
}

const fn entry(key: &'static str, what: &'static str) -> Entry {
    Entry { key, what }
}

/// A block of the help popup.
#[derive(Debug, Clone, Copy)]
pub struct Group {
    /// Where the keys apply.
    pub title: &'static str,
    /// Its entries.
    pub entries: &'static [Entry],
}

/// All keys and mouse actions, grouped by where they apply. Short enough to read at 40 columns.
pub const HELP: &[Group] = &[
    Group {
        title: "Main screen",
        entries: &[
            entry("↑↓ j k", "choose a function"),
            entry("Enter", "run it"),
            entry("s", "stop it while it runs"),
            entry("a", "what needs attention"),
            entry("?", "this help"),
            entry("q, Ctrl-C", "quit"),
        ],
    },
    Group {
        title: "Forms",
        entries: &[
            entry("Tab, Shift-Tab", "next, previous field"),
            entry("↑↓", "move inside a field"),
            entry("Space", "tick, switch, open"),
            entry("type", "filter a list"),
            entry("Enter", "continue, then start"),
            entry("Esc", "clear filter, or cancel"),
            entry("r", "look for devices again"),
            entry("o", "open candidate in browser"),
        ],
    },
    Group {
        title: "Lists and questions",
        entries: &[
            entry("↑↓ PgUp PgDn", "scroll"),
            entry("Home End", "top, end"),
            entry("y n", "yes, no"),
            entry("Esc q", "close"),
        ],
    },
    Group {
        title: "Mouse",
        entries: &[
            entry("click", "act on what is under it"),
            entry("wheel", "scroll the list under it"),
            entry("drag", "move a scrollbar"),
        ],
    },
];

/// The footer's hints for what is on screen now: the open form's, or the main screen's.
#[must_use]
pub fn footer(app: &App) -> Vec<Hint> {
    const fn hint(key: &'static str, what: &'static str) -> Hint {
        Hint { key, what }
    }
    match app.top() {
        Some(Popup::Dialog(dialog)) => dialog.hints(),
        Some(Popup::Confirm(_)) => vec![hint("y", "yes"), hint("n", "no")],
        Some(Popup::Waiting(_)) => vec![hint("Esc", "cancel")],
        Some(_) => vec![hint("↑↓", "scroll"), hint("Esc", "close")],
        None => {
            let mut hints = vec![hint("↑↓", "choose"), hint("Enter", "run")];
            if app
                .view(app.selected)
                .is_some_and(crate::app::FunctionView::is_running)
            {
                hints.push(hint("s", "stop"));
            }
            hints.extend([hint("a", "attention"), hint("?", "help"), hint("q", "quit")]);
            hints
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_fits_forty_columns() {
        for group in HELP {
            for entry in group.entries {
                assert!(entry.key.chars().count() <= 14, "{}", entry.key);
                assert!(
                    14_usize.saturating_add(entry.what.chars().count()) <= 40,
                    "{}",
                    entry.what
                );
            }
        }
    }
}
