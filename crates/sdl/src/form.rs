//! The step form (F1): the one popup every function uses for its choices.
//!
//! A function declares its form as data: a title and [`Row`]s of four kinds (text, choice, tick
//! list, toggle). The form owns focus, keys, which rows are complete, the summary line and, in
//! [`view`], how it looks. The caller feeds it keys and pastes and reads [`Step`] back; when a
//! choice changes what other rows offer, the caller changes those rows with the `set_*` calls.
//!
//! Keys: Up and Down move through the rows (inside a tick list they move through its entries
//! first), Left and Right cycle a choice or flip a toggle, Space ticks an entry, flips a toggle or
//! cycles a choice, Enter continues, Esc cancels. Enter only continues when every visible row is
//! complete; otherwise focus jumps to the first row that is not.

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler as _;
use tui_input::{Input, InputRequest};

use crate::keys::{self, Hint, Key};

pub mod view;

/// Names a row within its form.
pub type RowId = &'static str;

/// One entry of a tick list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    /// The entry's name.
    pub label: String,
    /// A figure shown at the right edge, such as a song count.
    pub detail: Option<String>,
    /// Whether it is ticked.
    pub on: bool,
}

impl Tick {
    /// An entry with no detail.
    #[must_use]
    pub fn new(label: impl Into<String>, on: bool) -> Self {
        Self {
            label: label.into(),
            detail: None,
            on,
        }
    }

    /// The same entry with a figure at the right edge.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone)]
enum Body {
    Text(Input),
    Choice {
        options: Vec<String>,
        at: usize,
        refresh: bool,
    },
    Ticks {
        items: Vec<Tick>,
        cursor: usize,
        loading: bool,
    },
    Toggle(bool),
}

/// One row of a form. Build it with a constructor and the builder calls, then hand it to
/// [`Form::new`].
#[derive(Debug, Clone)]
pub struct Row {
    id: RowId,
    label: String,
    body: Body,
    needs: Option<String>,
    empty: Option<String>,
    visible: bool,
}

impl Row {
    fn new(id: RowId, label: &str, body: Body) -> Self {
        Self {
            id,
            label: label.to_owned(),
            body,
            needs: None,
            empty: None,
            visible: true,
        }
    }

    /// A text row holding `value`.
    #[must_use]
    pub fn text(id: RowId, label: &str, value: &str) -> Self {
        Self::new(id, label, Body::Text(Input::new(value.to_owned())))
    }

    /// A choice row; Left and Right cycle through `options`, starting at `at`.
    #[must_use]
    pub fn choice(id: RowId, label: &str, options: Vec<String>, at: usize) -> Self {
        let at = at.min(options.len().saturating_sub(1));
        Self::new(
            id,
            label,
            Body::Choice {
                options,
                at,
                refresh: false,
            },
        )
    }

    /// A tick list; Space ticks the entry under the cursor.
    #[must_use]
    pub fn ticks(id: RowId, label: &str, items: Vec<Tick>) -> Self {
        Self::new(
            id,
            label,
            Body::Ticks {
                items,
                cursor: 0,
                loading: false,
            },
        )
    }

    /// A toggle row.
    #[must_use]
    pub fn toggle(id: RowId, label: &str, on: bool) -> Self {
        Self::new(id, label, Body::Toggle(on))
    }

    /// The row is incomplete until it holds something: a non-blank text, or at least one ticked
    /// entry. `hint` says what to do, in a few words (shown in the footer).
    #[must_use]
    pub fn needs(mut self, hint: &str) -> Self {
        self.needs = Some(hint.to_owned());
        self
    }

    /// What to show, in plain words, when a choice has no options or a tick list no entries. The
    /// row is incomplete while that is so.
    #[must_use]
    pub fn when_empty(mut self, text: &str) -> Self {
        self.empty = Some(text.to_owned());
        self
    }

    /// The `r` key on this choice row asks the caller to look again ([`Step::Refresh`]).
    #[must_use]
    pub fn refreshable(mut self) -> Self {
        if let Body::Choice { refresh, .. } = &mut self.body {
            *refresh = true;
        }
        self
    }

    /// The row starts hidden.
    #[must_use]
    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }

