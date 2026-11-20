//! The form: fields in focus order, the keys, paste and mouse that drive them, and what is still
//! missing.

use std::path::{Path, PathBuf};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui_explorer::{FileExplorer, FileExplorerBuilder, Input as Browse};
use tui_input::backend::crossterm::to_input_request;
use tui_input::{Input, InputRequest};
use tui_scrollbar::{ScrollBar, ScrollCommand, ScrollLengths};

use crate::field::{Field, Folder, Kind, Pick, Rows};
use crate::{HitMap, Key, LIST_ROWS, Target, Value};

/// Where the keyboard is. Tab and Shift+Tab walk the visible fields in order, a folder field's
/// Browse action right after it, then the primary button, then Cancel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    /// A field.
    Field(Key),
    /// The Browse action of a folder field.
    Browse(Key),
    /// The primary button (Continue or Start).
    Primary,
    /// The Cancel button.
    Cancel,
}

/// What a key, a paste or a mouse event did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// The form has no use for it and nothing changed; the app may use it (`r`, `o`, Ctrl-C).
    Ignored,
    /// Something changed only in what is drawn (focus, scroll, filter, cursor, an open dropdown):
    /// redraw.
    Handled,
    /// A field's value changed: redraw, and the app may answer with a new plan.
    Changed(Key),
    /// A row of a list field was activated (Enter or a click).
    Activated {
        /// The list field.
        field: Key,
        /// The row's option id.
        id: String,
    },
    /// Enter or the primary button while [`Form::can_submit`].
    Submit,
    /// Esc with nothing to clear, or the Cancel button.
    Cancel,
}

/// One entry of the footer's key hints: a key and what it does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    /// The key as the footer shows it.
    pub key: &'static str,
    /// One or two words.
    pub what: &'static str,
}

const fn hint(key: &'static str, what: &'static str) -> Hint {
    Hint { key, what }
}

/// A form. Build it with [`Form::new`] and [`Form::field`]; feed it keys; read its values.
#[derive(Debug)]
pub struct Form {
    title: String,
    notes: Vec<String>,
    fields: Vec<Field>,
    focus: Focus,
    primary: String,
    back: bool,
    summary: Option<String>,
    busy: Option<String>,
    blocked: Option<String>,
    spin: usize,
}

