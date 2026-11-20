//! Motion (F1): popups and the running panel fade or slide in and out (150 ms at most), a
//! function row flashes once when its task ends. Built on `tachyonfx`. Input is never held up:
//! an effect only recolours cells of the frame that is drawn anyway, and the loop reads input
//! while it plays.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use tachyonfx::{EffectManager, Interpolation, Motion, fx};

/// How long any effect plays, at most.
pub const LENGTH_MS: u32 = 150;

/// What an effect is for; a new effect of the same kind replaces the one playing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A popup opening.
    #[default]
    Popup,
    /// The running panel opening.
    Running,
    /// The row at this position flashing.
    Row(usize),
}

/// The effects playing now.
#[derive(Debug, Default)]
pub struct Effects {
    manager: EffectManager<Kind>,
}

impl Effects {
    /// A popup fades in over `area`, from the background colour `base`.
    pub fn popup_in(&mut self, area: Rect, base: Color) {
        let effect = fx::fade_from(base, base, (LENGTH_MS, Interpolation::QuadOut)).with_area(area);
        self.manager.add_unique_effect(Kind::Popup, effect);
    }

    /// The running panel slides in from the right over `area`.
    pub fn running_in(&mut self, area: Rect, base: Color) {
        let effect = fx::slide_in(
            Motion::RightToLeft,
            6,
            0,
            base,
            (LENGTH_MS, Interpolation::QuadOut),
        )
        .with_area(area);
        self.manager.add_unique_effect(Kind::Running, effect);
    }

    /// The row at `index` (position among the function rows) flashes once: it fades in from
    /// `base`.
    pub fn flash_row(&mut self, index: usize, area: Rect, base: Color) {
        let effect = fx::fade_from(base, base, (LENGTH_MS, Interpolation::SineOut)).with_area(area);
        self.manager.add_unique_effect(Kind::Row(index), effect);
    }

    /// Whether any effect is playing (what `App::animating` is set from, so the loop knows it
    /// needs the animation timer).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.manager.is_running()
    }

    /// Advance every effect by `elapsed` and apply it to the frame's buffer. Effects that are
    /// done are dropped.
    pub fn process(&mut self, elapsed: Duration, buf: &mut Buffer, area: Rect) {
        self.manager.process_effects(elapsed.into(), buf, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effect_runs_until_its_length_has_passed_and_then_is_gone() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let mut effects = Effects::default();
        assert!(!effects.is_running());
        effects.popup_in(area, Color::Black);
        assert!(effects.is_running());
        effects.process(Duration::from_millis(100), &mut buf, area);
        assert!(effects.is_running());
        effects.process(Duration::from_millis(100), &mut buf, area);
        assert!(!effects.is_running());
    }

    #[test]
    fn a_second_effect_of_the_same_kind_replaces_the_first() {
        let area = Rect::new(0, 0, 10, 3);
        let mut effects = Effects::default();
        effects.flash_row(1, area, Color::Black);
        effects.flash_row(1, area, Color::Black);
        let mut buf = Buffer::empty(area);
        effects.process(Duration::from_millis(200), &mut buf, area);
        assert!(!effects.is_running());
    }
}
