//! Every key, once: the help popup lists this table (F1).

/// One block of the help popup.
#[derive(Debug, Clone, Copy)]
pub struct KeyGroup {
    /// Where the keys apply.
    pub title: &'static str,
    /// Key name and what it does.
    pub keys: &'static [(&'static str, &'static str)],
}

/// All keys, grouped by where they apply. Short enough to read at 40 columns.
pub const HELP: &[KeyGroup] = &[
    KeyGroup {
        title: "Main screen",
        keys: &[
            ("↑↓ j k", "choose a function"),
            ("Enter", "run it"),
            ("s", "stop it while it runs"),
            ("a", "what needs attention"),
            ("?", "this help"),
            ("q, Ctrl-C", "quit"),
        ],
    },
    KeyGroup {
        title: "Popups",
        keys: &[
            ("↑↓ j k", "move or scroll"),
            ("PgUp PgDn", "scroll a page"),
            ("Home, End", "go to the top or end"),
            ("Space", "tick or untick a list"),
            ("Ctrl-U", "clear the link field"),
            ("Enter, y", "continue, start, or yes"),
            ("Esc, n", "cancel, close, or no"),
            ("q", "close a list or help"),
        ],
    },
    KeyGroup {
        title: "Sync card",
        keys: &[
            ("Esc", "plan: unmount and cancel"),
            ("n, Esc", "last step: leave mounted"),
        ],
    },
];

/// Width of the key column in the help popup.
pub const KEY_WIDTH: usize = 11;

/// Number of lines the help text takes: a title and its keys per group, with a blank line between
/// groups.
#[must_use]
pub fn help_line_count() -> usize {
    HELP.iter()
        .map(|g| g.keys.len().saturating_add(2))
        .fold(0, usize::saturating_add)
        .saturating_sub(1)
}
