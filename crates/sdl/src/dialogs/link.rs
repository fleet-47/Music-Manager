//! Download link (F5): the link, then Start. There is no plan, so the one answer starts the run.

use sdl_form::{Field, Form};
use sdl_tasks::Answers;

use super::choices::text_of;

/// The text field. Its value is the link.
pub(super) const LINK: &str = "link";

/// Whether `text` looks like a link, in the owner's words when it does not. Review's pasted link
/// uses the same check.
pub(super) fn check(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.starts_with("http://") || text.starts_with("https://") {
        Ok(())
    } else {
        Err("That does not look like a link. It should start with https://".to_owned())
    }
}

/// The form: one text field that needs a link, primary button `Start`.
pub(super) fn open() -> Form {
    let mut form = Form::new("Download link").field(
        Field::text(LINK, "Link")
            .placeholder("Paste a YouTube or YouTube Music link")
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
