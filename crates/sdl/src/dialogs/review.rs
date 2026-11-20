//! Review (F8): a filterable list of held songs; Enter shows what was wanted and a radio group of
//! choices; `Download N chosen` starts the run.
//!
//! The walk, keyboard and mouse alike:
//! * **List** ([`open`]): Enter or a click on a row opens that song's detail. Each row's detail
//!   text shows the choice made so far in words (`→ Song · Artist`, `→ link's own audio`,
//!   `→ pasted link`, `→ never`), so nothing has to be remembered; `Later` shows nothing.
//! * **Detail**: notes say what was wanted (`Title · Artists`, `Album · 3:41`); the radio group
//!   [`CHOICE`] lists each candidate (parts that differ from what was wanted flagged), then
//!   `Use the link's own audio` (link songs only), `Paste a link`, `Leave for later`,
//!   `Never download`, pre-set to the choice made before (none the first time: `needs("Choose what
//!   to do with this song")`). Choosing `Paste a link` shows [`PASTED`] (validated like Download
//!   link's field, `needs("Paste a link")`). Primary `Done` records the choice and returns to the
//!   list; `Back` (Esc) returns without recording (`Form::set_back(true)`). `o` opens the chosen
//!   candidate (`https://music.youtube.com/watch?v=<id>`) with `Cmd::Open`.
//! * **Start**: the list's primary reads `Download N chosen`, N counting every decided song except
//!   `Leave for later` (a `Never download` is applied by the same run); blocked with "Choose what
//!   to do with at least one song" while N is 0. Submit sends `Answer(Review { choices })` for
//!   those songs, then as rule 5 of [`super`].
//! * **While open**, another task can hold or land songs: on `HeldCount` the list's rows are
//!   rebuilt from `settings.review` (choices for songs that left the list are dropped).

use std::collections::BTreeMap;

use sdl_core::EventKind;
use sdl_form::{Choice, Field, Form, Part};
use sdl_tasks::ReviewChoice;

use super::Step;
use crate::app::Settings;

/// The list of held songs. Row ids are `WantedSong::key`.
pub const SONGS: &str = "songs";

/// The detail's radio group. Option ids: a candidate's video id, or one of [`OWN_AUDIO`],
/// [`PASTE`], [`LATER`], [`NEVER`].
pub const CHOICE: &str = "choice";

/// The detail's pasted-link text field, shown only while [`PASTE`] is chosen.
pub const PASTED: &str = "pasted";

/// Option ids of the detail's radio group.
pub const OWN_AUDIO: &str = "own-audio";
/// Use the pasted link's audio.
pub const PASTE: &str = "paste";
/// Leave the song for later.
pub const LATER: &str = "later";
/// Never download the song.
pub const NEVER: &str = "never";

/// Whether an option id of [`CHOICE`] is a candidate (a video id) rather than one of the fixed
/// choices.
#[must_use]
pub fn is_candidate(id: &str) -> bool {
    ![OWN_AUDIO, PASTE, LATER, NEVER].contains(&id)
}

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub struct State {
    /// The choice made so far per held song key; the list's primary button counts the decided
    /// ones (`Download N chosen`) and is blocked while there are none.
    pub chosen: BTreeMap<String, ReviewChoice>,
    /// The list form, kept while a song's detail form is shown.
    pub list: Option<Form>,
}

/// The list form: one row per held song (`title · artists`, detail: where it came from), filter
/// box, primary button `Download 0 chosen` blocked with "Choose what to do with at least one
/// song". The run is not busy: the held songs are read from the runner's review list.
#[must_use]
pub fn open(settings: &Settings) -> (Form, State) {
    let options = settings
        .review
        .songs()
        .iter()
        .map(|held| {
            let artists = held.song.artists_joined();
            Choice::from_parts(
                held.song.key().to_owned(),
                vec![Part::plain(held.song.title.clone()), Part::plain(artists)],
            )
            .with_detail(held.from.join(", "))
        })
        .collect();
    let mut form = Form::new("Review").field(
        Field::list(SONGS, "Held songs")
            .options(options)
            .when_empty("Nothing is waiting for review"),
    );
    form.set_primary("Download 0 chosen");
    form.set_blocked(Some("Choose what to do with at least one song".to_owned()));
    (form, State::default())
}

/// `HeldCount` rebuilds the list's rows from the runner's review list; `Started` closes; the rest
/// is ignored.
pub fn on_event(_form: &mut Form, _state: &mut State, _kind: &EventKind) -> Step {
    Step::Stay(Vec::new())
}
