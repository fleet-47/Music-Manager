//! Review (F8): a filterable list of held songs; Enter shows what was wanted beside the
//! candidates; `Download N chosen` starts the run.
//!
//! The walk, keyboard and mouse alike:
//! * **List** ([`open`]): Enter or a click on a row opens that song's detail. Each row shows the
//!   choice made so far in words (`→ Song · Artist`, `→ link's own audio`, `→ pasted link`,
//!   `→ never`), so nothing has to be remembered; `Leave for later` shows nothing.
//! * **Detail**: notes say what was wanted and why it was held; the radio group [`CHOICE`] lists
//!   each candidate (the parts that differ from what was wanted flagged), then `Use the link's own
//!   audio` (link songs only), `Paste a link`, `Leave for later`, `Never download`, pre-set to the
//!   choice made before. `Paste a link` shows [`PASTED`]. `Done` records the choice and returns to
//!   the list; `Back` (Esc) returns without recording. `o` opens the chosen candidate.
//! * **Start**: the list's primary reads `Download N chosen`, N counting every decided song except
//!   `Leave for later`; it is blocked while N is 0.
//! * **While open**, another task can hold or land songs: on `HeldCount` the rows are rebuilt from
//!   `settings.review`; choices for songs that left are dropped.

use std::collections::BTreeMap;
use std::time::Duration;

use sdl_core::{Candidate, EventKind, HeldSong};
use sdl_form::{Choice, Field, Form, Part, Value};
use sdl_tasks::{Answers, Resolution, ReviewChoice};

use super::choices::{chosen_of, text_of};
use super::link;
use crate::app::Settings;
use sdl_core::words::thousands;

/// The list of held songs. Row ids are `WantedSong::key`.
pub(super) const SONGS: &str = "songs";

/// The detail's radio group. Option ids: a candidate's video id, or one of [`OWN_AUDIO`],
/// [`PASTE`], [`LATER`], [`NEVER`].
pub(super) const CHOICE: &str = "choice";

/// The detail's pasted-link text field, shown only while [`PASTE`] is chosen.
pub(super) const PASTED: &str = "pasted";

/// Use the link's own audio.
pub(super) const OWN_AUDIO: &str = "own-audio";
/// Use the pasted link's audio.
pub(super) const PASTE: &str = "paste";
/// Leave the song for later.
pub(super) const LATER: &str = "later";
/// Never download the song.
pub(super) const NEVER: &str = "never";

/// How far a candidate's length may differ from the wanted one before it is flagged: the matching
/// rule's own limit (F3).
const LENGTH_SLACK: Duration = Duration::from_secs(3);

/// Whether an option id of [`CHOICE`] is a candidate (a video id) rather than a fixed choice.
#[must_use]
pub fn is_candidate(id: &str) -> bool {
    ![OWN_AUDIO, PASTE, LATER, NEVER].contains(&id)
}

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The choice made so far per held song key.
    chosen: BTreeMap<String, Resolution>,
    /// The list form, kept while a song's detail form is shown.
    pub(super) list: Option<Form>,
    /// The key of the song whose detail is shown.
    detail: Option<String>,
}

/// The list form: one row per held song, the filter box, and `Download 0 chosen`, blocked.
pub(super) fn open(settings: &Settings) -> (Form, State) {
    let state = State::default();
    let mut form = Form::new("Review").field(
        Field::list(SONGS, "Held songs")
            .options(rows(settings, &state.chosen))
            .when_empty("Nothing is waiting for review"),
    );
    label_start(&mut form, &state.chosen);
    (form, state)
}

fn rows(settings: &Settings, chosen: &BTreeMap<String, Resolution>) -> Vec<Choice> {
    settings
        .review
        .songs()
        .iter()
        .map(|held| {
            let key = held.song.key();
            let detail = chosen
                .get(key)
                .and_then(|resolution| choice_words(held, resolution))
                .unwrap_or_else(|| held.from.join(", "));
            Choice::from_parts(
                key.to_owned(),
                vec![
                    Part::plain(held.song.title.clone()),
                    Part::plain(held.song.artists_joined()),
                ],
            )
            .with_detail(detail)
        })
        .collect()
}