    /// What this row still needs, if it is visible and incomplete.
    fn missing(&self) -> Option<String> {
        if !self.visible {
            return None;
        }
        let none_yet = || {
            self.needs
                .clone()
                .unwrap_or_else(|| format!("Fill in {}", self.label.to_lowercase()))
        };
        match &self.body {
            Body::Text(input) => {
                (self.needs.is_some() && input.value().trim().is_empty()).then(none_yet)
            }
            Body::Choice { options, .. } => options
                .is_empty()
                .then(|| self.needs.clone().unwrap_or_else(|| self.empty_text())),
            Body::Ticks { items, loading, .. } => {
                if *loading {
                    Some("Wait for the list".to_owned())
                } else if items.is_empty() {
                    Some(self.empty_text())
                } else {
                    (self.needs.is_some() && !items.iter().any(|item| item.on)).then(none_yet)
                }
            }
            Body::Toggle(_) => None,
        }
    }

    fn empty_text(&self) -> String {
        self.empty
            .clone()
            .unwrap_or_else(|| "Nothing to choose from".to_owned())
    }
}

/// The line under the rows: what the form's choices come to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Summary {
    /// Nothing to say.
    #[default]
    None,
    /// A plain statement; numbers are picked out in the emphasis colour.
    Info(String),
    /// Work is going on behind the form; a spinner shows.
    Busy(String),
    /// Something stands in the way, in plain words.
    Warn(String),
}

/// What a key or paste did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing the caller needs to know.
    Idle,
    /// The value of this row changed.
    Changed(RowId),
    /// `r` was pressed on a refreshable choice row.
    Refresh(RowId),
    /// Enter, with every visible row complete.
    Submit,
    /// Esc.
    Cancel,
}

/// A step form.
#[derive(Debug, Clone)]
pub struct Form {
    title: String,
    rows: Vec<Row>,
    focus: usize,
    locked: bool,
    summary: Summary,
}

impl Form {
    /// A form of `rows`, focus on the first visible one.
    #[must_use]
    pub fn new(title: &str, rows: Vec<Row>) -> Self {
        let mut form = Self {
            title: title.to_owned(),
            rows,
            focus: 0,
            locked: false,
            summary: Summary::None,
        };
        form.focus = form.first_visible().unwrap_or(0);
        form
    }

    /// The form's title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    // ---- keys ----

    /// A key was pressed. A locked form only answers Enter and Esc.
    pub fn handle_key(&mut self, key: KeyEvent) -> Step {
        match key.code {
            KeyCode::Esc => Step::Cancel,
            KeyCode::Enter => self.submit(),
            _ if self.locked => Step::Idle,
            KeyCode::Up => {
                self.move_focus(false);
                Step::Idle
            }
            KeyCode::Down => {
                self.move_focus(true);
                Step::Idle
            }
            _ => self.edit(key),
        }
    }

    /// Text was pasted. Only a focused text row takes it; control characters (newlines
    /// included) are dropped.
    pub fn paste(&mut self, text: &str) -> Step {
        if self.locked {
            return Step::Idle;
        }
        let Some(row) = self.rows.get_mut(self.focus) else {
            return Step::Idle;
        };
        let Body::Text(input) = &mut row.body else {
            return Step::Idle;
        };
        let mut changed = false;
        for c in text.chars().filter(|c| !c.is_control()) {
            changed |= input
                .handle(InputRequest::InsertChar(c))
                .is_some_and(|state| state.value);
        }
        if changed {
            Step::Changed(row.id)
        } else {
            Step::Idle
        }
    }

    fn submit(&mut self) -> Step {
        match self.rows.iter().position(|row| row.missing().is_some()) {
            None => Step::Submit,
            Some(at) => {
                self.focus = at;
                Step::Idle
            }
        }
    }

