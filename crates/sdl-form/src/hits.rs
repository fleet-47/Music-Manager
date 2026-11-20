//! The rectangles an element drew, and what a click on each means.

use ratatui::layout::{Position, Rect};

use crate::Key;

/// What was drawn at a rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A field as a whole (its label row and anything not listed below): a click focuses it, and
    /// activates a toggle or opens a dropdown.
    Field(Key),
    /// A row of a field's list, counted among the rows currently listed (after the filter): a
    /// click picks a dropdown or radio option, ticks a tick-list row, or activates a list row.
    Row {
        /// The field.
        field: Key,
        /// Position among the listed rows.
        index: usize,
    },
    /// A tick list's or list's filter box: a click focuses the field.
    Filter(Key),
    /// A list's scrollbar: click and drag move the list.
    Scrollbar(Key),
    /// The Browse action of a folder field.
    Browse(Key),
    /// The folder browser's "Choose this folder" action.
    BrowserChoose,
    /// The folder browser's "Back" action.
    BrowserBack,
    /// The primary button (Continue or Start).
    Primary,
    /// A row of the folder browser: a click selects it; a second click on a selected folder
    /// opens it.
    BrowserRow(usize),
    /// The Cancel button.
    Cancel,
}

/// Rectangles recorded while drawing, each with what it means. Later entries sit on top of
/// earlier ones, so [`HitMap::at`] finds the last entry containing a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitMap<T> {
    items: Vec<(Rect, T)>,
}

impl<T> Default for HitMap<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T> HitMap<T> {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `area` was drawn for `target`. Empty rectangles are not recorded.
    pub fn push(&mut self, area: Rect, target: T) {
        if area.width > 0 && area.height > 0 {
            self.items.push((area, target));
        }
    }

    /// What is drawn at column `column`, row `row` (the topmost), if anything.
    #[must_use]
    pub fn at(&self, column: u16, row: u16) -> Option<&T> {
        let position = Position::new(column, row);
        self.items
            .iter()
            .rev()
            .find(|(area, _)| area.contains(position))
            .map(|(_, target)| target)
    }

    /// The rectangle recorded for the first entry equal to `target`.
    #[must_use]
    pub fn area_of(&self, target: &T) -> Option<Rect>
    where
        T: PartialEq,
    {
        self.items
            .iter()
            .find(|(_, candidate)| candidate == target)
            .map(|(area, _)| *area)
    }

    /// Add the entries of `inner`, drawn into a scrolled buffer whose row `offset` is shown at the
    /// top of `view`: each rectangle is moved there and cut to `view`; one wholly out of view is
    /// dropped.
    pub(crate) fn append_scrolled(&mut self, inner: Self, offset: u16, view: Rect) {
        let shown = Rect::new(0, offset, view.width, view.height);
        for (area, target) in inner.items {
            let visible = area.intersection(shown);
            if visible.is_empty() {
                continue;
            }
            let moved = Rect::new(
                view.x.saturating_add(visible.x),
                view.y.saturating_add(visible.y.saturating_sub(offset)),
                visible.width,
                visible.height,
            );
            self.push(moved, target);
        }
    }

    /// Number of recorded rectangles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_topmost_rectangle_wins_and_misses_are_none() {
        let mut hits = HitMap::new();
        hits.push(Rect::new(0, 0, 10, 3), 1);
        hits.push(Rect::new(2, 1, 3, 1), 2);
        hits.push(Rect::new(0, 0, 0, 5), 3);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits.at(3, 1), Some(&2));
        assert_eq!(hits.at(8, 1), Some(&1));
        assert_eq!(hits.at(10, 1), None);
        assert_eq!(hits.area_of(&2), Some(Rect::new(2, 1, 3, 1)));
    }
}
