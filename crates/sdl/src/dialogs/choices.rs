//! What Sync device and Convert both ask in the same way: the `What` tick list (Whole library,
//! then each playlist) and the Quality dropdown whose labels follow the format, and the readers
//! that turn a form's values back into the choices the task layer takes.

use sdl_core::{Format, Quality, Selection};
use sdl_form::{Choice, Form, Key, Value};

/// The option id of Whole library; every other option id is a playlist's name.
pub(super) const WHOLE: &str = "whole";

/// Whole library, then each playlist.
pub(super) fn what_options(playlists: &[String]) -> Vec<Choice> {
    let mut options = vec![Choice::new(WHOLE, "Whole library")];
    options.extend(
        playlists
            .iter()
            .map(|name| Choice::new(name.clone(), name.clone())),
    );
    options
}

/// A remembered selection as the tick list's value.
pub(super) fn what_value(selection: &Selection) -> Value {
    let mut ticked = selection.playlists.clone();
    if selection.whole_library {
        ticked.push(WHOLE.to_owned());
    }
    Value::Ticked(ticked)
}

/// The selection the tick list `key` holds now.
pub(super) fn selection_of(form: &Form, key: Key) -> Selection {
    let ticked = ticked_of(form, key);
    Selection {
        whole_library: ticked.iter().any(|id| id == WHOLE),
        playlists: ticked.into_iter().filter(|id| id != WHOLE).collect(),
    }
}

/// The ids ticked in `key`.
pub(super) fn ticked_of(form: &Form, key: Key) -> Vec<String> {
    match form.value(key) {
        Some(Value::Ticked(ids)) => ids,
        _ => Vec::new(),
    }
}

/// The three qualities, each labelled with its bitrate for `format`.
pub(super) fn quality_options(format: Format) -> Vec<Choice> {
    Quality::ALL
        .iter()
        .map(|quality| Choice::new(quality.name(), quality.label(format)))
        .collect()
}

/// The quality chosen in `key` (the default one when none is).
pub(super) fn quality_of(form: &Form, key: Key) -> Quality {
    let id = chosen_of(form, key);
    Quality::ALL
        .into_iter()
        .find(|quality| Some(quality.name()) == id.as_deref())
        .unwrap_or_default()
}

/// The id chosen in the dropdown or radio group `key`.
pub(super) fn chosen_of(form: &Form, key: Key) -> Option<String> {
    match form.value(key) {
        Some(Value::Choice(id)) => id,
        _ => None,
    }
}

/// The text of `key`, trimmed.
pub(super) fn text_of(form: &Form, key: Key) -> String {
    match form.value(key) {
        Some(Value::Text(text)) => text.trim().to_owned(),
        _ => String::new(),
    }
}

/// The toggle `key`.
pub(super) fn flag_of(form: &Form, key: Key) -> bool {
    matches!(form.value(key), Some(Value::Flag(true)))
}

#[cfg(test)]
mod tests {
    use sdl_form::Field;

    use super::*;

    #[test]
    fn a_selection_round_trips_through_the_tick_list() {
        let selection = Selection {
            whole_library: true,
            playlists: vec!["Road trip".into()],
        };
        let mut form = Form::new("F").field(
            Field::ticks("what", "What").options(what_options(&["Road trip".into(), "Gym".into()])),
        );
        form.set("what", what_value(&selection));
        assert_eq!(selection_of(&form, "what"), selection);
    }

    #[test]
    fn quality_labels_follow_the_format() {
        let mp3 = quality_options(Format::Mp3);
        let m4a = quality_options(Format::M4a);
        assert_eq!(mp3.len(), 3);
        assert_ne!(
            mp3.first().map(Choice::label),
            m4a.first().map(Choice::label)
        );
    }
}
