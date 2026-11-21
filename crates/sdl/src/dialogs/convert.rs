//! Convert (F9): what, format, quality, cover and destination, then `N files to convert · M
//! already there`, then Start. A change to a dropdown, tick list or toggle makes a new plan at
//! once; typing a destination only clears the summary, so no plan is made per keystroke.

use std::path::PathBuf;

use sdl_core::{ConvertChoice, EventKind, Format};
use sdl_form::{Choice, Field, Form, Value};
use sdl_tasks::Answers;

use super::choices::{
    chosen_of, flag_of, quality_of, quality_options, selection_of, text_of, what_options,
    what_value,
};
use super::conversation::Conversation;
use crate::app::Settings;
use sdl_core::words::{files, thousands};

/// Tick list: Whole library, then each playlist.
pub(super) const WHAT: &str = "what";
/// Dropdown: MP3, M4A.
pub(super) const FORMAT: &str = "format";
/// Dropdown: High, Medium, Small, each with its approximate bitrate.
pub(super) const QUALITY: &str = "quality";
/// Toggle.
pub(super) const COVER: &str = "cover";
/// Folder field.
pub(super) const DESTINATION: &str = "destination";

/// The form pre-filled from `settings.state.convert`, waiting for the playlists.
pub(super) fn open(settings: &Settings) -> (Form, Conversation) {
    let choice = &settings.state.convert;
    let mut form = Form::new("Convert")
        .field(
            Field::ticks(WHAT, "What")
                .needs("Tick what to convert")
                .options(what_options(&[])),
        )
        .field(
            Field::dropdown(FORMAT, "Format").options(
                Format::CONVERT
                    .iter()
                    .map(|format| Choice::new(format.extension(), format.label()))
                    .collect(),
            ),
        )
        .field(Field::dropdown(QUALITY, "Quality").options(quality_options(choice.format)))
        .field(Field::toggle(COVER, "Keep cover art"))
        .field(Field::folder(DESTINATION, "Destination").needs("Choose a destination"));
    form.set(
        FORMAT,
        Value::Choice(Some(choice.format.extension().to_owned())),
    );
    form.set(
        QUALITY,
        Value::Choice(Some(choice.quality.name().to_owned())),
    );
    form.set(WHAT, what_value(&choice.what));
    form.set(COVER, Value::Flag(choice.keep_cover));
    form.set(
        DESTINATION,
        Value::Text(choice.destination.display().to_string()),
    );
    let conv = Conversation::planned("Making the plan", Some("Reading your playlists"));
    (form, conv)
}

/// `Playlists` adds one option per playlist; `Plan` is the summary, and its refusal (a destination
/// inside the library) is said in the task's plain words.
pub(super) fn on_event(form: &mut Form, conv: &mut Conversation, kind: &EventKind) {
    match kind {
        EventKind::Playlists(names) => {
            form.set_options(WHAT, what_options(names));
            conv.wait_for(None);
        }
        EventKind::Plan(plan) => {
            let summary = format!(
                "{} to convert · {} already there",
                files(plan.files),
                thousands(plan.already)
            );
            if conv.plan(summary) {
                form.set_blocked(plan.refusal.clone());
            }
        }
        EventKind::Headline(text) => conv.headline(text),
        _ => {}
    }
}

/// A field changed. The quality labels follow the format. Returns whether a plan should be asked
/// for now (not for typing a destination).
pub(super) fn changed(form: &mut Form, field: &str) -> bool {
    if field == FORMAT {
        let format = format_of(form);
        form.set_options(QUALITY, quality_options(format));
    }
    field != DESTINATION
}

fn format_of(form: &Form) -> Format {
    let id = chosen_of(form, FORMAT);
    Format::CONVERT
        .into_iter()
        .find(|format| Some(format.extension()) == id.as_deref())
        .unwrap_or(Format::Mp3)
}

/// The choices in the form.
pub(super) fn answers(form: &Form) -> Answers {
    Answers::Convert(ConvertChoice {
        what: selection_of(form, WHAT),
        format: format_of(form),
        quality: quality_of(form, QUALITY),
        keep_cover: flag_of(form, COVER),
        destination: PathBuf::from(text_of(form, DESTINATION)),
    })
}
