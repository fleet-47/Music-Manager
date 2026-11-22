//! The one place colours are named (F1). Roles map to Catppuccin Mocha; the background is painted
//! with `base`. When `NO_COLOR` is set and not empty, every role is the default style: no colours, no background.

use catppuccin::PALETTE;
use ratatui::style::{Modifier, Style};

/// Role to style mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    colour: bool,
}

impl Theme {
    /// A theme with colours on or off.
    #[must_use]
    pub fn new(colour: bool) -> Self {
        Self { colour }
    }

    /// Colours off when the `NO_COLOR` environment variable is set to a non-empty value.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()))
    }

    fn fg(self, colour: catppuccin::Color) -> Style {
        if self.colour {
            Style::new().fg(colour.into())
        } else {
            Style::new()
        }
    }

    fn bg(self, colour: catppuccin::Color) -> Style {
        if self.colour {
            Style::new().bg(colour.into())
        } else {
            Style::new()
        }
    }

    /// The whole screen: `base` behind `text`, so unstyled text is body text.
    #[must_use]
    pub fn background(self) -> Style {
        self.bg(PALETTE.mocha.colors.base).patch(self.body())
    }

    /// Headings and panel titles: `mauve`, bold.
    #[must_use]
    pub fn heading(self) -> Style {
        self.fg(PALETTE.mocha.colors.mauve)
            .add_modifier(Modifier::BOLD)
    }

    /// Body text: `text`.
    #[must_use]
    pub fn body(self) -> Style {
        self.fg(PALETTE.mocha.colors.text)
    }

    /// Secondary text (hints, "Last run"): `subtext0`.
    #[must_use]
    pub fn secondary(self) -> Style {
        self.fg(PALETTE.mocha.colors.subtext0)
    }

    /// Emphasis (numbers, the selected row's marker, key names): `peach`.
    #[must_use]
    pub fn emphasis(self) -> Style {
        self.fg(PALETTE.mocha.colors.peach)
    }

    /// Borders: `surface2`.
    #[must_use]
    pub fn border(self) -> Style {
        self.fg(PALETTE.mocha.colors.surface2)
    }

    /// The focused popup's border: `lavender`.
    #[must_use]
    pub fn border_focus(self) -> Style {
        self.fg(PALETTE.mocha.colors.lavender)
    }

    /// The selected row's background: `surface0`.
    #[must_use]
    pub fn selected(self) -> Style {
        self.bg(PALETTE.mocha.colors.surface0)
    }

    /// Status running: `blue`.
    #[must_use]
    pub fn running(self) -> Style {
        self.fg(PALETTE.mocha.colors.blue)
    }

    /// Status done: `green`.
    #[must_use]
    pub fn done(self) -> Style {
        self.fg(PALETTE.mocha.colors.green)
    }

    /// Status needs attention or unmatched: `yellow`.
    #[must_use]
    pub fn attention(self) -> Style {
        self.fg(PALETTE.mocha.colors.yellow)
    }

    /// Status failed: `red`. Used for failures only.
    #[must_use]
    pub fn failed(self) -> Style {
        self.fg(PALETTE.mocha.colors.red)
    }

    /// Status skipped: `overlay1`.
    #[must_use]
    pub fn skipped(self) -> Style {
        self.fg(PALETTE.mocha.colors.overlay1)
    }

    /// The focused field's label: `lavender`, bold (bold alone without colour, so it still stands
    /// out).
    #[must_use]
    pub fn focus(self) -> Style {
        self.border_focus().add_modifier(Modifier::BOLD)
    }

    /// The button with the focus, the one Enter presses: filled, `base` on `lavender`, bold.
    /// Without colour it is reversed, so it is still a filled block.
    #[must_use]
    pub fn button(self) -> Style {
        if self.colour {
            Style::new()
                .fg(PALETTE.mocha.colors.base.into())
                .bg(PALETTE.mocha.colors.lavender.into())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }

    /// The styles a form is drawn in, from the roles here: labels `mauve`, body `text`, secondary
    /// `subtext0`, emphasis `peach`, the focused field `lavender`, the focused button filled, flagged parts and what is
    /// missing `yellow` (red is for failures only), the highlighted row on `surface0`.
    #[must_use]
    pub fn form_style(self) -> sdl_form::FormStyle {
        sdl_form::FormStyle {
            label: self.heading(),
            body: self.body(),
            secondary: self.secondary(),
            emphasis: self.emphasis(),
            focus: self.focus(),
            button: self.button(),
            selected: self.selected(),
            flagged: self.attention(),
            error: self.attention(),
        }
    }

    /// The colour effects fade from and to: the `base` background. `None` without colour (effects
    /// are off).
    #[must_use]
    pub fn base(self) -> Option<ratatui::style::Color> {
        self.colour.then(|| PALETTE.mocha.colors.base.into())
    }

    /// A progress bar: the running colour on the selected-row background. Without colour the
    /// filled part is reversed so it stays visible.
    #[must_use]
    pub fn bar(self) -> Style {
        if self.colour {
            self.running().patch(self.selected())
        } else {
            Style::new().add_modifier(Modifier::REVERSED)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_colour_applies_nothing_to_colour_roles() {
        let theme = Theme::new(false);
        for style in [
            theme.background(),
            theme.body(),
            theme.secondary(),
            theme.emphasis(),
            theme.border(),
            theme.border_focus(),
            theme.selected(),
            theme.running(),
            theme.done(),
            theme.attention(),
            theme.failed(),
            theme.skipped(),
        ] {
            assert_eq!(style.fg, None);
            assert_eq!(style.bg, None);
        }
    }

    #[test]
    fn roles_are_distinct_with_colour() {
        let theme = Theme::new(true);
        let roles = [
            theme.heading().fg,
            theme.body().fg,
            theme.secondary().fg,
            theme.emphasis().fg,
            theme.border().fg,
            theme.border_focus().fg,
            theme.running().fg,
            theme.done().fg,
            theme.attention().fg,
            theme.failed().fg,
            theme.skipped().fg,
        ];
        for (i, a) in roles.iter().enumerate() {
            assert!(a.is_some());
            for b in roles.iter().skip(i.saturating_add(1)) {
                assert_ne!(a, b);
            }
        }
        assert!(theme.background().bg.is_some());
        assert!(theme.selected().bg.is_some());
    }
}
