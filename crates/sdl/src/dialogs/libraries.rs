//! The library fields of F12, shared by the four forms that choose one.
//!
//! * Sync Spotify and Download link ask **For me**, and with it off a **Library** dropdown (the
//!   other libraries, then **New library…**) and, for a new one, its name ([`recipient_fields`]).
//! * Sync device and Convert start with a **Library** dropdown holding every library, the
//!   owner's own first ([`picker`]).
//!
//! The libraries come from the task (`EventKind::Libraries`); [`Known`] keeps the last list.

use sdl_core::{Library, LibraryId};
use sdl_form::{Choice, Field, Form, Value};
use sdl_tasks::{Offer, Recipient};

use super::choices::{chosen_of, flag_of, text_of};

/// Toggle (download forms): the songs are for the owner.
pub(super) const FOR_ME: &str = "for_me";
/// Dropdown: the library. Download forms show it only with For me off.
pub(super) const LIBRARY: &str = "library";
/// Text (download forms): the name of the library to make.
pub(super) const NAME: &str = "new_library";

/// Option id of the owner's own library, and of **New library…**. A folder name cannot hold a
/// slash, so neither can be a library's name.
const OWN: &str = "/own";
const NEW: &str = "/new";

/// The libraries on disk as the task last listed them, the owner's own first.
#[derive(Debug, Default)]
pub(super) struct Known {
    all: Option<Vec<Library>>,
}

impl Known {
    /// Whether the task has sent the list.
    pub(super) fn is_in(&self) -> bool {
        self.all.is_some()
    }

    fn libraries(&self) -> &[Library] {
        self.all.as_deref().unwrap_or_default()
    }

    /// The name of `library`; `None` before the list arrives.
    pub(super) fn name_of(&self, library: &LibraryId) -> Option<String> {
        match library {
            LibraryId::Other(name) => Some(name.clone()),
            LibraryId::Own => self
                .libraries()
                .iter()
                .find(|library| library.is_own())
                .map(|library| library.name().to_owned()),
        }
    }

    /// A remembered library that is gone from the disk gives way to the owner's own in a
    /// [`picker`]; before the list arrives nothing is known to be gone.
    pub(super) fn settle(&self, form: &mut Form) {
        let chosen = picked(form);
        if self.is_in()
            && !chosen.is_own()
            && !self.libraries().iter().any(|each| each.id() == chosen)
        {
            pick(form, &LibraryId::Own);
        }
    }
}

/// Which library's playlists the What list shows, so it follows the Library dropdown: the task
/// sends the owner's own at open, and the playlists of any other library only when asked
/// (`Offer::Playlists`), one `Playlists` event for each.
#[derive(Debug)]
pub(super) struct Follow {
    requested: LibraryId,
    pending: usize,
}

impl Default for Follow {
    fn default() -> Self {
        Self {
            requested: LibraryId::Own,
            pending: 1,
        }
    }
}

