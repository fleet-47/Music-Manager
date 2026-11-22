//! Motion (F1): popups fade in and out (150 ms at most), a function's card flashes once when its
//! task ends. Built on `tachyonfx`. Input is never held up:
//! an effect only recolours cells of the frame that is drawn anyway, and the loop reads input
//! while it plays.
//!
//! Going out needs what is no longer drawn: the last frame is kept, and while a popup goes,
//! its cells from that frame are copied back over the new frame
//! (`tachyonfx::blit_buffer_region`) and faded out there. The app already acts on the
//! screen underneath; the ghost only paints over it for at most 150 ms.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Offset, Rect};
use ratatui::style::Color;
use tachyonfx::{Effect, EffectManager, Interpolation, blit_buffer_region, fx};

/// How long any effect plays, at most.
pub const LENGTH_MS: u32 = 150;

/// What an effect is for; a new effect of the same kind replaces the one playing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// A popup opening.
    #[default]
    Popup,
    /// The card at this position flashing.
    Row(usize),
}

/// What the frame just drawn showed, for [`Effects::begin`] to tell what is new in it.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// How many popups are open.
    pub popups: usize,
    /// The top popup's frame.
    pub top: Option<Rect>,
    /// Functions whose run just ended: their position among the functions and their card's
    /// rectangle.
    pub ended: Vec<(usize, Rect)>,
}

/// Something that is no longer drawn, playing its way out: its cells from the last frame it was
/// in, where they were, and the effect over them.
#[derive(Debug)]
struct Ghost {
    frame: Buffer,
    area: Rect,
    effect: Effect,
}

/// The effects playing now, and what the last frame showed.
#[derive(Debug, Default)]
pub struct Effects {
    manager: EffectManager<Kind>,
    popups: usize,
    top: Option<Rect>,
    /// The last frame as drawn, for a ghost to copy from.
    last: Option<Buffer>,
    ghosts: Vec<Ghost>,
}

impl Effects {
    /// A popup fades in over `area`, from the background colour `base`.
    fn popup_in(&mut self, area: Rect, base: Color) {
        let effect = fx::fade_from(base, base, (LENGTH_MS, Interpolation::QuadOut)).with_area(area);
        self.manager.add_unique_effect(Kind::Popup, effect);
    }

    /// The card at `index` (position among the functions) flashes once: it fades in from
    /// `base`.
    fn flash_row(&mut self, index: usize, area: Rect, base: Color) {
        let effect = fx::fade_from(base, base, (LENGTH_MS, Interpolation::SineOut)).with_area(area);
        self.manager.add_unique_effect(Kind::Row(index), effect);
    }

    /// What was at `area` in the last frame plays out with `effect`. Without a last frame there
    /// is nothing to show going.
    fn ghost(&mut self, area: Rect, effect: Effect) {
        if let Some(frame) = self.last.clone() {
            self.ghosts.push(Ghost {
                frame,
                area,
                effect: effect.with_area(area),
            });
        }
    }

    /// Start the effects for what is new in `scene` against the frame before: a popup that
    /// opened fades in and one that closed fades out, each ended function's card flashes.
    pub fn begin(&mut self, scene: &Scene, base: Color) {
        let timer = (LENGTH_MS, Interpolation::QuadOut);
        if scene.popups > self.popups
            && let Some(top) = scene.top
        {
            self.popup_in(top, base);
        }
        if scene.popups < self.popups
            && let Some(gone) = self.top
        {
            self.ghost(gone, fx::fade_to(base, base, timer));
        }
        for (index, area) in &scene.ended {
            self.flash_row(*index, *area, base);
        }
        self.popups = scene.popups;
        self.top = scene.top;
    }

    /// Whether any effect is playing (what `App::animating` is set from, so the loop knows it
    /// needs the animation timer).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.manager.is_running() || !self.ghosts.is_empty()
    }

    /// Advance every effect by `elapsed` and apply it to the frame's buffer; ghosts are painted
    /// over everything. Effects that are done are dropped. The frame as it ends up is kept for
    /// the next ghost.
    pub fn process(&mut self, elapsed: Duration, buf: &mut Buffer, area: Rect) {
        self.manager.process_effects(elapsed.into(), buf, area);
        // A ghost whose time is up leaves the fresh cells under it: no redraw follows the last
        // frame of an effect, so whatever that frame shows stays on screen.
        let fresh = (!self.ghosts.is_empty()).then(|| buf.clone());
        self.ghosts.retain_mut(|ghost| {
            let at = Offset {
                x: i32::from(ghost.area.x),
                y: i32::from(ghost.area.y),
            };
            blit_buffer_region(&ghost.frame, ghost.area, buf, at);
            ghost.effect.process(elapsed.into(), buf, ghost.area);
            let playing = ghost.effect.running();
            if !playing && let Some(fresh) = &fresh {
                blit_buffer_region(fresh, ghost.area, buf, at);
            }
            playing
        });
        self.last = Some(buf.clone());
    }
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;
    use ratatui::widgets::Widget;

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
    fn only_what_is_new_starts_an_effect() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let mut effects = Effects::default();
        let nothing = Scene::default();
        effects.begin(&nothing, Color::Black);
        assert!(!effects.is_running());
        let with_popup = Scene {
            popups: 1,
            top: Some(area),
            ..Scene::default()
        };
        effects.begin(&with_popup, Color::Black);
        assert!(effects.is_running());
        effects.process(Duration::from_millis(200), &mut buf, area);
        assert!(!effects.is_running());
        // The same popup again starts nothing; the popup closing plays it out.
        effects.begin(&with_popup, Color::Black);
        assert!(!effects.is_running());
        effects.begin(&nothing, Color::Black);
        assert!(effects.is_running());
        effects.process(Duration::from_millis(200), &mut buf, area);
        assert!(!effects.is_running());
        let ended = Scene {
            ended: vec![(2, area)],
            ..Scene::default()
        };
        effects.begin(&ended, Color::Black);
        assert!(effects.is_running());
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

    #[test]
    fn a_closed_popup_is_painted_from_the_last_frame_until_it_has_faded() {
        let area = Rect::new(0, 0, 12, 3);
        let popup = Rect::new(2, 1, 6, 1);
        let mut effects = Effects::default();
        let open = Scene {
            popups: 1,
            top: Some(popup),
            ..Scene::default()
        };
        effects.begin(&open, Color::Black);
        let mut frame = Buffer::empty(area);
        Line::raw("POPUP!").render(popup, &mut frame);
        effects.process(Duration::from_millis(200), &mut frame, area);
        // Closed: the new frame no longer draws it, the ghost does, then it is gone.
        effects.begin(&Scene::default(), Color::Black);
        let mut next = Buffer::empty(area);
        effects.process(Duration::ZERO, &mut next, area);
        let row: String = (2..8)
            .filter_map(|x| next.cell((x, 1)).map(ratatui::buffer::Cell::symbol))
            .collect();
        assert_eq!(row, "POPUP!");
        // Its last frame shows what is really there, not a faded ghost.
        let mut last = Buffer::empty(area);
        Line::raw("under").render(popup, &mut last);
        effects.process(Duration::from_millis(200), &mut last, area);
        assert!(!effects.is_running());
        let row: String = (2..7)
            .filter_map(|x| last.cell((x, 1)).map(ratatui::buffer::Cell::symbol))
            .collect();
        assert_eq!(row, "under");
    }
}
