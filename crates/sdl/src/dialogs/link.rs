//! Download link (F5): the link, then Start. There is no plan, so the one answer starts the run.

use sdl_form::{Field, Form};
use sdl_tasks::Answers;

use super::choices::text_of;

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

/// The form: one text field that needs a link, primary button `Start`.
pub(super) fn open() -> Form {
    let mut form = Form::new("Download link").field(
        Field::text(LINK, "Link")
            .placeholder("Paste a YouTube, YouTube Music or SoundCloud link")
            .validate(check)
            .needs("Paste a link"),
    );
    form.set_primary("Start");
    form
}

/// The pasted link.
pub(super) fn answers(form: &Form) -> Answers {
    Answers::DownloadLink {
        url: text_of(form, LINK),
    }
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
