//! Sync Spotify (F4): a tick list of Liked Songs and the owner's playlists, then `N new of M ·
//! K waiting in Review`, then Start. Listing songs costs Spotify calls, so a plan is asked for
//! only on Continue.

use sdl_core::{EventKind, ListChoice, SignInState};
use sdl_form::{Choice, Field, Form, Value};
use sdl_tasks::Answers;

use super::choices::ticked_of;
use super::conversation::Conversation;
use crate::app::Settings;
use crate::fmt::thousands;

/// The tick list of Spotify lists. Values are list ids.
pub(super) const LISTS: &str = "lists";

const SIGNING_IN: &str = "Signing in to Spotify";

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The lists the task offered, to turn the ticked ids back into `ListChoice`s for the answer.
    lists: Vec<ListChoice>,
}

/// The form, ticks remembered from `state.ticked_lists`, waiting for the lists.
pub(super) fn open(settings: &Settings) -> (Form, State, Conversation) {
    let mut form = Form::new("Sync Spotify").field(
        Field::ticks(LISTS, "Spotify lists")
            .needs("Tick at least one list")
            .when_empty("Your lists will show here"),
    );
    form.set(LISTS, Value::Ticked(settings.state.ticked_lists.clone()));
    let conv = Conversation::planned("Listing your songs", Some(SIGNING_IN));
    (form, State::default(), conv)
}

/// `Lists` fills the tick list (`name`, detail `1,234 songs`); `NewOfTotal` is the summary. The
/// sign-in popups are the app's; the form keeps saying it is signing in under them.
pub(super) fn on_event(
    form: &mut Form,
    state: &mut State,
    conv: &mut Conversation,
    kind: &EventKind,
) {
    match kind {
        EventKind::SignIn(SignInState::SignedIn) => conv.wait_for(Some("Reading your lists")),
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
            conv.wait_for(None);
        }
        EventKind::NewOfTotal {
            new,
            total,
            waiting,
        } => {
            conv.plan(format!(
                "{} new of {} · {} waiting in Review",
                thousands(*new),
                thousands(*total),
                thousands(*waiting)
            ));
        }
        EventKind::Headline(text) => conv.headline(text),
        _ => {}
    }
}

/// The ticked lists, in the order they were offered.
pub(super) fn answers(form: &Form, state: &State) -> Option<Answers> {
    let ticked = ticked_of(form, LISTS);
    let lists: Vec<ListChoice> = state
        .lists
        .iter()
        .filter(|list| ticked.contains(&list.id))
        .cloned()
        .collect();
    (!lists.is_empty()).then_some(Answers::SyncSpotify { lists })
}
