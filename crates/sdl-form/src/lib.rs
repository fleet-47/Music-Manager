//! The form module: one shared form for every function of the app (modelled on Charm's Huh). It
//! knows nothing about the app: fields, focus order, what a key, a paste or a mouse event does,
//! the summary and what is still missing, and the rectangles it drew.
//!
//! **State lives in the form; drawing is a pure function of that state.** [`render`] reads a
//! [`Form`] and draws it, and returns the [`HitMap`] of what it drew; the screen keeps that map
//! and hands it back with the next mouse event ([`Form::mouse`]). Nothing here draws a popup or
//! owns a timer: the screen puts [`render`] inside a popup of [`size`], and calls [`Form::tick`]
//! while [`Form::is_animating`].
//!
//! ```text
//! build:   Form::new("Convert").field(Field::ticks("what", "What")) ... .primary("Start")
//! feed:    form.key(key) / form.paste(text) / form.mouse(event, &hits)  ->  Reaction
//! update:  form.set_options(key, options)   (a device appears, a list arrives)
//!          form.set_summary(..) / set_busy(..) / set_blocked(..)
//! read:    form.value(key)  ->  Value
//! draw:    let hits = render(&form, area, buf, &style);   size(&form, max_width)
//! ```
//!
//! Fixed heights: a list shows [`LIST_ROWS`] rows and scrolls inside them; an open dropdown shows
//! at most [`DROPDOWN_ROWS`]. So [`size`] and [`render`] are functions of the state alone.

mod draw;
mod field;
mod filter;
mod form;
mod hits;

pub use draw::{FormStyle, render, size};
pub use field::{Choice, Field, Key, Part, Value};
pub use form::{Focus, Form, Hint, Reaction};
pub use hits::{HitMap, Target};

/// Rows a tick list or list shows at once; longer lists scroll inside them.
pub const LIST_ROWS: u16 = 6;

/// Rows an open dropdown shows at most.
pub(crate) const DROPDOWN_ROWS: u16 = 8;

/// One frame of the spinner, for a step counter that only ever goes up: the app's spinner step
/// and a form's ([`Form::tick`]) both go through here, so every spinner on screen looks the same.
#[must_use]
pub fn spinner(step: usize) -> &'static str {
    let frames = throbber_widgets_tui::BRAILLE_DOUBLE.symbols;
    step.checked_rem(frames.len())
        .and_then(|at| frames.get(at))
        .copied()
        .unwrap_or("")
}