impl Follow {
    /// A `Playlists` event arrived.
    pub(super) fn arrived(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    /// Whether every `Playlists` asked for has arrived, so the last one is on show.
    pub(super) fn is_settled(&self) -> bool {
        self.pending == 0
    }

    /// The offer to make now: the playlists of the chosen library, when they are not the ones
    /// asked for last. Nothing until the libraries are known, so a remembered library that is
    /// gone is never asked for.
    pub(super) fn offer(&mut self, form: &Form, known: &Known) -> Option<Offer> {
        if !known.is_in() {
            return None;
        }
        let chosen = picked(form);
        if chosen == self.requested {
            return None;
        }
        self.requested.clone_from(&chosen);
        self.pending = self.pending.saturating_add(1);
        Some(Offer::Playlists(chosen))
    }
}

/// The Library dropdown that opens Sync device and Convert.
pub(super) fn picker() -> Field {
    Field::dropdown(LIBRARY, "Library").needs("Choose a library")
}

/// Show the remembered library in a form made with [`picker`], before the list arrives.
pub(super) fn pick(form: &mut Form, library: &LibraryId) {
    let id = match library {
        LibraryId::Own => OWN.to_owned(),
        LibraryId::Other(name) => name.clone(),
    };
    form.set(LIBRARY, Value::Choice(Some(id)));
}

/// The library chosen in a form made with [`picker`].
pub(super) fn picked(form: &Form) -> LibraryId {
    match chosen_of(form, LIBRARY).as_deref() {
        None | Some(OWN) => LibraryId::Own,
        Some(name) => LibraryId::Other(name.to_owned()),
    }
}

/// The list arrived for a [`picker`]: every library is offered, and a remembered library that is
/// gone from the disk gives way to the owner's own.
pub(super) fn offer_all(form: &mut Form, known: &mut Known, libraries: &[Library]) {
    known.all = Some(libraries.to_vec());
    form.set_options(
        LIBRARY,
        libraries
            .iter()
            .map(|library| {
                let id = if library.is_own() {
                    OWN
                } else {
                    library.name()
                };
                Choice::new(id, library.name())
            })
            .collect(),
    );
    // `set_options` drops a choice that is no longer offered.
    if chosen_of(form, LIBRARY).is_none() {
        pick(form, &LibraryId::Own);
    }
}

/// The For me toggle, the Library dropdown and the new library's name, in front of the form's own
/// fields. For me is on whenever the form opens; the other two show when it is turned off.
pub(super) fn recipient_fields(form: Form) -> Form {
    let mut form = form
        .field(Field::toggle(FOR_ME, "For me"))
        .field(
            Field::dropdown(LIBRARY, "Library")
                .needs("Choose a library")
                .hidden(),
        )
        .field(
            Field::text(NAME, "Library name")
                .placeholder("Name of the new library")
                .needs("Type a name for the library")
                .hidden(),
        );
    form.set(FOR_ME, Value::Flag(true));
    form
}

/// The list arrived for [`recipient_fields`]: the other libraries, then New library…, with the one
/// `last` used chosen when it is still there (with no other library, New library… is the only
/// thing to choose, so it is chosen).
pub(super) fn offer_others(
    form: &mut Form,
    known: &mut Known,
    libraries: &[Library],
    last: Option<&str>,
) {
    known.all = Some(libraries.to_vec());
    let mut options: Vec<Choice> = libraries
        .iter()
        .filter(|library| !library.is_own())
        .map(|library| Choice::new(library.name(), library.name()))
        .collect();
    let chosen = last
        .filter(|name| options.iter().any(|option| option.id == *name))
        .map(str::to_owned)
        .or_else(|| options.is_empty().then(|| NEW.to_owned()));
    options.push(Choice::new(NEW, "New library…"));
    form.set_options(LIBRARY, options);
    if chosen_of(form, LIBRARY).is_none() {
        form.set(LIBRARY, Value::Choice(chosen));
    }
    follow(form, known);
}

/// Show what the toggle and the dropdown call for, and say in plain words when the new name is
/// taken. The empty name is said by the field's own need.
pub(super) fn follow(form: &mut Form, known: &Known) {
    let for_me = flag_of(form, FOR_ME);
    let new = !for_me && chosen_of(form, LIBRARY).as_deref() == Some(NEW);
    form.set_visible(LIBRARY, !for_me);
    form.set_visible(NAME, new);
    let typed = text_of(form, NAME);
    let refusal = (new && !typed.is_empty())
        .then(|| Library::check_new_name(known.libraries(), &typed).err())
        .flatten()
        .map(|why| why.to_string());
    form.set_blocked(refusal);
}

/// Whose library the songs go into, as the form says now; `None` while the form cannot say (no
/// library chosen, or a new name that is refused).
pub(super) fn recipient(form: &Form, known: &Known) -> Option<Recipient> {
    if flag_of(form, FOR_ME) {
        return Some(Recipient::Me);
    }
    match chosen_of(form, LIBRARY)?.as_str() {
        NEW => {
            let typed = text_of(form, NAME);
            Library::check_new_name(known.libraries(), &typed)
                .ok()
                .map(|_| Recipient::New(typed))
        }
        name => Some(Recipient::Existing(name.to_owned())),
    }
}