/// The choice in words for a row; `None` for `Leave for later`.
fn choice_words(held: &HeldSong, resolution: &Resolution) -> Option<String> {
    match resolution {
        Resolution::Candidate(id) => {
            let candidate = held.candidates.iter().find(|c| &c.video_id == id)?;
            Some(format!(
                "→ {} · {}",
                candidate.title,
                candidate.artists.join(", ")
            ))
        }
        Resolution::OwnAudio => Some("→ link's own audio".to_owned()),
        Resolution::PastedLink(_) => Some("→ pasted link".to_owned()),
        Resolution::Never => Some("→ never".to_owned()),
        Resolution::Later => None,
    }
}

fn decided(chosen: &BTreeMap<String, Resolution>) -> Vec<ReviewChoice> {
    chosen
        .iter()
        .filter(|(_, resolution)| **resolution != Resolution::Later)
        .map(|(key, resolution)| ReviewChoice {
            key: key.clone(),
            resolution: resolution.clone(),
        })
        .collect()
}

/// The list's button and what blocks it.
fn label_start(form: &mut Form, chosen: &BTreeMap<String, Resolution>) {
    let count = decided(chosen).len();
    form.set_primary(format!("Download {} chosen", thousands(count)));
    form.set_blocked((count == 0).then(|| "Choose what to do with at least one song".to_owned()));
}

/// The list form, whichever of list and detail is on screen.
fn list_of<'a>(form: &'a mut Form, state: &'a mut State) -> &'a mut Form {
    state.list.as_mut().unwrap_or(form)
}

/// Rebuild the rows from the runner's review list, and the button from the choices.
fn rebuild(form: &mut Form, state: &mut State, settings: &Settings) {
    let held: Vec<&str> = settings
        .review
        .songs()
        .iter()
        .map(|held| held.song.key())
        .collect();
    state.chosen.retain(|key, _| held.contains(&key.as_str()));
    let options = rows(settings, &state.chosen);
    let chosen = state.chosen.clone();
    let list = list_of(form, state);
    list.set_options(SONGS, options);
    label_start(list, &chosen);
}

/// `HeldCount` rebuilds the rows; a song that left while its detail is shown sends the detail
/// back to the list.
pub(super) fn on_event(form: &mut Form, state: &mut State, kind: &EventKind, settings: &Settings) {
    if !matches!(kind, EventKind::HeldCount(_)) {
        return;
    }
    rebuild(form, state, settings);
    let gone = state
        .detail
        .as_deref()
        .is_some_and(|key| find(settings, key).is_none());
    if gone {
        back(form, state);
    }
}

fn find<'a>(settings: &'a Settings, key: &str) -> Option<&'a HeldSong> {
    settings
        .review
        .songs()
        .iter()
        .find(|held| held.song.key() == key)
}

/// Enter or a click on a row: show that song's detail.
pub(super) fn open_detail(form: &mut Form, state: &mut State, key: &str, settings: &Settings) {
    if state.list.is_some() {
        return;
    }
    let Some(held) = find(settings, key) else {
        return;
    };
    let detail = detail_form(held, state.chosen.get(key));
    state.list = Some(std::mem::replace(form, detail));
    state.detail = Some(key.to_owned());
}

/// Return to the list, keeping what was chosen before. `false` when the list is already shown.
pub(super) fn back(form: &mut Form, state: &mut State) -> bool {
    let Some(list) = state.list.take() else {
        return false;
    };
    *form = list;
    state.detail = None;
    true
}

/// `Done` in the detail: record the choice, return to the list.
pub(super) fn done(form: &mut Form, state: &mut State, settings: &Settings) {
    let resolution = match (chosen_of(form, CHOICE), state.detail.clone()) {
        (Some(id), Some(key)) => Some((key, resolution_of(form, &id))),
        _ => None,
    };
    back(form, state);
    if let Some((key, resolution)) = resolution {
        state.chosen.insert(key, resolution);
    }
    rebuild(form, state, settings);
}

fn resolution_of(form: &Form, id: &str) -> Resolution {
    match id {
        OWN_AUDIO => Resolution::OwnAudio,
        PASTE => Resolution::PastedLink(text_of(form, PASTED)),
        LATER => Resolution::Later,
        NEVER => Resolution::Never,
        video_id => Resolution::Candidate(video_id.to_owned()),
    }
}

/// The detail's choice changed: the pasted-link field shows only for `Paste a link`.
pub(super) fn changed(form: &mut Form, field: &str) {
    if field == CHOICE {
        let paste = chosen_of(form, CHOICE).as_deref() == Some(PASTE);
        form.set_visible(PASTED, paste);
    }
}

