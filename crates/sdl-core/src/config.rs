//! `~/.config/sdl/config.toml`.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::Deserialize;

use crate::place::component;

const DEFAULT_MY_LIBRARY: &str = "Me";

const DEFAULT_DOWNLOAD_JOBS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(jobs) => jobs,
    None => NonZeroUsize::MIN,
};

/// Errors loading the config. Every variant names the config path.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// No home directory could be determined.
    #[error("cannot determine the home directory to locate the config file")]
    NoHome,
    /// The file is missing or unreadable.
    #[error("cannot read config file {}", path.display())]
    Read {
        /// The config path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The file is not valid TOML or lacks a required key.
    #[error("malformed config file {}", path.display())]
    Parse {
        /// The config path.
        path: PathBuf,
        /// Underlying error.
        source: toml::de::Error,
    },
    /// `my_library` is not a usable folder name once made FAT-safe (F7).
    #[error("config file {}: my_library is not a usable folder name", path.display())]
    LibraryName {
        /// The config path.
        path: PathBuf,
    },
}

/// Settings from the config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Spotify developer app client ID.
    pub client_id: String,
    /// Music library root (default `~/Music`); a leading `~` is expanded.
    pub music_root: PathBuf,
    /// Name of the owner's own library, the folder `<music_root>/<my_library>` (default `Me`),
    /// made FAT-safe like a path component (F12).
    pub my_library: String,
    /// Parallel song downloads (default 4). Zero is a parse error.
    pub download_jobs: NonZeroUsize,
    /// Motion on (default true). `false` turns effects off.
    pub animations: bool,
}

/// Unknown keys (such as an old `card_uuid`) are ignored: serde does so unless told not to.
#[derive(Deserialize)]
struct RawConfig {
    client_id: String,
    music_root: Option<PathBuf>,
    my_library: Option<String>,
    download_jobs: Option<NonZeroUsize>,
    animations: Option<bool>,
}

