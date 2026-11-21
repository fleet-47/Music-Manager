//! Mouse routing. The top of the screen takes the event: a dialog's form, a plain popup, or the
//! main screen. Each tests the position against the rectangles the last frame recorded
//! ([`crate::app::Hits`]); a click outside a popup does nothing. Every action here has a key.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use tui_scrollbar::{ScrollBar, ScrollBarInteraction, ScrollCommand, ScrollLengths};

use super::popups::Scroll;
use crate::app::{App, Cmd, MainTarget, Popup, PopupTarget};

impl App {
    /// A mouse event; only the left button, the wheel and a scrollbar drag mean anything.
    pub(super) fn on_mouse(&mut self, event: MouseEvent) -> Vec<Cmd> {
        let step = match self.popups.last_mut() {
            Some(Popup::Dialog(dialog)) => Some((
                dialog.task(),
                dialog.mouse(event, &self.hits.form, &self.settings),
            )),
            Some(_) => return self.on_popup_mouse(event),
            None => return self.on_main_mouse(event),
        };
        step.map_or_else(Vec::new, |(task, step)| self.apply_step(task, step))
    }

    fn on_popup_mouse(&mut self, event: MouseEvent) -> Vec<Cmd> {
        let target = self.hits.popup.at(event.column, event.row).copied();
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => match target {
                Some(PopupTarget::Yes) => self.answer(true),
                Some(PopupTarget::No) => self.answer(false),
                Some(PopupTarget::Close) => {
                    self.popups.pop();
                    Vec::new()
                }
                Some(PopupTarget::Item(index)) => self.scroll_to_item(index),
                Some(PopupTarget::Scrollbar) => self.drag_scrollbar(event),
                None => Vec::new(),
            },
            MouseEventKind::Drag(MouseButton::Left) if target == Some(PopupTarget::Scrollbar) => {
                self.drag_scrollbar(event)
            }
            MouseEventKind::ScrollUp => self.scroll(Scroll::By(-1)),
            MouseEventKind::ScrollDown => self.scroll(Scroll::By(1)),
            _ => Vec::new(),
        }
    }

    /// A click on an attention item brings it to the top of the list.
    fn scroll_to_item(&mut self, index: usize) -> Vec<Cmd> {
        let first = self
            .scrolled()
            .and_then(|(extent, _)| extent.items.get(index).map(|(start, _)| *start));
        first.map_or_else(Vec::new, |line| self.scroll(Scroll::To(line)))
    }

    /// Click or drag on the scrollbar: the thumb goes where the pointer is. A drag is read as a
    /// press at each new position, so no state has to outlive the event.
    fn drag_scrollbar(&mut self, event: MouseEvent) -> Vec<Cmd> {
        let (Some((extent, at)), Some(area)) = (
            self.scrolled(),
            self.hits.popup.area_of(&PopupTarget::Scrollbar),
        ) else {
            return Vec::new();
        };
        let bar = ScrollBar::vertical(ScrollLengths {
            content_len: extent.content,
            viewport_len: extent.viewport,
        })
        .offset(at);
        let press = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            ..event
        };
        let mut fresh = ScrollBarInteraction::new();
        match bar.handle_mouse_event(area, press, &mut fresh) {
            Some(ScrollCommand::SetOffset(line)) => self.scroll(Scroll::To(line)),
            None => Vec::new(),
        }
    }

    /// A click on a function row chooses it; a second click on the chosen one runs it. A click
    /// on a running task chooses its function.
    fn on_main_mouse(&mut self, event: MouseEvent) -> Vec<Cmd> {
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Vec::new();
        }
        match self.hits.main.at(event.column, event.row).copied() {
            Some(MainTarget::Row(task)) if task == self.selected => self.run_selected(),
            Some(MainTarget::Row(task) | MainTarget::Running(task)) => {
                self.selected = task;
                Vec::new()
            }
            None => Vec::new(),
        }
    }
}