impl Form {
    /// A form with a title, no fields, buttons `Continue` and `Cancel`, and focus on the primary
    /// button until a field is added.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            notes: Vec::new(),
            fields: Vec::new(),
            focus: Focus::Primary,
            primary: "Continue".to_owned(),
            back: false,
            summary: None,
            busy: None,
            blocked: None,
            spin: 0,
        }
    }

    /// Add a field after the others. Focus starts on the first visible field.
    #[must_use]
    pub fn field(mut self, field: Field) -> Self {
        self.fields.push(field);
        if let Some(first) = self.stops().into_iter().next() {
            self.focus = first;
        }
        self
    }

    /// The title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Plain lines shown under the title (a note on what the form does; what was wanted in
    /// review). Replaces the lines before.
    pub fn set_notes(&mut self, notes: Vec<String>) {
        self.notes = notes;
    }

    /// The notes.
    #[must_use]
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// The label of the primary button: `Continue`, or `Start` on the last step.
    pub fn set_primary(&mut self, label: impl Into<String>) {
        self.primary = label.into();
    }

    /// The primary button's label.
    #[must_use]
    pub fn primary(&self) -> &str {
        &self.primary
    }

    /// Whether leaving this form goes back to the form before it (Review's song detail goes back
    /// to its list) rather than cancelling: the button reads `Back` and the footer says
    /// `Esc back`. [`Reaction::Cancel`] is the same either way; the app decides what it means.
    pub fn set_back(&mut self, back: bool) {
        self.back = back;
    }

    /// The cancel button's label: `Cancel`, or `Back` ([`Form::set_back`]).
    #[must_use]
    pub fn cancel(&self) -> &'static str {
        if self.back { "Back" } else { "Cancel" }
    }

    /// The summary line (`412 new of 2,117 · 3 waiting in Review`); `None` hides it.
    pub fn set_summary(&mut self, summary: Option<String>) {
        self.summary = summary;
    }

    /// The summary line.
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// What the app is doing for the form right now (`Listing your playlists`): drawn with a
    /// spinner instead of the summary, so nothing looks hung, and the form cannot continue until
    /// it is cleared with `None`.
    pub fn set_busy(&mut self, busy: Option<String>) {
        self.busy = busy;
    }

    /// Why the app says the form cannot continue (`Not enough room on the device`); shown among
    /// what is missing, after the fields'.
    pub fn set_blocked(&mut self, reason: Option<String>) {
        self.blocked = reason;
    }

    /// Whether a spinner is drawn, so the screen runs its spinner timer.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.busy.is_some()
    }

    /// Advance the spinner one step; the screen calls it on each spinner wake.
    pub fn tick(&mut self) {
        self.spin = self.spin.wrapping_add(1);
    }

    /// The spinner step, for drawing.
    #[must_use]
    pub fn spin(&self) -> usize {
        self.spin
    }

    /// The busy text, for drawing.
    #[must_use]
    pub fn busy(&self) -> Option<&str> {
        self.busy.as_deref()
    }

    /// All fields, visible or not, in order.
    #[must_use]
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// What field `key` holds. `None` for an unknown key and for a plain list.
    #[must_use]
    pub fn value(&self, key: Key) -> Option<Value> {
        self.field_ref(key)?.value()
    }

    /// Put `value` in field `key` (pre-filling from what was remembered). An unknown key or a
    /// value of the wrong shape changes nothing.
    pub fn set(&mut self, key: Key, value: Value) {
        if let Some(field) = self.field_mut(key) {
            field.set(value);
        }
    }

    /// Replace the options of field `key` while the form is open, keeping what the owner chose
    /// where an option is still there. If the focused field was the one that went away, nothing
    /// moves; if focus is on a field that becomes hidden, [`Form::set_visible`] moves it.
    pub fn set_options(&mut self, key: Key, options: Vec<crate::Choice>) {
        if let Some(field) = self.field_mut(key) {
            field.replace_options(options);
        }
    }

    /// Show or hide field `key`. Focus on a field that is hidden moves to the next stop.
    pub fn set_visible(&mut self, key: Key, visible: bool) {
        if let Some(field) = self.field_mut(key) {
            field.visible = visible;
        }
        if !self.stops().contains(&self.focus) {
            self.focus = self.stops().into_iter().next().unwrap_or(Focus::Primary);
        }
    }

    /// Where the keyboard is.
    #[must_use]
    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    /// Move the keyboard to `focus` if it is a stop of the form.
    pub fn set_focus(&mut self, focus: Focus) {
        if self.stops().contains(&focus) {
            self.focus = focus;
        }
    }

    /// What still stands in the way of continuing, in plain words: each visible field's
    /// complaint or need, then the app's reason ([`Form::set_blocked`]).
    #[must_use]
    pub fn missing(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|field| field.visible)
            .filter_map(Field::missing)
            .chain(self.blocked.clone())
            .collect()
    }

    /// Whether Enter can continue: nothing is missing and the app is not busy.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        self.busy.is_none() && self.missing().is_empty()
    }

    /// The keys valid right now, for the footer.
    #[must_use]
    pub fn hints(&self) -> Vec<Hint> {
        let browsing = self.explorer().is_some();
        if browsing {
            return vec![
                hint("↑↓", "move"),
                hint("→", "open"),
                hint("←", "up"),
                hint("Enter", "choose this folder"),
                hint("Esc", "back"),
            ];
        }
        let kind = match &self.focus {
            Focus::Field(key) => self.field_ref(key).map(|field| &field.kind),
            _ => None,
        };
        let escape = match kind {
            Some(Kind::Ticks(rows) | Kind::List(rows)) if !rows.filter.value().is_empty() => {
                hint("Esc", "clear filter")
            }
            Some(Kind::Dropdown(pick)) if pick.open => hint("Esc", "close"),
            _ if self.back => hint("Esc", "back"),
            _ => hint("Esc", "cancel"),
        };
        let mut hints = match &self.focus {
            Focus::Field(key) => match self.field_ref(key).map(|field| &field.kind) {
                Some(Kind::Text(_) | Kind::Folder(_)) => vec![hint("Enter", "continue")],
                Some(Kind::Dropdown(pick)) if pick.open => {
                    vec![hint("↑↓", "move"), hint("Enter", "choose")]
                }
                Some(Kind::Dropdown(_)) => vec![hint("Enter", "open")],
                Some(Kind::Radio(_)) => vec![hint("↑↓", "choose"), hint("Enter", "continue")],
                Some(Kind::Ticks(_)) => {
                    vec![
                        hint("↑↓", "move"),
                        hint("Space", "tick"),
                        hint("type", "filter"),
                    ]
                }
                Some(Kind::List(_)) => {
                    vec![
                        hint("↑↓", "move"),
                        hint("Enter", "open"),
                        hint("type", "filter"),
                    ]
                }
                Some(Kind::Toggle(_)) => vec![hint("Space", "switch"), hint("Enter", "continue")],
                None => Vec::new(),
            },
            Focus::Browse(_) => vec![hint("Enter", "browse")],
            Focus::Primary | Focus::Cancel => vec![hint("Enter", "press")],
        };
        hints.push(hint("Tab", "next"));
        hints.push(escape);
        hints
    }

    /// The open folder browser of the focused folder field, if any.
    #[must_use]
    pub fn explorer(&self) -> Option<&FileExplorer> {
        self.fields.iter().find_map(|field| match &field.kind {
            Kind::Folder(Folder {
                browser: Some(browser),
                ..
            }) => Some(&**browser),
            _ => None,
        })
    }

    // ----- keys, paste, mouse -----

    /// A key press (repeats count; releases are ignored).
    ///
    /// * Tab, Shift+Tab: next, previous stop. Up, Down: inside a list, radio group or open
    ///   dropdown; on a single-row field they move to the previous or next stop.
    /// * Enter: continues ([`Reaction::Submit`], only when [`Form::can_submit`]); on a dropdown it
    ///   opens, on an open dropdown it chooses, on a list row it activates, on Browse it browses,
    ///   on a button it presses.
    /// * Space: ticks, switches, opens a dropdown, presses a button.
    /// * Esc: clears a non-empty filter, otherwise closes an open dropdown or browser, otherwise
    ///   [`Reaction::Cancel`].
    /// * Characters, Backspace, Ctrl-U and the other editing keys of `tui-input`: into a text
    ///   field or a list's filter box. Typing in a filter returns the highlight to the best match.
    #[must_use]
    pub fn key(&mut self, key: KeyEvent) -> Reaction {
        if key.kind == KeyEventKind::Release {
            return Reaction::Ignored;
        }
        if self.explorer().is_some() {
            return self.browser_key(key);
        }
        match key.code {
            KeyCode::Tab => return self.walk(1),
            KeyCode::BackTab => return self.walk(-1),
            KeyCode::Esc => return self.escape(),
            KeyCode::Enter => return self.enter(),
            _ => {}
        }
        match self.focus.clone() {
            Focus::Field(field) => self.field_key(field, key),
            Focus::Browse(_) | Focus::Primary | Focus::Cancel => match key.code {
                KeyCode::Char(' ') => self.enter(),
                KeyCode::Up => self.walk(-1),
                KeyCode::Down => self.walk(1),
                _ => Reaction::Ignored,
            },
        }
    }

    /// Pasted text goes into the focused text field, folder field or list filter, with line
    /// breaks removed. Anything else ignores it.
    #[must_use]
    pub fn paste(&mut self, text: &str) -> Reaction {
        let Focus::Field(key) = self.focus.clone() else {
            return Reaction::Ignored;
        };
        let flat: String = text.chars().filter(|c| !c.is_control()).collect();
        let Some(field) = self.field_mut(key) else {
            return Reaction::Ignored;
        };
        match &mut field.kind {
            Kind::Text(text) => {
                insert(&mut text.input, &flat);
                Reaction::Changed(key)
            }
            Kind::Folder(folder) if folder.browser.is_none() => {
                insert(&mut folder.text.input, &flat);
                Reaction::Changed(key)
            }
            Kind::Ticks(rows) | Kind::List(rows) => {
                insert(&mut rows.filter, &flat);
                rows.cursor = 0;
                rows.offset = 0;
                Reaction::Handled
            }
            _ => Reaction::Ignored,
        }
    }

    /// A mouse event, tested against the rectangles the last [`crate::render`] recorded. A left
    /// click focuses or activates what is under it; the wheel scrolls the list under it; a drag
    /// on a scrollbar moves its list.
    #[must_use]
    pub fn mouse(&mut self, event: MouseEvent, hits: &HitMap<Target>) -> Reaction {
        let target = hits.at(event.column, event.row).cloned();
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => match target {
                Some(target) => self.click(&target, event, hits),
                None => Reaction::Ignored,
            },
            MouseEventKind::ScrollUp => self.wheel(target.as_ref(), -1),
            MouseEventKind::ScrollDown => self.wheel(target.as_ref(), 1),
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                self.drag(event, hits)
            }
            _ => Reaction::Ignored,
        }
    }

    // ----- internals -----

    fn field_ref(&self, key: Key) -> Option<&Field> {
        self.fields.iter().find(|field| field.key == key)
    }

    fn field_mut(&mut self, key: Key) -> Option<&mut Field> {
        self.fields.iter_mut().find(|field| field.key == key)
    }

    /// Every stop of the focus order.
    fn stops(&self) -> Vec<Focus> {
        let mut stops = Vec::new();
        for field in self.fields.iter().filter(|field| field.visible) {
            stops.push(Focus::Field(field.key));
            if matches!(field.kind, Kind::Folder(_)) {
                stops.push(Focus::Browse(field.key));
            }
        }
        stops.push(Focus::Primary);
        stops.push(Focus::Cancel);
        stops
    }

    /// Move focus `step` stops (wrapping), closing an open dropdown the keyboard leaves.
    fn walk(&mut self, step: isize) -> Reaction {
        let stops = self.stops();
        let at = stops
            .iter()
            .position(|stop| stop == &self.focus)
            .unwrap_or(0);
        let count = isize::try_from(stops.len()).unwrap_or(1).max(1);
        let to = isize::try_from(at)
            .unwrap_or(0)
            .saturating_add(step)
            .rem_euclid(count);
        self.close_dropdowns();
        if let Some(next) = usize::try_from(to).ok().and_then(|to| stops.get(to)) {
            self.focus = next.clone();
        }
        Reaction::Handled
    }

    fn close_dropdowns(&mut self) {
        for field in &mut self.fields {
            if let Kind::Dropdown(pick) = &mut field.kind {
                pick.open = false;
            }
        }
    }

    fn escape(&mut self) -> Reaction {
        let Focus::Field(key) = self.focus.clone() else {
            return Reaction::Cancel;
        };
        match self.field_mut(key).map(|field| &mut field.kind) {
            Some(Kind::Ticks(rows) | Kind::List(rows)) if !rows.filter.value().is_empty() => {
                rows.filter.reset();
                rows.cursor = 0;
                rows.offset = 0;
                Reaction::Handled
            }
            Some(Kind::Dropdown(pick)) if pick.open => {
                pick.open = false;
                Reaction::Handled
            }
            _ => Reaction::Cancel,
        }
    }

    fn enter(&mut self) -> Reaction {
        match self.focus.clone() {
            Focus::Cancel => Reaction::Cancel,
            Focus::Browse(key) => self.open_browser(key),
            Focus::Primary => self.submit(),
            Focus::Field(key) => {
                let Some(field) = self.field_mut(key) else {
                    return Reaction::Ignored;
                };
                match &mut field.kind {
                    Kind::Dropdown(pick) if pick.open => choose_at_cursor(pick, key),
                    Kind::Dropdown(pick) => {
                        open_dropdown(pick);
                        Reaction::Handled
                    }
                    Kind::List(rows) => activate(rows, rows.cursor, key),
                    _ => self.submit(),
                }
            }
        }
    }

    fn submit(&self) -> Reaction {
        if self.can_submit() {
            Reaction::Submit
        } else {
            Reaction::Handled
        }
    }

    fn field_key(&mut self, key: Key, event: KeyEvent) -> Reaction {
        let Some(field) = self.field_mut(key) else {
            return Reaction::Ignored;
        };
        let step = match event.code {
            KeyCode::Up => Some(-1),
            KeyCode::Down => Some(1),
            _ => None,
        };
        match &mut field.kind {
            Kind::Text(text) => match step {
                Some(step) => self.walk(step),
                None => edit_text(&mut text.input, event, Reaction::Changed(key)),
            },
            Kind::Folder(folder) => match step {
                Some(step) => self.walk(step),
                None => edit_text(&mut folder.text.input, event, Reaction::Changed(key)),
            },
            Kind::Toggle(on) => match (event.code, step) {
                (KeyCode::Char(' '), _) => {
                    *on = !*on;
                    Reaction::Changed(key)
                }
                (_, Some(step)) => self.walk(step),
                _ => Reaction::Ignored,
            },
            Kind::Dropdown(pick) => match (event.code, step) {
                (KeyCode::Char(' '), _) if pick.open => choose_at_cursor(pick, key),
                (KeyCode::Char(' '), _) => {
                    open_dropdown(pick);
                    Reaction::Handled
                }
                (_, Some(step)) if pick.open => {
                    let last = pick.options.len().saturating_sub(1);
                    pick.cursor = pick.cursor.saturating_add_signed(step).min(last);
                    Reaction::Handled
                }
                (_, Some(step)) => self.walk(step),
                _ => Reaction::Ignored,
            },
            Kind::Radio(pick) => match step {
                Some(step) if !pick.options.is_empty() => {
                    let last = pick.options.len().saturating_sub(1);
                    let at = pick
                        .chosen
                        .as_ref()
                        .and_then(|id| pick.options.iter().position(|option| &option.id == id));
                    // With nothing chosen yet, either arrow chooses the first option.
                    let to = at.map_or(0, |at| at.saturating_add_signed(step).min(last));
                    if at == Some(to) {
                        return Reaction::Handled;
                    }
                    pick.chosen = pick.options.get(to).map(|option| option.id.clone());
                    pick.cursor = to;
                    Reaction::Changed(key)
                }
                Some(step) => self.walk(step),
                None => Reaction::Ignored,
            },
            Kind::Ticks(rows) => match event.code {
                KeyCode::Char(' ') => tick_at_cursor(rows, key),
                _ => rows_key(rows, event, step),
            },
            Kind::List(rows) => rows_key(rows, event, step),
        }
    }

    fn open_browser(&mut self, key: Key) -> Reaction {
        let Some(field) = self.field_mut(key) else {
            return Reaction::Ignored;
        };
        let Kind::Folder(folder) = &mut field.kind else {
            return Reaction::Ignored;
        };
        let start = browse_start(folder.text.input.value());
        // Only folders are listed: the browser chooses a folder, so files would only be noise.
        let explorer = FileExplorerBuilder::default()
            .working_dir(start)
            .filter_map(|file| file.is_dir.then_some(file))
            .build();
        // A folder that cannot be listed leaves the field as it was; the text field still works.
        if let Ok(browser) = explorer {
            folder.browser = Some(Box::new(browser));
        }
        Reaction::Handled
    }

    fn browser_key(&mut self, event: KeyEvent) -> Reaction {
        match event.code {
            KeyCode::Esc => self.close_browser(false),
            KeyCode::Enter => self.close_browser(true),
            _ => {
                let input = Browse::from(&Event::Key(event));
                self.browse(input)
            }
        }
    }

    fn browse(&mut self, input: Browse) -> Reaction {
        let Some(browser) = self.browser_mut() else {
            return Reaction::Ignored;
        };
        if input == Browse::None {
            return Reaction::Ignored;
        }
        // A folder that cannot be opened leaves the browser where it was.
        let _ = browser.handle(input);
        Reaction::Handled
    }

    fn browser_mut(&mut self) -> Option<&mut FileExplorer> {
        self.fields
            .iter_mut()
            .find_map(|field| match &mut field.kind {
                Kind::Folder(Folder {
                    browser: Some(browser),
                    ..
                }) => Some(&mut **browser),
                _ => None,
            })
    }

    /// Close the browser; with `choose`, the folder it shows becomes the field's text.
    fn close_browser(&mut self, choose: bool) -> Reaction {
        for field in &mut self.fields {
            let Kind::Folder(folder) = &mut field.kind else {
                continue;
            };
            let Some(browser) = folder.browser.take() else {
                continue;
            };
            if choose {
                folder.text.input = Input::new(browser.cwd().display().to_string());
                return Reaction::Changed(field.key);
            }
            return Reaction::Handled;
        }
        Reaction::Ignored
    }

    fn click(&mut self, target: &Target, event: MouseEvent, hits: &HitMap<Target>) -> Reaction {
        match target.clone() {
            Target::Primary => {
                self.focus = Focus::Primary;
                self.submit()
            }
            Target::Cancel => Reaction::Cancel,
            Target::BrowserChoose => self.close_browser(true),
            Target::BrowserBack => self.close_browser(false),
            Target::BrowserRow(index) => self.click_browser_row(index),
            Target::Browse(key) => {
                self.focus = Focus::Browse(key);
                self.open_browser(key)
            }
            Target::Filter(key) => {
                self.focus = Focus::Field(key);
                Reaction::Handled
            }
            Target::Scrollbar(key) => {
                self.focus = Focus::Field(key);
                self.drag(event, hits)
            }
            Target::Field(key) => {
                self.focus = Focus::Field(key);
                let Some(field) = self.field_mut(key) else {
                    return Reaction::Ignored;
                };
                match &mut field.kind {
                    Kind::Toggle(on) => {
                        *on = !*on;
                        Reaction::Changed(key)
                    }
                    Kind::Dropdown(pick) => {
                        if pick.open {
                            pick.open = false;
                        } else {
                            open_dropdown(pick);
                        }
                        Reaction::Handled
                    }
                    _ => Reaction::Handled,
                }
            }
            Target::Row { field: key, index } => {
                self.focus = Focus::Field(key);
                let Some(field) = self.field_mut(key) else {
                    return Reaction::Ignored;
                };
                match &mut field.kind {
                    Kind::Dropdown(pick) => {
                        pick.cursor = index;
                        choose_at_cursor(pick, key)
                    }
                    Kind::Radio(pick) => {
                        pick.cursor = index;
                        pick.chosen = pick.options.get(index).map(|option| option.id.clone());
                        Reaction::Changed(key)
                    }
                    Kind::Ticks(rows) => {
                        rows.cursor = index;
                        tick_at_cursor(rows, key)
                    }
                    Kind::List(rows) => {
                        rows.cursor = index;
                        activate(rows, index, key)
                    }
                    _ => Reaction::Handled,
                }
            }
        }
    }

    fn click_browser_row(&mut self, index: usize) -> Reaction {
        let Some(browser) = self.browser_mut() else {
            return Reaction::Ignored;
        };
        if browser.selected_idx() == index {
            return self.browse(Browse::Right);
        }
        browser.set_selected_idx(index);
        Reaction::Handled
    }

    fn wheel(&mut self, target: Option<&Target>, delta: isize) -> Reaction {
        if self.explorer().is_some() {
            return self.browse(if delta < 0 { Browse::Up } else { Browse::Down });
        }
        let key = match target {
            Some(
                Target::Field(key)
                | Target::Row { field: key, .. }
                | Target::Filter(key)
                | Target::Scrollbar(key),
            ) => Some(*key),
            _ => None,
        };
        if let Some(Kind::Ticks(rows) | Kind::List(rows)) = key
            .and_then(|key| self.field_mut(key))
            .map(|field| &mut field.kind)
        {
            let most = rows.listed().len().saturating_sub(usize::from(LIST_ROWS));
            rows.offset = rows.offset.saturating_add_signed(delta).min(most);
            return Reaction::Handled;
        }
        // Anywhere else on the form the wheel moves through it as Tab and Shift+Tab do, so the
        // drawn part follows the focus and a form taller than the popup can be read to its end.
        if target.is_none() {
            return Reaction::Ignored;
        }
        self.walk(delta)
    }

    /// Pointer press, drag or release on the focused list's scrollbar (`tui-scrollbar`).
    fn drag(&mut self, event: MouseEvent, hits: &HitMap<Target>) -> Reaction {
        let Focus::Field(key) = self.focus.clone() else {
            return Reaction::Ignored;
        };
        let Some(area) = hits.area_of(&Target::Scrollbar(key)) else {
            return Reaction::Ignored;
        };
        let Some(field) = self.field_mut(key) else {
            return Reaction::Ignored;
        };
        let (Kind::Ticks(rows) | Kind::List(rows)) = &mut field.kind else {
            return Reaction::Ignored;
        };
        let bar = scrollbar(rows);
        match bar.handle_mouse_event(area, event, &mut rows.drag) {
            Some(ScrollCommand::SetOffset(offset)) => {
                rows.offset = offset;
                Reaction::Handled
            }
            None => Reaction::Ignored,
        }
    }
}

