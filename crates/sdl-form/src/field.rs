//! The fields of a form: what each holds, and how it is built.

use std::collections::BTreeSet;

use ratatui_explorer::FileExplorer;
use tui_input::Input;
use tui_scrollbar::ScrollBarInteraction;

use crate::filter::rank;

/// A field's name inside its form; the app reads and updates a field by it.
pub type Key = &'static str;

/// A piece of an option's label. A flagged part is drawn in the flagged style (review shows the
/// parts of a candidate that differ from what was wanted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The text.
    pub text: String,
    /// Whether it is drawn flagged.
    pub flagged: bool,
}

impl Part {
    /// A plain part.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            flagged: false,
        }
    }

    /// A flagged part.
    #[must_use]
    pub fn flagged(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            flagged: true,
        }
    }
}

/// One option of a dropdown, radio group, tick list or list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Stable id: what values and remembered choices refer to. It survives
    /// [`crate::Form::set_options`]; a label does not have to.
    pub id: String,
    /// The label, in parts; drawn joined with ` · `, and what typing filters on.
    pub parts: Vec<Part>,
    /// Secondary text drawn after the label (`2,117 songs`, `12.3 GB free`); not filtered on.
    pub detail: String,
}

impl Choice {
    /// An option with a plain label.
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::from_parts(id, vec![Part::plain(label)])
    }

    /// An option whose label has several parts.
    #[must_use]
    pub fn from_parts(id: impl Into<String>, parts: Vec<Part>) -> Self {
        Self {
            id: id.into(),
            parts,
            detail: String::new(),
        }
    }

    /// The same option with secondary text.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// The label as one string: the parts joined with ` · `.
    #[must_use]
    pub fn label(&self) -> String {
        let texts: Vec<&str> = self.parts.iter().map(|part| part.text.as_str()).collect();
        texts.join(" · ")
    }
}

/// What a field holds, as the app reads and writes it ([`crate::Form::value`],
/// [`crate::Form::set`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Text and folder fields: the text as typed.
    Text(String),
    /// Dropdown and radio group: the id of the chosen option, `None` when nothing is chosen.
    Choice(Option<String>),
    /// Tick list: the ids of the ticked options, in option order.
    Ticked(Vec<String>),
    /// Toggle.
    Flag(bool),
}

/// Checks a text field's value: `Err` carries the plain message shown under the field.
pub type Validate = fn(&str) -> Result<(), String>;

/// A text box: `tui-input` plus what is shown while empty and how it is checked.
#[derive(Debug, Clone)]
pub(crate) struct Text {
    pub(crate) input: Input,
    pub(crate) placeholder: String,
    pub(crate) validate: Option<Validate>,
}

impl Text {
    fn new() -> Self {
        Self {
            input: Input::default(),
            placeholder: String::new(),
            validate: None,
        }
    }

    /// The message under the field when the value is not acceptable. An empty value is "missing"
    /// (the field's `needs`), not "not acceptable".
    pub(crate) fn complaint(&self) -> Option<String> {
        let value = self.input.value();
        if value.is_empty() {
            return None;
        }
        self.validate.and_then(|check| check(value).err())
    }
}

/// A single choice among options: dropdown and radio group.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pick {
    pub(crate) options: Vec<Choice>,
    /// Id of the chosen option.
    pub(crate) chosen: Option<String>,
    /// Highlighted option while a dropdown is open.
    pub(crate) cursor: usize,
    /// Whether a dropdown shows its option list.
    pub(crate) open: bool,
    /// Shown instead of the options when there are none.
    pub(crate) empty: String,
}

/// A list with a filter box: tick list (`ticks`) and list.
#[derive(Debug, Clone, Default)]
pub(crate) struct Rows {
    pub(crate) options: Vec<Choice>,
    /// Ids of ticked options (tick list only). Kept when an option goes, so a list that arrives
    /// again is still ticked.
    pub(crate) ticked: BTreeSet<String>,
    pub(crate) filter: Input,
    /// Highlighted row among the listed rows.
    pub(crate) cursor: usize,
    /// First listed row shown.
    pub(crate) offset: usize,
    /// Shown instead of the rows when there are none.
    pub(crate) empty: String,
    /// Scrollbar drag state.
    pub(crate) drag: ScrollBarInteraction,
}

