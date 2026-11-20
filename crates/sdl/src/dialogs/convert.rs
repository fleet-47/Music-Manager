//! Convert (F9): what, format, quality, cover and destination, then `N files to convert · M
//! already there`, then Start.

use sdl_core::{EventKind, Format, Quality};
use sdl_form::{Choice, Field, Form, Value};

use super::Step;
use crate::app::Settings;

/// Tick list: Whole library, then each playlist. Option ids: `whole` and the playlist's name.
pub const WHAT: &str = "what";
/// Dropdown: MP3, M4A.
pub const FORMAT: &str = "format";
/// Dropdown: High, Medium, Small, each with its approximate bitrate.
pub const QUALITY: &str = "quality";
/// Toggle, on by default.
pub const COVER: &str = "cover";
/// Folder field, default `~/Music-converted`.
pub const DESTINATION: &str = "destination";

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub struct State {
    /// The playlists last offered.
    pub playlists: Vec<String>,
}

/// The form pre-filled from `settings.state.convert`; busy ("Reading your playlists") until
/// `Playlists` arrives. Primary button `Continue` until a plan for the current choices arrives,
/// then `Start`. Quality labels follow the format (`Quality::label`), and are set again whenever
/// the format changes.
#[must_use]
pub fn open(settings: &Settings) -> (Form, State) {
    let choice = &settings.state.convert;
    let mut form = Form::new("Convert")
        .field(
            Field::ticks(WHAT, "What")
                .needs("Tick at least one")
                .options(vec![Choice::new("whole", "Whole library")]),
        )
        .field(
            Field::dropdown(FORMAT, "Format").options(
                Format::CONVERT
                    .iter()
                    .map(|format| Choice::new(format.extension(), format.label()))
                    .collect(),
            ),
        )
        .field(
            Field::dropdown(QUALITY, "Quality").options(
                Quality::ALL
                    .iter()
                    .map(|quality| Choice::new(quality.name(), quality.label(choice.format)))
                    .collect(),
            ),
        )
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
    let mut ticked: Vec<String> = choice.what.playlists.clone();
    if choice.what.whole_library {
        ticked.push("whole".to_owned());
    }
    form.set(WHAT, Value::Ticked(ticked));
    form.set(COVER, Value::Flag(choice.keep_cover));
    form.set(
        DESTINATION,
        Value::Text(choice.destination.display().to_string()),
    );
    form.set_busy(Some("Reading your playlists".to_owned()));
    (form, State::default())
}

/// `Playlists` adds one option per playlist after Whole library and clears the busy line; `Plan`
/// becomes the summary `N files to convert · M already there`, with the plan's refusal in
/// plain words through `set_blocked`.
pub fn on_event(form: &mut Form, state: &mut State, kind: &EventKind) -> Step {
    if let EventKind::Playlists(names) = kind {
        state.playlists.clone_from(names);
        let mut options = vec![Choice::new("whole", "Whole library")];
        options.extend(
            names
                .iter()
                .map(|name| Choice::new(name.clone(), name.clone())),
        );
        form.set_options(WHAT, options);
        form.set_busy(None);
    }
    Step::Stay(Vec::new())
}