/// The candidate chosen in the detail, to open in the browser.
pub(super) fn chosen_candidate<'a>(
    form: &Form,
    state: &State,
    settings: &'a Settings,
) -> Option<&'a Candidate> {
    let id = chosen_of(form, CHOICE).filter(|id| is_candidate(id))?;
    let held = find(settings, state.detail.as_deref()?)?;
    held.candidates
        .iter()
        .find(|candidate| candidate.video_id == id)
}

/// The choices of the decided songs.
pub(super) fn answers(state: &State) -> Answers {
    Answers::Review {
        choices: decided(&state.chosen),
    }
}

fn detail_form(held: &HeldSong, previous: Option<&Resolution>) -> Form {
    let song = &held.song;
    let mut options: Vec<Choice> = held
        .candidates
        .iter()
        .map(|candidate| {
            Choice::from_parts(candidate.video_id.clone(), candidate_parts(held, candidate))
        })
        .collect();
    if song.is_from_link() {
        options.push(Choice::new(OWN_AUDIO, "Use the link's own audio"));
    }
    options.push(Choice::new(PASTE, "Paste a link"));
    options.push(Choice::new(LATER, "Leave for later"));
    options.push(Choice::new(NEVER, "Never download"));
    let mut form = Form::new(song.title.clone())
        .field(
            Field::radio(CHOICE, "What to do")
                .options(options)
                .needs("Choose what to do with this song"),
        )
        .field(
            Field::text(PASTED, "Link")
                .placeholder("Paste a YouTube, YouTube Music or SoundCloud link")
                .validate(link::check)
                .needs("Paste a link")
                .hidden(),
        );
    let mut wanted = vec![format!(
        "Wanted: {}",
        label(&song.title, &song.artists_joined())
    )];
    let album = song.album.clone().unwrap_or_default();
    let length = song.duration.map(clock).unwrap_or_default();
    wanted.push(label(&album, &length));
    wanted.push(held.reason.clone());
    wanted.retain(|line| !line.is_empty());
    form.set_notes(wanted);
    form.set_primary("Done");
    form.set_back(true);
    match previous {
        Some(Resolution::Candidate(id)) => form.set(CHOICE, Value::Choice(Some(id.clone()))),
        Some(Resolution::OwnAudio) => form.set(CHOICE, Value::Choice(Some(OWN_AUDIO.to_owned()))),
        Some(Resolution::PastedLink(url)) => {
            form.set(CHOICE, Value::Choice(Some(PASTE.to_owned())));
            form.set(PASTED, Value::Text(url.clone()));
        }
        Some(Resolution::Later) => form.set(CHOICE, Value::Choice(Some(LATER.to_owned()))),
        Some(Resolution::Never) => form.set(CHOICE, Value::Choice(Some(NEVER.to_owned()))),
        None => {}
    }
    changed(&mut form, CHOICE);
    form
}

/// `a · b`, leaving out an empty side.
fn label(first: &str, second: &str) -> String {
    [first, second]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `3:41`.
fn clock(length: Duration) -> String {
    let secs = length.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A candidate's title, artists, album and length, each flagged when it differs from the wanted
/// song's.
fn candidate_parts(held: &HeldSong, candidate: &Candidate) -> Vec<Part> {
    let song = &held.song;
    let same = |a: &str, b: &str| a.trim().to_lowercase() == b.trim().to_lowercase();
    let part = |text: String, same: bool| {
        if same {
            Part::plain(text)
        } else {
            Part::flagged(text)
        }
    };
    let artists_same = song.artists.len() == candidate.artists.len()
        && song
            .artists
            .iter()
            .all(|a| candidate.artists.iter().any(|b| same(a, b)));
    let artists = if candidate.artists.is_empty() {
        "no artist".to_owned()
    } else {
        candidate.artists.join(", ")
    };
    let mut parts = vec![
        part(candidate.title.clone(), same(&song.title, &candidate.title)),
        part(artists, artists_same),
    ];
    if let Some(album) = &candidate.album {
        let matches = song
            .album
            .as_deref()
            .is_none_or(|wanted| same(wanted, album));
        parts.push(part(album.clone(), matches));
    } else if song.album.is_some() {
        parts.push(Part::flagged("no album"));
    }
    match (candidate.duration, song.duration) {
        (Some(found), Some(wanted)) => {
            let apart = found.abs_diff(wanted);
            parts.push(part(clock(found), apart <= LENGTH_SLACK));
        }
        (Some(found), None) => parts.push(Part::plain(clock(found))),
        (None, _) => {}
    }
    parts
}
