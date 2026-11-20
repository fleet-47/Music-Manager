//! Download link (F5): the link, then Start.

use sdl_form::{Field, Form};

/// The text field. Its value is the link.
pub const LINK: &str = "link";

/// The form: one text field that needs a link, primary button `Start`. A value that does not
/// start with `http` is "not acceptable" in plain words.
#[must_use]
pub fn open() -> Form {
    fn check(text: &str) -> Result<(), String> {
        if text.starts_with("http://") || text.starts_with("https://") {
            Ok(())
        } else {
            Err("That does not look like a link. It should start with https://".to_owned())
        }
    }
    let mut form = Form::new("Download link").field(
        Field::text(LINK, "Link")
            .placeholder("Paste a YouTube or YouTube Music link")
            .validate(check)
            .needs("Paste a link"),
    );
    form.set_primary("Start");
    form
}