impl Rows {
    /// Positions in `options` of the rows listed now: all of them, or those matching the filter,
    /// best match first.
    pub(crate) fn listed(&self) -> Vec<usize> {
        let labels: Vec<String> = self.options.iter().map(Choice::label).collect();
        rank(&labels, self.filter.value())
    }

    /// Move the highlight by `delta` rows among the listed rows, staying inside them, and scroll
    /// so it stays visible inside `rows` rows.
    pub(crate) fn step(&mut self, delta: isize, rows: usize) {
        let count = self.listed().len();
        let last = count.saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
        self.reveal(rows);
    }

    /// Scroll so the highlight is inside `rows` visible rows.
    pub(crate) fn reveal(&mut self, rows: usize) {
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if rows > 0 && self.cursor >= self.offset.saturating_add(rows) {
            self.offset = self.cursor.saturating_add(1).saturating_sub(rows);
        }
    }
}

/// A text field plus a Browse action that opens `ratatui-explorer`.
#[derive(Debug, Clone)]
pub(crate) struct Folder {
    pub(crate) text: Text,
    /// The open folder browser, if any.
    pub(crate) browser: Option<Box<FileExplorer>>,
}

/// What a field is.
#[derive(Debug, Clone)]
pub(crate) enum Kind {
    Text(Text),
    Dropdown(Pick),
    Radio(Pick),
    Ticks(Box<Rows>),
    List(Box<Rows>),
    Toggle(bool),
    Folder(Folder),
}

/// One field of a form. Build it with a constructor and chain the settings.
#[derive(Debug, Clone)]
pub struct Field {
    pub(crate) key: Key,
    pub(crate) label: String,
    pub(crate) kind: Kind,
    pub(crate) visible: bool,
    /// Shown under the summary while the field cannot let the form continue (empty text, nothing
    /// chosen, nothing ticked).
    pub(crate) needs: Option<String>,
}

impl Field {
    fn new(key: Key, label: impl Into<String>, kind: Kind) -> Self {
        Self {
            key,
            label: label.into(),
            kind,
            visible: true,
            needs: None,
        }
    }

    /// A text field (`tui-input`; paste works).
    #[must_use]
    pub fn text(key: Key, label: impl Into<String>) -> Self {
        Self::new(key, label, Kind::Text(Text::new()))
    }

    /// A dropdown: shows its current value; Enter, Space or a click opens the full option list.
    #[must_use]
    pub fn dropdown(key: Key, label: impl Into<String>) -> Self {
        Self::new(key, label, Kind::Dropdown(Pick::default()))
    }

    /// A radio group: every option visible at once; Up/Down move and choose.
    #[must_use]
    pub fn radio(key: Key, label: impl Into<String>) -> Self {
        Self::new(key, label, Kind::Radio(Pick::default()))
    }

    /// A tick list: a filter box above [`crate::LIST_ROWS`] rows, Space ticks, and a line always
    /// shows `5 of 34 ticked`.
    #[must_use]
    pub fn ticks(key: Key, label: impl Into<String>) -> Self {
        Self::new(key, label, Kind::Ticks(Box::default()))
    }

    /// A filterable list whose rows are activated, not ticked: Enter or a click on a row gives
    /// [`crate::Reaction::Activated`]. Review's list of held songs.
    #[must_use]
    pub fn list(key: Key, label: impl Into<String>) -> Self {
        Self::new(key, label, Kind::List(Box::default()))
    }

    /// A toggle.
    #[must_use]
    pub fn toggle(key: Key, label: impl Into<String>) -> Self {
        Self::new(key, label, Kind::Toggle(false))
    }

    /// A folder field: a text field plus a Browse action that opens `ratatui-explorer`; Enter in
    /// the browser chooses the folder it shows.
    #[must_use]
    pub fn folder(key: Key, label: impl Into<String>) -> Self {
        Self::new(
            key,
            label,
            Kind::Folder(Folder {
                text: Text::new(),
                browser: None,
            }),
        )
    }

