//! Reading and writing the two settings files (`state.toml`, `review.toml`) as TOML.
//!
//! One implementation of "a TOML file the app owns", used by [`crate::State`] and
//! [`crate::ReviewList`]. Every error names the file's path, so the message can say which file.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Errors loading or saving a settings file. Every variant (except `NoHome`) names the path.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No home directory could be determined.
    #[error("cannot determine the home directory to locate the settings files")]
    NoHome,
    /// The file exists but cannot be read.
    #[error("cannot read {}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The file is not valid TOML of the expected shape.
    #[error("{} is malformed", path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// Underlying error.
        source: toml::de::Error,
    },
    /// The file could not be written.
    #[error("cannot write {}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The value could not be turned into TOML.
    #[error("cannot encode {}", path.display())]
    Encode {
        /// The file.
        path: PathBuf,
        /// Underlying error.
        source: toml::ser::Error,
    },
}

impl From<crate::ConfigError> for StoreError {
    fn from(_: crate::ConfigError) -> Self {
        Self::NoHome
    }
}

/// Parse `text` as `T`; `path` only names the file in errors.
pub(crate) fn parse<T: DeserializeOwned>(text: &str, path: &Path) -> Result<T, StoreError> {
    toml::from_str(text).map_err(|source| StoreError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Read `path` as `T`. A missing file is `T::default()` (first run).
pub(crate) fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StoreError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(StoreError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Write `value` to `path` as TOML, creating its directory if needed.
pub(crate) fn save<T: Serialize>(value: &T, path: &Path) -> Result<(), StoreError> {
    let text = toml::to_string_pretty(value).map_err(|source| StoreError::Encode {
        path: path.to_path_buf(),
        source,
    })?;
    let write = |source| StoreError::Write {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(write)?;
    }
    std::fs::write(path, text).map_err(write)
}

/// A `Option<Duration>` stored as whole seconds (TOML has no duration type). Sub-second parts are
/// dropped, which is harmless: matching tolerates 3 s.
pub(crate) mod opt_secs {
    use std::borrow::Borrow;
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer, T: Borrow<Option<Duration>>>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value.borrow() {
            Some(duration) => serializer.serialize_u64(duration.as_secs()),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(deserializer)?.map(Duration::from_secs))
    }
}
