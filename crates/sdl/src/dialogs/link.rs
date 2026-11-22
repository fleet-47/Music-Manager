//! Download link (F5): one or more links, then Start. There is no plan, so the one answer starts
//! the run.

use sdl_core::{EventKind, TaskId};
use sdl_form::{Field, Form};
use sdl_tasks::Answers;

use super::choices::text_of;
use super::libraries::{self, Known};
use crate::app::Settings;

/// The text field. Its value is the links, separated by spaces or commas.
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

/// The links in `text`: what stands between spaces, line breaks and commas, each once, in the
/// order given (F5).
pub(super) fn links(text: &str) -> Vec<&str> {
    let mut found: Vec<&str> = Vec::new();
    for part in text.split(|c: char| c.is_whitespace() || c == ',') {
        if !part.is_empty() && !found.contains(&part) {
            found.push(part);
        }
    }
    found
}

/// Whether every link in `text` is one this function takes. With several links the message
/// names the one that is not.
fn check_all(text: &str) -> Result<(), String> {
    let found = links(text);
    let Some(first) = found.first() else {
        return Err("Paste a link".to_owned());
    };
    if found.len() == 1 {
        return check(first);
    }
    for (at, link) in found.iter().enumerate() {
        check(link).map_err(|why| format!("Link {} ({link}): {why}", at.saturating_add(1)))?;
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

/// The form: whose library, then one text field that needs at least one link, primary button
/// `Start`.
pub(super) fn open(settings: &Settings) -> (Form, State) {
    let mut form = libraries::recipient_fields(Form::new("Download link")).field(
        Field::text(LINK, "Link")
            .placeholder("Paste one or more links: YouTube, YouTube Music, SoundCloud")
            .validate(check_all)
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

/// Whose library, and the pasted links.
pub(super) fn answers(form: &Form, state: &State) -> Option<Answers> {
    Some(Answers::DownloadLink {
        urls: links(&text_of(form, LINK))
            .into_iter()
            .map(str::to_owned)
            .collect(),
        recipient: libraries::recipient(form, &state.known)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{check, check_all, links};

    #[test]
    fn spaces_line_breaks_and_commas_separate_links_and_a_repeat_is_taken_once() {
        assert_eq!(
            links(" https://a.b/1,https://a.b/2 ,\n https://a.b/1\thttps://a.b/3 "),
            ["https://a.b/1", "https://a.b/2", "https://a.b/3"]
        );
        assert_eq!(links(" , "), [] as [&str; 0]);
    }

    #[test]
    fn with_several_links_the_message_names_the_one_that_is_wrong() {
        assert!(check_all("https://a.b/1 https://a.b/2").is_ok());
        assert!(
            check_all("https://a.b/1, https://open.spotify.com/track/x").is_err_and(|why| why
                .starts_with("Link 2 (https://open.spotify.com/track/x)")
                && why.contains("Sync Spotify"))
        );
        // One link keeps the short message.
        assert!(check_all("a.b/1").is_err_and(|why| why.starts_with("That does not look")));
        assert!(check_all(" , ").is_err());
    }

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
