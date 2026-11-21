//! Download link (F5): the link, then Start. There is no plan, so the one answer starts the run.

use sdl_core::{EventKind, TaskId};
use sdl_form::{Field, Form};
use sdl_tasks::Answers;

use super::choices::text_of;
use super::libraries::{self, Known};
use crate::app::Settings;

/// The text field. Its value is the link.
pub(super) const LINK: &str = "link";

/// Whether `text` is a link this function takes, in the owner's words when it is not. A Spotify
/// link is refused (F5: Spotify lists are F4). Review's pasted link uses the same check.
pub(super) fn check(text: &str) -> Result<(), String> {
    let text = text.trim();
    let Some(rest) = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
    else {
        return Err("That does not look like a link. It should start with https://".to_owned());
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or(host).to_ascii_lowercase();
    if host == "spotify.com" || host.ends_with(".spotify.com") || host == "spotify.link" {
        return Err(
            "That is a Spotify link. Spotify songs come in with Sync Spotify; paste a YouTube or SoundCloud link here."
                .to_owned(),
        );
    }
    Ok(())
}

/// What this dialog keeps besides its form.
#[derive(Debug, Default)]
pub(super) struct State {
    known: Known,
    /// The other library last used, preselected when the Library dropdown is shown.
    last: Option<String>,
}

/// The form: whose library, then one text field that needs a link, primary button `Start`.
pub(super) fn open(settings: &Settings) -> (Form, State) {
    let mut form = libraries::recipient_fields(Form::new("Download link")).field(
        Field::text(LINK, "Link")
            .placeholder("Paste a YouTube, YouTube Music or SoundCloud link")
            .validate(check)
            .needs("Paste a link"),
    );
    form.set_primary("Start");
    let state = State {
        known: Known::default(),
        last: settings
            .state
            .last_friend
            .get(&TaskId::DownloadLink)
            .cloned(),
    };
    (form, state)
}

/// The list of libraries arrives: the Library dropdown is filled.
pub(super) fn on_event(form: &mut Form, state: &mut State, kind: &EventKind) {
    if let EventKind::Libraries(found) = kind {
        libraries::offer_others(form, &mut state.known, found, state.last.as_deref());
    }
}

/// A field changed: the library fields follow the toggle and the dropdown.
pub(super) fn changed(form: &mut Form, state: &State) {
    libraries::follow(form, &state.known);
}

/// Whose library, and the pasted link.
pub(super) fn answers(form: &Form, state: &State) -> Option<Answers> {
    Some(Answers::DownloadLink {
        url: text_of(form, LINK),
        recipient: libraries::recipient(form, &state.known)?,
    })
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn a_spotify_link_is_refused_in_plain_words() {
        assert!(check(" https://music.youtube.com/watch?v=x ").is_ok());
        assert!(check("https://soundcloud.com/a/b").is_ok());
        assert!(
            check("https://open.spotify.com/track/x")
                .is_err_and(|why| why.contains("Sync Spotify"))
        );
        assert!(check("https://spotify.link/abc").is_err_and(|why| why.contains("Sync Spotify")));
        assert!(check("https://notspotify.com/x").is_ok());
        assert!(check("open.spotify.com/x").is_err_and(|why| why.contains("https://")));
    }
}
