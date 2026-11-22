//! Sync Spotify (F4): a tick list of Liked Songs and the owner's playlists, then `N new of M ·
//! K waiting in Review`, then Start. Listing songs costs Spotify calls, so a plan is asked for
//! only on Continue.

use sdl_core::{EventKind, ListChoice, SignInState, TaskId};
use sdl_form::{Choice, Field, Form, Value};
use sdl_tasks::Answers;

use super::choices::ticked_of;
use super::conversation::Conversation;
use super::libraries::{self, Known};
use crate::app::Settings;
use sdl_core::words::thousands;

/// The tick list of Spotify lists. Values are list ids.
pub(super) const LISTS: &str = "lists";

const SIGNING_IN: &str = "Signing in to Spotify";

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The lists the task offered, to turn the ticked ids back into `ListChoice`s for the answer.
    lists: Vec<ListChoice>,
    known: Known,
    /// The other library last used, preselected when the Library dropdown is shown.
    last: Option<String>,
}

/// The form, ticks remembered from `state.ticked_lists`, waiting for the lists.
pub(super) fn open(settings: &Settings) -> (Form, State, Conversation) {
    let mut form = libraries::recipient_fields(Form::new("Sync Spotify")).field(
        Field::ticks(LISTS, "Spotify lists")
            .needs("Tick at least one list")
            .when_empty("Your lists will show here"),
    );
    form.set(LISTS, Value::Ticked(settings.state.ticked_lists.clone()));
    let conv = Conversation::planned("Listing your songs", Some(SIGNING_IN));
    let state = State {
        last: settings
            .state
            .last_friend
            .get(&TaskId::SyncSpotify)
            .cloned(),
        ..State::default()
    };
    (form, state, conv)
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
            unreadable,
        } => {
            conv.plan(summary(*new, *total, *waiting, *unreadable));
        }
        EventKind::Libraries(found) => {
            libraries::offer_others(form, &mut state.known, found, state.last.as_deref());
        }
        EventKind::Headline(text) => conv.headline(text),
        _ => {}
    }
}

/// `N new of M · K waiting in Review`, then `· U entries could not be read` when Spotify sent
/// entries that could not be read as songs.
fn summary(new: usize, total: usize, waiting: usize, unreadable: usize) -> String {
    let line = format!(
        "{} new of {} · {} waiting in Review",
        thousands(new),
        thousands(total),
        thousands(waiting)
    );
    match unreadable {
        0 => line,
        1 => format!("{line} · 1 entry could not be read"),
        many => format!("{line} · {} entries could not be read", thousands(many)),
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
    let recipient = libraries::recipient(form, &state.known)?;
    (!lists.is_empty()).then_some(Answers::SyncSpotify { lists, recipient })
}

/// A field changed: the library fields follow the toggle and the dropdown.
pub(super) fn changed(form: &mut Form, state: &State) {
    libraries::follow(form, &state.known);
}

#[cfg(test)]
mod tests {
    use super::summary;

    #[test]
    fn the_summary_says_how_many_entries_could_not_be_read() {
        assert_eq!(
            summary(412, 2117, 3, 0),
            "412 new of 2,117 · 3 waiting in Review"
        );
        assert_eq!(
            summary(1, 5, 0, 1),
            "1 new of 5 · 0 waiting in Review · 1 entry could not be read"
        );
        assert_eq!(
            summary(0, 5, 0, 1200),
            "0 new of 5 · 0 waiting in Review · 1,200 entries could not be read"
        );
    }
}