/// The scrollbar of a list, from its state: what [`crate::render`] draws and what a drag moves.
pub(crate) fn scrollbar(rows: &Rows) -> ScrollBar {
    ScrollBar::vertical(ScrollLengths {
        content_len: rows.listed().len(),
        viewport_len: usize::from(LIST_ROWS),
    })
    .offset(rows.offset)
}

fn insert(input: &mut Input, text: &str) {
    for c in text.chars() {
        input.handle(InputRequest::InsertChar(c));
    }
}

/// Apply an editing key to a text box; `changed` when it changed the text.
fn edit_text(input: &mut Input, event: KeyEvent, changed: Reaction) -> Reaction {
    match to_input_request(&Event::Key(event)) {
        Some(request) => {
            let before = input.value().to_owned();
            input.handle(request);
            if input.value() == before {
                Reaction::Handled
            } else {
                changed
            }
        }
        None => Reaction::Ignored,
    }
}

fn open_dropdown(pick: &mut Pick) {
    pick.open = true;
    pick.cursor = pick
        .chosen
        .as_ref()
        .and_then(|id| pick.options.iter().position(|option| &option.id == id))
        .unwrap_or(0);
}

fn choose_at_cursor(pick: &mut Pick, key: Key) -> Reaction {
    pick.open = false;
    match pick.options.get(pick.cursor) {
        Some(option) => {
            pick.chosen = Some(option.id.clone());
            Reaction::Changed(key)
        }
        None => Reaction::Handled,
    }
}