/// Expand a leading `~` component of `path` to `home`. `~user` and a `~` elsewhere are left alone.
#[must_use]
pub(crate) fn expand_home(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// The directory that holds `config.toml` and `state.toml`, `~/.config/sdl`.
///
/// # Errors
/// [`ConfigError::NoHome`] when the home directory is unknown.
pub(crate) fn config_dir() -> Result<PathBuf, ConfigError> {
    let base = BaseDirs::new().ok_or(ConfigError::NoHome)?;
    Ok(base.home_dir().join(".config").join("sdl"))
}

/// Why the one-copy lock could not be asked for.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// The config directory is unknown.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The lock file could not be opened or locked.
    #[error("cannot use the lock file {}", path.display())]
    Io {
        /// The lock file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
}

/// Take the lock that lets only one copy of `sdl` run (`~/.config/sdl/lock`, F11). `Ok(None)`
/// when another copy holds it. Keep the file for the life of the process; the system releases the
/// lock when the process ends, however it ends.
///
/// # Errors
/// [`LockError`]: no home directory, or the file could not be opened or locked.
pub fn take_lock() -> Result<Option<std::fs::File>, LockError> {
    let path = config_dir()?.join("lock");
    lock_at(&path).map_err(|source| LockError::Io { path, source })
}

fn lock_at(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

impl Config {
    /// Path of the config file, `~/.config/sdl/config.toml`.
    ///
    /// # Errors
    /// [`ConfigError::NoHome`] when the home directory is unknown.
    pub fn path() -> Result<PathBuf, ConfigError> {
        Ok(config_dir()?.join("config.toml"))
    }

    /// Load the config from [`Config::path`].
    ///
    /// # Errors
    /// A missing, unreadable or malformed file, naming the path.
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::path()?;
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        Self::parse(&text, &path)
    }

    /// Parse config text with the real home directory; `path` only names the file in errors.
    ///
    /// # Errors
    /// Invalid TOML or a missing required key, or an unknown home directory.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let base = BaseDirs::new().ok_or(ConfigError::NoHome)?;
        Self::parse_with_home(text, path, base.home_dir())
    }

    /// Parse config text, expanding `~` to `home`; `path` only names the file in errors.
    ///
    /// # Errors
    /// Invalid TOML or a missing required key.
    fn parse_with_home(text: &str, path: &Path, home: &Path) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let music_root = match raw.music_root {
            Some(root) => expand_home(&root, home),
            None => home.join("Music"),
        };
        let my_library = match raw.my_library {
            Some(name) => Some(component(&name)).filter(|safe| !safe.is_empty()),
            None => Some(DEFAULT_MY_LIBRARY.to_owned()),
        }
        .ok_or_else(|| ConfigError::LibraryName {
            path: path.to_path_buf(),
        })?;
        Ok(Self {
            client_id: raw.client_id,
            music_root,
            my_library,
            download_jobs: raw.download_jobs.unwrap_or(DEFAULT_DOWNLOAD_JOBS),
            animations: raw.animations.unwrap_or(true),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn only_one_holder_of_the_lock_at_a_time() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("sdl").join("lock");
        let first = lock_at(&path)?;
        assert!(first.is_some(), "the first copy takes it");
        assert!(lock_at(&path)?.is_none(), "a second copy is refused");
        drop(first);
        assert!(
            lock_at(&path)?.is_some(),
            "free again once the first is gone"
        );
        Ok(())
    }

    #[test]
    fn full_file_parses() -> TestResult {
        let text =
            "client_id = \"abc\"\nmusic_root = \"/m\"\ndownload_jobs = 2\nanimations = false\n";
        let cfg = Config::parse(text, Path::new("/c.toml"))?;
        assert_eq!(cfg.client_id, "abc");
        assert_eq!(cfg.music_root, PathBuf::from("/m"));
        assert_eq!(cfg.download_jobs.get(), 2);
        assert!(!cfg.animations);
        Ok(())
    }

    #[test]
    fn defaults_apply() -> TestResult {
        let cfg = Config::parse("client_id = \"a\"\n", Path::new("/c.toml"))?;
        assert_eq!(cfg.download_jobs.get(), 4);
        assert!(cfg.animations);
        assert!(cfg.music_root.ends_with("Music"));
        assert_eq!(cfg.my_library, "Me");
        Ok(())
    }

    #[test]
    fn my_library_is_made_safe_and_must_not_be_empty() -> TestResult {
        let cfg = Config::parse(
            "client_id = \"a\"\nmy_library = \"Yazid: music\"\n",
            Path::new("/c.toml"),
        )?;
        assert_eq!(cfg.my_library, "Yazid music");
        for bad in ["", "..", "///"] {
            let err = Config::parse(
                &format!("client_id = \"a\"\nmy_library = \"{bad}\"\n"),
                Path::new("/c.toml"),
            );
            assert!(matches!(err, Err(ConfigError::LibraryName { .. })), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn default_music_root_is_home_music() -> TestResult {
        let cfg = Config::parse_with_home(
            "client_id = \"a\"\n",
            Path::new("/c.toml"),
            Path::new("/home/me"),
        )?;
        assert_eq!(cfg.music_root, PathBuf::from("/home/me/Music"));
        Ok(())
    }

    #[test]
    fn leading_tilde_is_expanded() -> TestResult {
        let cfg = Config::parse_with_home(
            "client_id = \"a\"\nmusic_root = \"~/Tunes\"\n",
            Path::new("/c.toml"),
            Path::new("/home/me"),
        )?;
        assert_eq!(cfg.music_root, PathBuf::from("/home/me/Tunes"));
        Ok(())
    }

    #[test]
    fn only_a_leading_tilde_component_is_expanded() {
        let home = Path::new("/h");
        assert_eq!(expand_home(Path::new("~"), home), PathBuf::from("/h"));
        assert_eq!(
            expand_home(Path::new("~bob/x"), home),
            PathBuf::from("~bob/x")
        );
        assert_eq!(
            expand_home(Path::new("/a/~/b"), home),
            PathBuf::from("/a/~/b")
        );
    }

    #[test]
    fn unknown_keys_are_ignored() -> TestResult {
        let cfg = Config::parse(
            "client_id = \"a\"\ncard_uuid = \"old\"\nsomething_new = 3\n",
            Path::new("/c.toml"),
        )?;
        assert_eq!(cfg.client_id, "a");
        Ok(())
    }

    #[test]
    fn missing_key_names_path() {
        let err = Config::parse("download_jobs = 2\n", Path::new("/x/config.toml"));
        let msg = err.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(msg.contains("/x/config.toml"), "{msg}");
    }

    #[test]
    fn zero_jobs_is_malformed() {
        let err = Config::parse(
            "client_id = \"a\"\ndownload_jobs = 0\n",
            Path::new("/c.toml"),
        );
        assert!(matches!(err, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn malformed_names_path() {
        let err = Config::parse("not toml ===", Path::new("/y/config.toml"));
        assert!(matches!(err, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn example_file_parses() -> TestResult {
        let text = include_str!("../../../config.example.toml");
        let cfg = Config::parse(text, Path::new("config.example.toml"))?;
        assert_eq!(cfg.download_jobs.get(), 4);
        Ok(())
    }
}
