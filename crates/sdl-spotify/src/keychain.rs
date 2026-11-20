//! Where the refresh token is kept: the [`TokenStore`] seam and its real adapter, the macOS
//! Keychain through the `keyring` crate (service `sdl`, account `spotify`, nowhere else).
//!
//! `keyring` 4.2.0 with its default `v1` feature selects the real macOS Keychain store:
//! `src/v1.rs` of the crate, `set_credential_store()`, calls
//! `apple_native_keyring_store::keychain::Store::new()` on macOS and installs it with
//! `keyring_core::set_default_store`; `Cargo.toml` feature `v1` enables
//! `apple-native-keyring-store/keychain`. Without `v1` the crate has no `Entry` at all, so there
//! is no mock store to fall into. Tests therefore use their own in-memory [`TokenStore`] and never
//! reach this adapter.

use keyring::{Entry, Error};

const SERVICE: &str = "sdl";
const ACCOUNT: &str = "spotify";

/// A place that remembers one refresh token. Errors are plain words fit for the owner.
pub(crate) trait TokenStore: Send + Sync {
    /// The stored refresh token; `Ok(None)` when none is stored.
    fn read(&self) -> Result<Option<String>, String>;
    /// Replace the stored refresh token.
    fn write(&self, token: &str) -> Result<(), String>;
}

/// The macOS Keychain entry for service `sdl`, account `spotify`.
pub(crate) struct Keychain;

impl TokenStore for Keychain {
    fn read(&self) -> Result<Option<String>, String> {
        match entry()?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(plain("read", &error)),
        }
    }

    fn write(&self, token: &str) -> Result<(), String> {
        entry()?
            .set_password(token)
            .map_err(|error| plain("save", &error))
    }
}

fn entry() -> Result<Entry, String> {
    Entry::new(SERVICE, ACCOUNT).map_err(|error| plain("open", &error))
}

fn plain(action: &str, error: &Error) -> String {
    format!(
        "The macOS Keychain could not {action} the Spotify sign-in ({error}). Sign-in will ask for the browser each time until it can."
    )
}