/// The option under listed row `row`.
fn option_at(rows: &Rows, row: usize) -> Option<&crate::Choice> {
    rows.listed().get(row).and_then(|at| rows.options.get(*at))
}

fn tick_at_cursor(rows: &mut Rows, key: Key) -> Reaction {
    let Some(id) = option_at(rows, rows.cursor).map(|option| option.id.clone()) else {
        return Reaction::Handled;
    };
    if !rows.ticked.remove(&id) {
        rows.ticked.insert(id);
    }
    Reaction::Changed(key)
}

fn activate(rows: &Rows, row: usize, key: Key) -> Reaction {
    match option_at(rows, row) {
        Some(option) => Reaction::Activated {
            field: key,
            id: option.id.clone(),
        },
        None => Reaction::Handled,
    }
}

/// Up, Down, paging and typing in a list: the filter is a text box, and typing in it returns the
/// highlight to the best match.
fn rows_key(rows: &mut Rows, event: KeyEvent, step: Option<isize>) -> Reaction {
    let page = isize::try_from(LIST_ROWS).unwrap_or(6);
    let rows_visible = usize::from(LIST_ROWS);
    match (event.code, step) {
        (_, Some(step)) => rows.step(step, rows_visible),
        (KeyCode::PageUp, _) => rows.step(page.saturating_neg(), rows_visible),
        (KeyCode::PageDown, _) => rows.step(page, rows_visible),
        _ => {
            let before = rows.filter.value().to_owned();
            return match to_input_request(&Event::Key(event)) {
                Some(request) => {
                    rows.filter.handle(request);
                    if rows.filter.value() != before {
                        rows.cursor = 0;
                        rows.offset = 0;
                    }
                    Reaction::Handled
                }
                None => Reaction::Ignored,
            };
        }
    }
    Reaction::Handled
}

