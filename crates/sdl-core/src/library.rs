//! Libraries (F12): a library is a named folder directly under `music_root`, holding one
//! person's songs in the F7 layout and that person's playlist files.
//!
//! The rules live here and nowhere else: which folders are libraries, what a new library may be
//! called, which library is the owner's own, and what the settings files record to say "which
//! library" without knowing the config ([`LibraryId`]).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Config;
use crate::place::component;

/// Which library a remembered item belongs to, as the settings files record it: the owner's own
/// (`my_library` in the config, written as nothing at all, so files that predate libraries mean
/// the owner's own), or another library by name.
///
/// Stored as a string: `""` for the owner's own, otherwise the folder name.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum LibraryId {
    /// The owner's own library.
    #[default]
    Own,
    /// Another library, by folder name.
    Other(String),
}

impl LibraryId {
    /// Whether this is the owner's own library (what `skip_serializing_if` asks).
    #[must_use]
    pub const fn is_own(&self) -> bool {
        matches!(self, Self::Own)
    }
}

impl From<String> for LibraryId {
    fn from(name: String) -> Self {
        if name.is_empty() {
            Self::Own
        } else {
            Self::Other(name)
        }
    }
}

impl From<LibraryId> for String {
    fn from(id: LibraryId) -> Self {
        match id {
            LibraryId::Own => Self::new(),
            LibraryId::Other(name) => name,
        }
    }
}

/// Why a new library cannot be made. Messages are plain words fit for a message under the form's
/// name field.
#[derive(Debug, thiserror::Error)]
pub enum NewLibraryError {
    /// Nothing usable was typed (empty, or only characters a folder name cannot hold).
    #[error("Type a name for the library.")]
    Empty,
    /// A library with this name exists already (names are compared without regard to case,
    /// because the Mac and FAT drives do not tell `Sam` and `sam` apart).
    #[error("A library named {0} already exists. Choose another name.")]
    Exists(String),
    /// The folder could not be made.
    #[error("The library folder could not be made: {0}")]
    Io(#[from] std::io::Error),
}

/// One library: its name and its folder `<music_root>/<name>`.
///
/// Invariant: `root` is directly under the music root, and its name is a single path component
/// that does not start with a dot. The folder itself may not exist yet (the owner's own library
/// before its first song).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    name: String,
    root: PathBuf,
    own: bool,
}

impl Library {
    /// A library by its parts. Prefer [`Library::mine`], [`Library::list`] and
    /// [`Library::resolve`]; this is the raw constructor they use, and what a test uses to point a
    /// library at a temp folder.
    #[must_use]
    pub fn at(name: &str, root: &Path, own: bool) -> Self {
        Self {
            name: name.to_owned(),
            root: root.to_path_buf(),
            own,
        }
    }

    /// The owner's own library: `<music_root>/<my_library>`.
    #[must_use]
    pub fn mine(config: &Config) -> Self {
        Self::at(
            &config.my_library,
            &config.music_root.join(&config.my_library),
            true,
        )
    }

    /// The library `id` names. A name equal to the owner's own (without regard to case) is the
    /// owner's own.
    #[must_use]
    pub fn resolve(config: &Config, id: &LibraryId) -> Self {
        match id {
            LibraryId::Other(name) if !same_name(name, &config.my_library) => {
                Self::at(name, &config.music_root.join(name), false)
            }
            _ => Self::mine(config),
        }
    }