    fn edit(&mut self, key: KeyEvent) -> Step {
        let Some(row) = self.rows.get_mut(self.focus) else {
            return Step::Idle;
        };
        let id = row.id;
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL);
        let changed = match &mut row.body {
            Body::Text(input) => input
                .handle_event(&TermEvent::Key(key))
                .is_some_and(|state| state.value),
            Body::Choice {
                options,
                at,
                refresh,
            } => {
                if *refresh && plain && is_refresh_key(key.code) {
                    return Step::Refresh(id);
                }
                let to = match key.code {
                    KeyCode::Right | KeyCode::Char(' ') => next(*at, options.len()),
                    KeyCode::Left => previous(*at, options.len()),
                    _ => *at,
                };
                std::mem::replace(at, to) != to
            }
            Body::Ticks { items, cursor, .. } => match items.get_mut(*cursor) {
                Some(item) if key.code == KeyCode::Char(' ') => {
                    item.on = !item.on;
                    true
                }
                _ => false,
            },
            Body::Toggle(on) => {
                if matches!(
                    key.code,
                    KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                ) {
                    *on = !*on;
                    true
                } else {
                    false
                }
            }
        };
        if changed {
            Step::Changed(id)
        } else {
            Step::Idle
        }
    }

    fn move_focus(&mut self, down: bool) {
        if let Some(Row {
            body:
                Body::Ticks {
                    items,
                    cursor,
                    loading: false,
                },
            ..
        }) = self.rows.get_mut(self.focus)
        {
            if down && cursor.saturating_add(1) < items.len() {
                *cursor = cursor.saturating_add(1);
                return;
            }
            if !down && *cursor > 0 {
                *cursor = cursor.saturating_sub(1);
                return;
            }
        }
        let target = if down {
            (self.focus.saturating_add(1)..self.rows.len()).find(|i| self.is_visible(*i))
        } else {
            (0..self.focus).rev().find(|i| self.is_visible(*i))
        };
        let Some(target) = target else {
            return;
        };
        self.focus = target;
        // Arriving from below lands on the last entry of a list, from above on the first.
        if let Some(Row {
            body: Body::Ticks { items, cursor, .. },
            ..
        }) = self.rows.get_mut(target)
        {
            *cursor = if down {
                0
            } else {
                items.len().saturating_sub(1)
            };
        }
    }

    fn is_visible(&self, index: usize) -> bool {
        self.rows.get(index).is_some_and(|row| row.visible)
    }

    fn first_visible(&self) -> Option<usize> {
        (0..self.rows.len()).find(|i| self.is_visible(*i))
    }

    // ---- state ----

    /// The row with keyboard focus.
    #[must_use]
    pub fn focused(&self) -> Option<RowId> {
        self.rows.get(self.focus).map(|row| row.id)
    }

    /// Move focus to a visible row.
    pub fn focus_on(&mut self, id: RowId) {
        if let Some(at) = self.rows.iter().position(|row| row.id == id && row.visible) {
            self.focus = at;
        }
    }

    /// While locked, rows cannot be changed; Enter and Esc still work.
    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    /// Whether the form is locked.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Replace the summary line.
    pub fn set_summary(&mut self, summary: Summary) {
        self.summary = summary;
    }

    /// The summary line.
    #[must_use]
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Show or hide a row (hidden rows are skipped by focus and by completeness). Hiding the
    /// focused row moves focus to the first visible one.
    pub fn set_visible(&mut self, id: RowId, visible: bool) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.visible = visible;
        }
        if !self.is_visible(self.focus) {
            self.focus = self.first_visible().unwrap_or(0);
        }
    }

    /// Whether a row is shown.
    #[must_use]
    pub fn is_shown(&self, id: RowId) -> bool {
        self.rows.iter().any(|row| row.id == id && row.visible)
    }

    /// Replace a choice row's options, keeping position `at`.
    pub fn set_options(&mut self, id: RowId, options: Vec<String>, at: usize) {
        if let Some(Body::Choice {
            options: shown,
            at: current,
            ..
        }) = self.body_mut(id)
        {
            *current = at.min(options.len().saturating_sub(1));
            *shown = options;
        }
    }

    /// Set a choice row's position.
    pub fn set_choice(&mut self, id: RowId, at: usize) {
        if let Some(Body::Choice {
            options,
            at: current,
            ..
        }) = self.body_mut(id)
        {
            *current = at.min(options.len().saturating_sub(1));
        }
    }

    /// Change what an empty choice or tick list says.
    pub fn set_when_empty(&mut self, id: RowId, text: &str) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.empty = Some(text.to_owned());
        }
    }

    /// Replace a tick list's entries; it is no longer loading.
    pub fn set_ticks(&mut self, id: RowId, items: Vec<Tick>) {
        if let Some(Body::Ticks {
            items: shown,
            cursor,
            loading,
        }) = self.body_mut(id)
        {
            *cursor = (*cursor).min(items.len().saturating_sub(1));
            *shown = items;
            *loading = false;
        }
    }

    /// A tick list that is being filled shows "looking" and is incomplete.
    pub fn set_loading(&mut self, id: RowId, loading_now: bool) {
        if let Some(Body::Ticks { loading, .. }) = self.body_mut(id) {
            *loading = loading_now;
        }
    }

    fn body_mut(&mut self, id: RowId) -> Option<&mut Body> {
        self.rows
            .iter_mut()
            .find(|row| row.id == id)
            .map(|row| &mut row.body)
    }

    fn body(&self, id: RowId) -> Option<&Body> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| &row.body)
    }

    /// The text of a text row.
    #[must_use]
    pub fn text(&self, id: RowId) -> &str {
        match self.body(id) {
            Some(Body::Text(input)) => input.value(),
            _ => "",
        }
    }

    /// The position of a choice row.
    #[must_use]
    pub fn choice(&self, id: RowId) -> usize {
        match self.body(id) {
            Some(Body::Choice { at, .. }) => *at,
            _ => 0,
        }
    }

    /// Whether a toggle row is on.
    #[must_use]
    pub fn toggle(&self, id: RowId) -> bool {
        matches!(self.body(id), Some(Body::Toggle(true)))
    }

    /// The entries of a tick list.
    #[must_use]
    pub fn ticks(&self, id: RowId) -> &[Tick] {
        match self.body(id) {
            Some(Body::Ticks { items, .. }) => items,
            _ => &[],
        }
    }

    /// The positions of the ticked entries of a tick list.
    #[must_use]
    pub fn ticked(&self, id: RowId) -> Vec<usize> {
        self.ticks(id)
            .iter()
            .enumerate()
            .filter(|(_, item)| item.on)
            .map(|(at, _)| at)
            .collect()
    }

    // ---- completeness and hints ----

    /// What the first incomplete visible row still needs, in a few words.
    #[must_use]
    pub fn missing(&self) -> Option<String> {
        self.rows.iter().find_map(Row::missing)
    }

    /// Whether every visible row is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing().is_none()
    }

    /// The keys that work now. `enter` names what Enter does (it is offered only when the form is
    /// complete; otherwise the footer says what is missing instead); `None` when Enter does
    /// nothing at this step.
    #[must_use]
    pub fn hints(&self, enter: Option<Key>) -> Vec<Hint> {
        let mut hints = Vec::new();
        if !self.locked {
            hints.extend(self.focus_hints());
        }
        let missing = enter.and_then(|_| self.missing());
        if let Some(enter) = enter.filter(|_| missing.is_none()) {
            hints.push(enter.hint());
        }
        hints.push(keys::CANCEL.hint());
        hints.extend(missing.map(Hint::Missing));
        hints
    }

    fn focus_hints(&self) -> Vec<Hint> {
        let mut hints = Vec::new();
        let shown = self.rows.iter().filter(|row| row.visible).count();
        let Some(row) = self.rows.get(self.focus) else {
            return hints;
        };
        let roomy = |len: usize| len > 1;
        match &row.body {
            Body::Text(_) => {}
            Body::Choice {
                options, refresh, ..
            } => {
                if roomy(options.len()) {
                    hints.push(keys::FORM_CHANGE.hint());
                }
                if *refresh {
                    hints.push(keys::REFRESH.hint());
                }
            }
            Body::Ticks { items, loading, .. } => {
                if !*loading && !items.is_empty() {
                    hints.push(keys::FORM_TICK.hint());
                }
            }
            Body::Toggle(_) => hints.push(keys::FORM_SWITCH.hint()),
        }
        if roomy(shown) || matches!(&row.body, Body::Ticks { items, .. } if roomy(items.len())) {
            hints.insert(0, keys::FORM_MOVE.hint());
        }
        hints
    }
}

fn is_refresh_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::Char(c) if keys::REFRESH.key.starts_with(c))
}

fn next(at: usize, len: usize) -> usize {
    at.saturating_add(1).checked_rem(len).unwrap_or(0)
}

fn previous(at: usize, len: usize) -> usize {
    if at == 0 {
        len.saturating_sub(1)
    } else {
        at.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests;