/// Where the folder browser opens: the field's folder, else its nearest existing parent, else the
/// home folder, else the root.
fn browse_start(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let typed = match (text.strip_prefix('~'), &home) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(text),
    };
    let mut candidate: Option<&Path> = (!text.trim().is_empty()).then_some(typed.as_path());
    while let Some(path) = candidate {
        if path.is_dir() {
            return path.to_path_buf();
        }
        candidate = path.parent();
    }
    home.unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEventState, KeyModifiers};
    use ratatui::layout::Rect;

    use super::*;
    use crate::{Choice, Value};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn many(n: usize) -> Vec<Choice> {
        (0..n)
            .map(|i| Choice::new(format!("{i}"), format!("List {i}")))
            .collect()
    }

    fn lists_form() -> Form {
        Form::new("Sync Spotify").field(
            Field::ticks("lists", "Lists")
                .options(many(34))
                .needs("Tick at least one list"),
        )
    }

    #[test]
    fn enter_continues_only_when_nothing_is_missing() {
        let mut form = lists_form();
        assert_eq!(form.missing(), vec!["Tick at least one list".to_owned()]);
        assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Handled);
        assert_eq!(
            form.key(press(KeyCode::Char(' '))),
            Reaction::Changed("lists")
        );
        assert_eq!(form.value("lists"), Some(Value::Ticked(vec!["0".into()])));
        assert!(form.can_submit());
        assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Submit);
        form.set_busy(Some("Listing songs".into()));
        assert!(!form.can_submit());
        assert!(form.is_animating());
    }

    #[test]
    fn tab_walks_fields_then_buttons_and_wraps() {
        let mut form = Form::new("F")
            .field(Field::text("a", "A"))
            .field(Field::toggle("b", "B"))
            .field(Field::folder("c", "C"));
        let mut seen = vec![form.focus().clone()];
        for _ in 0..6 {
            assert_eq!(form.key(press(KeyCode::Tab)), Reaction::Handled);
            seen.push(form.focus().clone());
        }
        assert_eq!(
            seen,
            vec![
                Focus::Field("a"),
                Focus::Field("b"),
                Focus::Field("c"),
                Focus::Browse("c"),
                Focus::Primary,
                Focus::Cancel,
                Focus::Field("a"),
            ]
        );
        assert_eq!(form.key(press(KeyCode::BackTab)), Reaction::Handled);
        assert_eq!(form.focus(), &Focus::Cancel);
    }

    #[test]
    fn escape_clears_the_filter_first_then_cancels() {
        let mut form = lists_form();
        assert_eq!(form.key(press(KeyCode::Char('9'))), Reaction::Handled);
        assert_eq!(form.key(press(KeyCode::Esc)), Reaction::Handled);
        assert_eq!(form.key(press(KeyCode::Esc)), Reaction::Cancel);
    }

    #[test]
    fn typing_filters_the_list_and_space_ticks_the_best_match() {
        let mut form = lists_form();
        for c in "list 33".chars().filter(|c| *c != ' ') {
            let _ = form.key(press(KeyCode::Char(c)));
        }
        assert_eq!(
            form.key(press(KeyCode::Char(' '))),
            Reaction::Changed("lists")
        );
        assert_eq!(form.value("lists"), Some(Value::Ticked(vec!["33".into()])));
    }

    #[test]
    fn pasting_goes_into_the_focused_text_field_without_line_breaks() {
        let mut form = Form::new("F").field(Field::text("link", "Link"));
        assert_eq!(form.paste("https://a.b/c\n"), Reaction::Changed("link"));
        assert_eq!(
            form.value("link"),
            Some(Value::Text("https://a.b/c".into()))
        );
        let mut nothing = Form::new("F");
        assert_eq!(nothing.paste("x"), Reaction::Ignored);
    }

    #[test]
    fn a_radio_group_chooses_as_the_highlight_moves() {
        let mut form = Form::new("F").field(Field::radio("device", "Device").options(many(3)));
        assert_eq!(form.key(press(KeyCode::Down)), Reaction::Changed("device"));
        assert_eq!(form.value("device"), Some(Value::Choice(Some("0".into()))));
        assert_eq!(form.key(press(KeyCode::Down)), Reaction::Changed("device"));
        assert_eq!(form.value("device"), Some(Value::Choice(Some("1".into()))));
        assert_eq!(form.key(press(KeyCode::Up)), Reaction::Changed("device"));
        assert_eq!(form.key(press(KeyCode::Up)), Reaction::Handled);
        assert_eq!(form.value("device"), Some(Value::Choice(Some("0".into()))));
    }

    #[test]
    fn a_dropdown_opens_then_chooses() {
        let mut form = Form::new("F").field(Field::dropdown("fmt", "Format").options(many(3)));
        assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Handled);
        let _ = form.key(press(KeyCode::Down));
        assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Changed("fmt"));
        assert_eq!(form.value("fmt"), Some(Value::Choice(Some("1".into()))));
    }

    #[test]
    fn hiding_the_focused_field_moves_focus_on() {
        let mut form = Form::new("F")
            .field(Field::toggle("a", "A"))
            .field(Field::toggle("b", "B"));
        assert_eq!(form.focus(), &Focus::Field("a"));
        form.set_visible("a", false);
        assert_eq!(form.focus(), &Focus::Field("b"));
        assert_eq!(form.missing(), Vec::<String>::new());
    }

    #[test]
    fn a_list_row_is_activated_and_the_primary_button_by_click() {
        let mut form = Form::new("F").field(Field::list("held", "Held").options(many(3)));
        let mut hits = HitMap::new();
        hits.push(
            Rect::new(0, 2, 20, 1),
            Target::Row {
                field: "held",
                index: 1,
            },
        );
        hits.push(Rect::new(0, 9, 8, 1), Target::Primary);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            form.mouse(click(3, 2), &hits),
            Reaction::Activated {
                field: "held",
                id: "1".into()
            }
        );
        assert_eq!(form.mouse(click(3, 9), &hits), Reaction::Submit);
        assert_eq!(form.mouse(click(50, 50), &hits), Reaction::Ignored);
    }

    #[test]
    fn the_wheel_scrolls_a_list_and_elsewhere_moves_through_the_form() {
        let mut form = Form::new("F")
            .field(Field::ticks("lists", "Lists").options(many(20)))
            .field(Field::toggle("cover", "Cover"));
        let mut hits = HitMap::new();
        hits.push(
            Rect::new(0, 2, 20, 1),
            Target::Row {
                field: "lists",
                index: 0,
            },
        );
        hits.push(Rect::new(0, 9, 20, 1), Target::Field("cover"));
        let wheel = |row| MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(form.mouse(wheel(2), &hits), Reaction::Handled);
        assert_eq!(form.focus(), &Focus::Field("lists"));
        assert_eq!(form.mouse(wheel(9), &hits), Reaction::Handled);
        assert_eq!(form.focus(), &Focus::Field("cover"));
        assert_eq!(form.mouse(wheel(30), &hits), Reaction::Ignored);
    }

    #[test]
    fn a_form_that_goes_back_says_so() {
        let mut form = lists_form();
        assert!(form.hints().contains(&hint("Esc", "cancel")));
        let _ = form.key(press(KeyCode::Char('3')));
        assert!(form.hints().contains(&hint("Esc", "clear filter")));
        let _ = form.key(press(KeyCode::Esc));
        form.set_back(true);
        assert_eq!(form.cancel(), "Back");
        assert!(form.hints().contains(&hint("Esc", "back")));
    }

    #[test]
    fn the_browser_chooses_the_folder_it_shows() -> TestResult {
        let dir = std::env::temp_dir();
        let mut form = Form::new("F").field(Field::folder("dest", "Destination"));
        form.set("dest", Value::Text(dir.display().to_string()));
        let _ = form.key(press(KeyCode::Tab));
        assert_eq!(form.focus(), &Focus::Browse("dest"));
        assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Handled);
        assert!(form.explorer().is_some());
        assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Changed("dest"));
        assert!(form.explorer().is_none());
        let Some(Value::Text(chosen)) = form.value("dest") else {
            return Err("no text".into());
        };
        assert_eq!(std::path::Path::new(&chosen), dir.as_path());
        Ok(())
    }
}
