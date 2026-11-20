//! Every key, once (F1). The help popup and every key hint on screen are built from the entries
//! here, so a key is named and described in one place.

/// One key as the screen shows it.
#[derive(Debug, Clone, Copy)]
pub struct Key {
    /// Name in the footer and in hint lines.
    pub key: &'static str,
    /// Name in the help popup (may list alternatives, such as `q, Ctrl-C`).
    pub help_key: &'static str,
    /// One or two words for the footer.
    pub short: &'static str,
    /// What it does, for the help popup.
    pub what: &'static str,
}

impl Key {
    const fn new(key: &'static str, short: &'static str, what: &'static str) -> Self {
        Self {
            key,
            help_key: key,
            short,
            what,
        }
    }

    const fn help_as(mut self, help_key: &'static str) -> Self {
        self.help_key = help_key;
        self
    }

    /// The footer hint for this key.
    #[must_use]
    pub fn hint(self) -> Hint {
        Hint::Key(self.key, self.short)
    }
}

/// One entry of a hint line: a key and what it does, or a note on what is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// Key name and a word or two.
    Key(&'static str, &'static str),
    /// What the owner still has to do before Enter works, in a few words.
    Missing(String),
}

/// Choose a function on the main screen.
pub const CHOOSE: Key = Key::new("↑↓", "choose", "choose a function").help_as("↑↓ j k");
/// Run the chosen function.
pub const RUN: Key = Key::new("Enter", "run", "run it");
/// Stop the chosen function.
pub const STOP: Key = Key::new("s", "stop", "stop it while it runs");
/// Open the attention list.
pub const ATTENTION: Key = Key::new("a", "attention", "what needs attention");
/// Open the help popup.
pub const HELP_KEY: Key = Key::new("?", "help", "this help");
/// Quit.
pub const QUIT: Key = Key::new("q", "quit", "quit").help_as("q, Ctrl-C");

/// Move between the rows of a step form.
pub const FORM_MOVE: Key = Key::new("↑↓", "move", "move between rows");
/// Change a choice.
pub const FORM_CHANGE: Key = Key::new("←→", "change", "change a choice");
/// Tick or untick an entry of a list.
pub const FORM_TICK: Key = Key::new("Space", "tick", "tick, untick or switch");
/// Switch a toggle row on or off.
pub const FORM_SWITCH: Key = Key::new("Space", "switch", "switch on or off");
/// Continue to the next step of a form.
pub const FORM_CONTINUE: Key = Key::new("Enter", "continue", "continue, then start");
/// Start, on a form's last step.
pub const FORM_START: Key = Key::new("Enter", "start", "start");
/// Look for devices again.
pub const REFRESH: Key = Key::new("r", "look again", "look for devices again");
/// Clear a text field.
pub const CLEAR: Key = Key::new("Ctrl-U", "clear", "clear the text");

/// Cancel a step form.
pub const CANCEL: Key = Key::new("Esc", "cancel", "cancel, close, or no").help_as("Esc, n");
/// Close a popup.
pub const CLOSE: Key = Key::new("Esc", "close", "close").help_as("Esc, q");
/// Scroll a list.
pub const SCROLL: Key = Key::new("↑↓", "scroll", "scroll a list").help_as("↑↓ j k");
/// Scroll a page.
pub const PAGE: Key = Key::new("PgUp PgDn", "page", "scroll a page");
/// Jump to the ends of a list.
pub const ENDS: Key = Key::new("Home, End", "ends", "go to the top or end");
/// Answer yes.
pub const YES: Key = Key::new("y", "yes", "yes").help_as("y, Enter");
/// Answer no.
pub const NO: Key = Key::new("n", "no", "no, leave it as it is");

/// One block of the help popup.
#[derive(Debug, Clone, Copy)]
pub struct KeyGroup {
    /// Where the keys apply.
    pub title: &'static str,
    /// Its keys.
    pub keys: &'static [Key],
}

/// All keys, grouped by where they apply. Short enough to read at 40 columns.
pub const HELP: &[KeyGroup] = &[
    KeyGroup {
        title: "Main screen",
        keys: &[CHOOSE, RUN, STOP, ATTENTION, HELP_KEY, QUIT],
    },
    KeyGroup {
        title: "Step forms",
        keys: &[
            FORM_MOVE,
            FORM_CHANGE,
            FORM_TICK,
            FORM_CONTINUE,
            REFRESH,
            CLEAR,
            CANCEL,
        ],
    },
    KeyGroup {
        title: "Lists and questions",
        keys: &[SCROLL, PAGE, ENDS, YES, NO, CLOSE],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_footer_key_is_in_the_help() {
        let in_help = |key: &str| {
            HELP.iter()
                .flat_map(|group| group.keys)
                .any(|entry| entry.help_key.contains(key))
        };
        for key in [
            CHOOSE,
            RUN,
            STOP,
            ATTENTION,
            HELP_KEY,
            QUIT,
            FORM_MOVE,
            FORM_CHANGE,
            FORM_TICK,
            FORM_SWITCH,
            FORM_CONTINUE,
            FORM_START,
            REFRESH,
            CANCEL,
            CLOSE,
            SCROLL,
            YES,
            NO,
        ] {
            assert!(in_help(key.key), "help is missing {}", key.key);
        }
    }

    #[test]
    fn help_fits_forty_columns() {
        for group in HELP {
            for key in group.keys {
                assert!(key.help_key.chars().count() < KEY_WIDTH, "{}", key.help_key);
                // 40 columns less borders, padding and the scrollbar.
                assert!(
                    KEY_WIDTH.saturating_add(key.what.chars().count()) <= 35,
                    "{}",
                    key.what
                );
            }
        }
    }

    #[test]
    fn help_line_count_is_titles_keys_and_gaps() {
        let keys: usize = HELP.iter().map(|group| group.keys.len()).sum();
        assert_eq!(
            help_line_count(),
            keys.saturating_add(HELP.len().saturating_mul(2))
                .saturating_sub(1)
        );
    }
}