    /// Text and folder fields: what is shown while empty.
    #[must_use]
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        if let Kind::Text(Text { placeholder, .. })
        | Kind::Folder(Folder {
            text: Text { placeholder, .. },
            ..
        }) = &mut self.kind
        {
            *placeholder = text.into();
        }
        self
    }

    /// Text and folder fields: how the value is checked. The message is shown under the field and
    /// among what is missing.
    #[must_use]
    pub fn validate(mut self, check: Validate) -> Self {
        if let Kind::Text(Text { validate, .. })
        | Kind::Folder(Folder {
            text: Text { validate, .. },
            ..
        }) = &mut self.kind
        {
            *validate = Some(check);
        }
        self
    }

    /// The field must be filled for the form to continue; `message` is what the form says is
    /// missing (`Paste a link`, `Tick at least one list`). Without it, the field may stay empty.
    #[must_use]
    pub fn needs(mut self, message: impl Into<String>) -> Self {
        self.needs = Some(message.into());
        self
    }

    /// Dropdowns, radio groups and lists: what is shown when there are no options
    /// (`No device found. Plug one in.`).
    #[must_use]
    pub fn when_empty(mut self, text: impl Into<String>) -> Self {
        match &mut self.kind {
            Kind::Dropdown(pick) | Kind::Radio(pick) => pick.empty = text.into(),
            Kind::Ticks(rows) | Kind::List(rows) => rows.empty = text.into(),
            _ => {}
        }
        self
    }

    /// The options of a dropdown, radio group, tick list or list.
    #[must_use]
    pub fn options(mut self, options: Vec<Choice>) -> Self {
        self.replace_options(options);
        self
    }

    /// The field starts hidden: not drawn, not focusable, and not counted among what is missing,
    /// until [`crate::Form::set_visible`] shows it.
    #[must_use]
    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }

    /// Whether the field is drawn, focusable and counted.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The field's key.
    #[must_use]
    pub fn key(&self) -> Key {
        self.key
    }

    /// Replace the options, keeping what the owner chose where the option is still there (by
    /// id): the chosen option, the ticks (ticks of options that went are kept, so a list that
    /// arrives again is still ticked), and the highlighted row where it still exists.
    pub(crate) fn replace_options(&mut self, options: Vec<Choice>) {
        match &mut self.kind {
            Kind::Dropdown(pick) | Kind::Radio(pick) => {
                if !pick
                    .chosen
                    .as_ref()
                    .is_some_and(|id| options.iter().any(|option| &option.id == id))
                {
                    pick.chosen = None;
                }
                pick.options = options;
                pick.cursor = pick.cursor.min(pick.options.len().saturating_sub(1));
            }
            Kind::Ticks(rows) | Kind::List(rows) => {
                rows.options = options;
                let last = rows.listed().len().saturating_sub(1);
                rows.cursor = rows.cursor.min(last);
                rows.offset = rows.offset.min(rows.cursor);
            }
            _ => {}
        }
    }

    /// What the field holds.
    pub(crate) fn value(&self) -> Option<Value> {
        match &self.kind {
            Kind::Text(text) => Some(Value::Text(text.input.value().to_owned())),
            Kind::Folder(folder) => Some(Value::Text(folder.text.input.value().to_owned())),
            Kind::Dropdown(pick) | Kind::Radio(pick) => Some(Value::Choice(pick.chosen.clone())),
            Kind::Ticks(rows) => Some(Value::Ticked(
                rows.options
                    .iter()
                    .filter(|option| rows.ticked.contains(&option.id))
                    .map(|option| option.id.clone())
                    .collect(),
            )),
            Kind::Toggle(on) => Some(Value::Flag(*on)),
            Kind::List(_) => None,
        }
    }

    /// Put `value` in the field. A value of the wrong shape changes nothing. A chosen id with no
    /// such option is kept: the options may arrive later.
    pub(crate) fn set(&mut self, value: Value) {
        match (&mut self.kind, value) {
            (Kind::Text(text) | Kind::Folder(Folder { text, .. }), Value::Text(value)) => {
                text.input = Input::new(value);
            }
            (Kind::Dropdown(pick) | Kind::Radio(pick), Value::Choice(id)) => {
                if let Some(at) = id
                    .as_ref()
                    .and_then(|id| pick.options.iter().position(|option| &option.id == id))
                {
                    pick.cursor = at;
                }
                pick.chosen = id;
            }
            (Kind::Ticks(rows), Value::Ticked(ids)) => rows.ticked = ids.into_iter().collect(),
            (Kind::Toggle(on), Value::Flag(value)) => *on = value,
            _ => {}
        }
    }

    /// Whether the field stands in the way of continuing, and what it says.
    pub(crate) fn missing(&self) -> Option<String> {
        let complaint = match &self.kind {
            Kind::Text(text) => text.complaint(),
            Kind::Folder(folder) => folder.text.complaint(),
            _ => None,
        };
        if complaint.is_some() {
            return complaint;
        }
        let needs = self.needs.as_ref()?;
        let unmet = match &self.kind {
            Kind::Text(text) => text.input.value().trim().is_empty(),
            Kind::Folder(folder) => folder.text.input.value().trim().is_empty(),
            Kind::Dropdown(pick) | Kind::Radio(pick) => pick.chosen.is_none(),
            Kind::Ticks(rows) => rows.ticked.is_empty(),
            Kind::List(_) | Kind::Toggle(_) => false,
        };
        unmet.then(|| needs.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<Choice> {
        vec![Choice::new("a", "Alpha"), Choice::new("b", "Beta")]
    }

    #[test]
    fn a_chosen_option_survives_new_options_only_while_it_exists() {
        let mut field = Field::radio("r", "R").options(options());
        field.set(Value::Choice(Some("b".into())));
        field.replace_options(vec![Choice::new("b", "Beta 2"), Choice::new("c", "Gamma")]);
        assert_eq!(field.value(), Some(Value::Choice(Some("b".into()))));
        field.replace_options(vec![Choice::new("c", "Gamma")]);
        assert_eq!(field.value(), Some(Value::Choice(None)));
    }

    #[test]
    fn ticks_come_back_in_option_order_and_survive_a_list_that_goes_and_returns() {
        let mut field = Field::ticks("t", "T").options(options());
        field.set(Value::Ticked(vec!["b".into(), "a".into()]));
        assert_eq!(
            field.value(),
            Some(Value::Ticked(vec!["a".into(), "b".into()]))
        );
        field.replace_options(Vec::new());
        assert_eq!(field.value(), Some(Value::Ticked(Vec::new())));
        field.replace_options(options());
        assert_eq!(
            field.value(),
            Some(Value::Ticked(vec!["a".into(), "b".into()]))
        );
    }

    #[test]
    fn a_needed_field_says_what_is_missing_until_it_is_filled() {
        let mut field = Field::text("link", "Link").needs("Paste a link");
        assert_eq!(field.missing().as_deref(), Some("Paste a link"));
        field.set(Value::Text("https://x".into()));
        assert_eq!(field.missing(), None);
    }

    #[test]
    fn a_value_that_is_not_acceptable_is_reported_in_the_checks_words() {
        fn check(text: &str) -> Result<(), String> {
            text.starts_with("http")
                .then_some(())
                .ok_or_else(|| "That is not a link".to_owned())
        }
        let mut field = Field::text("link", "Link").validate(check);
        assert_eq!(field.missing(), None);
        field.set(Value::Text("nope".into()));
        assert_eq!(field.missing().as_deref(), Some("That is not a link"));
    }

    #[test]
    fn a_hidden_field_starts_hidden_and_labels_join_their_parts() {
        let field = Field::toggle("t", "T").hidden();
        assert!(!field.is_visible());
        let choice = Choice::from_parts("x", vec![Part::plain("Song"), Part::flagged("Artist")]);
        assert_eq!(choice.label(), "Song · Artist");
    }

    #[test]
    fn the_highlight_stays_inside_the_listed_rows_and_visible() {
        let many: Vec<Choice> = (0..20)
            .map(|n| Choice::new(format!("{n}"), format!("Row {n}")))
            .collect();
        let mut field = Field::ticks("t", "T").options(many);
        if let Kind::Ticks(rows) = &mut field.kind {
            rows.step(100, 6);
            assert_eq!((rows.cursor, rows.offset), (19, 14));
            rows.step(-100, 6);
            assert_eq!((rows.cursor, rows.offset), (0, 0));
        }
    }
}