    /// The libraries on disk: the folders directly under `music_root` whose names do not start
    /// with a dot, sorted by name, with the owner's own always first and always present (even
    /// before its folder exists). A missing `music_root` has only the owner's own.
    ///
    /// # Errors
    /// `music_root` exists but cannot be read.
    pub fn list(config: &Config) -> std::io::Result<Vec<Self>> {
        let mut others = Vec::new();
        match std::fs::read_dir(&config.music_root) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let file_name = entry.file_name();
                    let Some(name) = file_name.to_str() else {
                        continue;
                    };
                    if !name.starts_with('.')
                        && !same_name(name, &config.my_library)
                        && entry.path().is_dir()
                    {
                        others.push(Self::at(name, &entry.path(), false));
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        others.sort_by(|a, b| a.name.cmp(&b.name));
        let mut all = vec![Self::mine(config)];
        all.extend(others);
        Ok(all)
    }

    /// The name a new library would have: `typed` made FAT-safe like a path component (F7).
    /// `existing` is the list the form was offered ([`Library::list`]). Pure, so a form can check
    /// as the owner types.
    ///
    /// # Errors
    /// [`NewLibraryError::Empty`] or [`NewLibraryError::Exists`].
    pub fn check_new_name(existing: &[Self], typed: &str) -> Result<String, NewLibraryError> {
        let name = component(typed);
        if name.is_empty() {
            return Err(NewLibraryError::Empty);
        }
        match existing
            .iter()
            .find(|library| same_name(&library.name, &name))
        {
            Some(found) => Err(NewLibraryError::Exists(found.name.clone())),
            None => Ok(name),
        }
    }

    /// Make a new library: [`Library::check_new_name`] against what is on disk now, then create
    /// its folder (and `music_root` when missing).
    ///
    /// # Errors
    /// The name is refused, or the folder could not be made.
    pub fn create(config: &Config, typed: &str) -> Result<Self, NewLibraryError> {
        let name = Self::check_new_name(&Self::list(config)?, typed)?;
        std::fs::create_dir_all(&config.music_root)?;
        let root = config.music_root.join(&name);
        // Not `create_dir_all`: this fails when the folder appeared meanwhile.
        match std::fs::create_dir(&root) {
            Ok(()) => Ok(Self::at(&name, &root, false)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                Err(NewLibraryError::Exists(name))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The library's name, which is also its folder's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The library's folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The music root this library lives under: its folder's parent. (Convert refuses a
    /// destination inside it, F9.)
    #[must_use]
    pub fn music_root(&self) -> &Path {
        self.root.parent().unwrap_or(&self.root)
    }

    /// Whether this is the owner's own library.
    #[must_use]
    pub const fn is_own(&self) -> bool {
        self.own
    }

    /// How the settings files record this library.
    #[must_use]
    pub fn id(&self) -> LibraryId {
        if self.own {
            LibraryId::Own
        } else {
            LibraryId::Other(self.name.clone())
        }
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn config(root: &Path) -> Config {
        Config {
            client_id: "id".into(),
            music_root: root.to_path_buf(),
            my_library: "Me".into(),
            download_jobs: std::num::NonZeroUsize::MIN,
            animations: true,
        }
    }

    #[test]
    fn the_owners_own_library_is_listed_first_even_with_no_folder() -> TestResult {
        let dir = tempfile::tempdir()?;
        let libraries = Library::list(&config(&dir.path().join("missing")))?;
        assert_eq!(libraries.len(), 1);
        assert!(libraries.iter().all(Library::is_own));
        assert_eq!(libraries.first().map(Library::name), Some("Me"));
        Ok(())
    }

    #[test]
    fn libraries_are_the_folders_under_the_music_root_without_dot_names() -> TestResult {
        let dir = tempfile::tempdir()?;
        for folder in ["Sam", "Alex", ".sdl-tmp", "Me"] {
            std::fs::create_dir(dir.path().join(folder))?;
        }
        std::fs::write(dir.path().join("Liked Songs.m3u8"), b"x")?;
        let names: Vec<(String, bool)> = Library::list(&config(dir.path()))?
            .iter()
            .map(|library| (library.name().to_owned(), library.is_own()))
            .collect();
        assert_eq!(
            names,
            [("Me", true), ("Alex", false), ("Sam", false)].map(|(n, o)| (n.to_owned(), o))
        );
        Ok(())
    }

    #[test]
    fn a_new_library_is_made_safe_and_an_existing_name_is_refused() -> TestResult {
        let dir = tempfile::tempdir()?;
        let cfg = config(dir.path());
        let made = Library::create(&cfg, "S:am?")?;
        assert_eq!(made.name(), "Sam");
        assert!(made.root().is_dir() && !made.is_own());
        assert_eq!(made.root(), dir.path().join("Sam"));
        for again in ["Sam", "sam", "Me", "me"] {
            let refused = Library::create(&cfg, again);
            assert!(
                matches!(refused, Err(NewLibraryError::Exists(_))),
                "{again}: {refused:?}"
            );
        }
        assert!(matches!(
            Library::create(&cfg, "  ..?? "),
            Err(NewLibraryError::Empty)
        ));
        assert!(matches!(
            Library::create(&cfg, ""),
            Err(NewLibraryError::Empty)
        ));
        let message = Library::create(&cfg, "Sam").err().map(|e| e.to_string());
        assert_eq!(
            message.as_deref(),
            Some("A library named Sam already exists. Choose another name.")
        );
        Ok(())
    }

    #[test]
    fn a_name_cannot_climb_out_of_the_music_root() -> TestResult {
        let dir = tempfile::tempdir()?;
        let made = Library::create(&config(dir.path()), "../../evil")?;
        assert_eq!(made.root().parent(), Some(dir.path()));
        assert_eq!(made.name(), "evil");
        Ok(())
    }

    #[test]
    fn ids_resolve_back_to_the_same_library() -> TestResult {
        let dir = tempfile::tempdir()?;
        let cfg = config(dir.path());
        let mine = Library::mine(&cfg);
        assert_eq!(mine.id(), LibraryId::Own);
        assert_eq!(Library::resolve(&cfg, &LibraryId::Own), mine);
        let sam = Library::create(&cfg, "Sam")?;
        assert_eq!(sam.id(), LibraryId::Other("Sam".into()));
        assert_eq!(Library::resolve(&cfg, &sam.id()), sam);
        // An id that names the owner's own folder is the owner's own.
        assert_eq!(Library::resolve(&cfg, &LibraryId::Other("me".into())), mine);
        Ok(())
    }

    #[test]
    fn an_id_is_written_as_a_string_and_own_as_empty() -> TestResult {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            #[serde(default, skip_serializing_if = "LibraryId::is_own")]
            library: LibraryId,
        }
        let own = toml::to_string(&Holder {
            library: LibraryId::Own,
        })?;
        assert_eq!(own, "");
        assert_eq!(toml::from_str::<Holder>(&own)?.library, LibraryId::Own);
        let sam = toml::to_string(&Holder {
            library: LibraryId::Other("Sam".into()),
        })?;
        assert_eq!(sam, "library = \"Sam\"\n");
        assert_eq!(
            toml::from_str::<Holder>(&sam)?.library,
            LibraryId::Other("Sam".into())
        );
        Ok(())
    }
}
