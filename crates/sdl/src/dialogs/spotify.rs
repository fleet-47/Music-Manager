//! Sync Spotify (F4): a tick list of Liked Songs and the owner's playlists, then `N new of M ·
//! K waiting in Review`, then Start.

use sdl_core::{EventKind, ListChoice};
use sdl_form::{Choice, Field, Form, Value};

use super::Step;
use crate::app::Settings;
use crate::fmt::thousands;

/// The tick list of Spotify lists. Values are list ids.
pub const LISTS: &str = "lists";

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub struct State {
    /// The lists the task offered, to turn the ticked ids back into `ListChoice`s for the answer.
    pub lists: Vec<ListChoice>,
}

/// The form: busy ("Signing in to Spotify") until `Lists` arrives; ticks remembered from
/// `state.ticked_lists`. Primary button `Continue`; after the summary shows, `Start`.
#[must_use]
pub fn open(settings: &Settings) -> (Form, State) {
    let mut form = Form::new("Sync Spotify").field(
        Field::ticks(LISTS, "Spotify lists")
            .needs("Tick at least one list")
            .when_empty("Your lists will show here"),
    );
    form.set(LISTS, Value::Ticked(settings.state.ticked_lists.clone()));
    form.set_busy(Some("Signing in to Spotify".to_owned()));
    (form, State::default())
}

/// `Lists` fills the tick list (`name`, detail `1,234 songs`); `Headline` is the busy line while
/// the form is busy; `NewOfTotal` becomes the summary `412 new of 2,117 · 3 waiting in Review`
/// and the button `Start` (a run with 0 new may start: it still rewrites the ticked lists'
/// playlist files); `Started` closes. The sign-in events are the app's (`Confirm::Browser`,
/// `Waiting::Browser`); the form stays busy (`Signing in to Spotify`) under them.
pub fn on_event(form: &mut Form, state: &mut State, kind: &EventKind) -> Step {
    match kind {
        EventKind::Lists(lists) => {
            state.lists.clone_from(lists);
            form.set_options(
                LISTS,
                lists
                    .iter()
                    .map(|list| {
                        Choice::new(list.id.clone(), list.name.clone())
                            .with_detail(format!("{} songs", thousands(list.count)))
                    })
                    .collect(),
            );
            form.set_busy(None);
        }
        EventKind::Headline(text) if form.is_animating() => form.set_busy(Some(text.clone())),
        _ => {}
    }
    Step::Stay(Vec::new())
}
