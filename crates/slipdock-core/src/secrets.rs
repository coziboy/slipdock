//! Passwords in the system keychain (opt-in per profile).
//!
//! By default a profile's password is a line in its file. A profile with
//! `password-store = keychain` under `[slipdock]` has no such line; the
//! password is kept by the operating system (Keychain on macOS, Secret
//! Service on Linux) and fetched for each run.
//!
//! The rest of the core talks to the [`SecretStore`] trait, never to the
//! keychain directly. Tests pass a [`MemoryStore`], so no test reads or
//! writes the real keychain.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::profile::{Profile, SECTION};
use crate::{Error, Result};

/// The name under which Slipdock's entries appear in the keychain.
const SERVICE: &str = "slipdock";

pub trait SecretStore: Send + Sync {
    /// The password of a profile; `None` when none is stored.
    fn get(&self, profile: &str) -> Result<Option<String>>;
    fn set(&self, profile: &str, password: &str) -> Result<()>;
    /// Removes the password. Removing one that is not there is fine.
    fn clear(&self, profile: &str) -> Result<()>;
}

/// The real keychain of this system.
pub struct Keychain;

impl Keychain {
    fn entry(profile: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, profile).map_err(keychain_error)
    }
}

fn keychain_error(error: keyring::Error) -> Error {
    Error::msg(format!("the system keychain refused: {error}"))
}

impl SecretStore for Keychain {
    fn get(&self, profile: &str) -> Result<Option<String>> {
        match Self::entry(profile)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(error)),
        }
    }

    fn set(&self, profile: &str, password: &str) -> Result<()> {
        Self::entry(profile)?
            .set_password(password)
            .map_err(keychain_error)
    }

    fn clear(&self, profile: &str) -> Result<()> {
        match Self::entry(profile)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(error)),
        }
    }
}

/// Moves a profile's password into the store: stores it, marks the profile
/// and removes the password line. The caller saves the profile afterwards.
/// The store is written first, so a refusal leaves the profile as it was.
pub fn move_to_store(store: &dyn SecretStore, profile: &mut Profile, password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(Error::msg("no password given; nothing changed"));
    }
    store.set(&profile.name, password)?;
    profile.doc.set(SECTION, "password-store", "keychain")?;
    profile.doc.remove("client", "password");
    Ok(())
}

/// The way back: removes the password from the store and the mark from the
/// profile. With `password`, the line under `[client]` is written again;
/// without, the caller tells the user to add it. The caller saves the profile.
pub fn move_to_file(
    store: &dyn SecretStore,
    profile: &mut Profile,
    password: Option<&str>,
) -> Result<()> {
    if let Some(password) = password {
        profile.doc.set("client", "password", password)?;
    }
    profile.doc.remove(SECTION, "password-store");
    store.clear(&profile.name)
}

/// Keeps passwords in memory. For tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, profile: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(profile).cloned())
    }

    fn set(&self, profile: &str, password: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(profile.to_string(), password.to_string());
        Ok(())
    }

    fn clear(&self, profile: &str) -> Result<()> {
        self.0.lock().unwrap().remove(profile);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_sets_gets_and_clears() {
        let store = MemoryStore::default();
        assert_eq!(store.get("example").unwrap(), None);
        store.set("example", "secret").unwrap();
        assert_eq!(store.get("example").unwrap().as_deref(), Some("secret"));
        assert_eq!(store.get("other").unwrap(), None);
        store.clear("example").unwrap();
        store.clear("example").unwrap();
        assert_eq!(store.get("example").unwrap(), None);
    }

    #[test]
    fn a_password_moves_to_the_store_and_back() {
        use crate::profile::{Document, PasswordStore};
        let store = MemoryStore::default();
        let mut profile = Profile {
            name: "example".into(),
            path: "/unused".into(),
            doc: Document::parse("[client]\nhost = h\npassword = old\n"),
        };

        assert!(move_to_store(&store, &mut profile, "").is_err());
        assert_eq!(profile.password_store(), PasswordStore::File);

        move_to_store(&store, &mut profile, "s3cret").unwrap();
        assert_eq!(store.get("example").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(profile.password_store(), PasswordStore::Keychain);
        assert_eq!(profile.doc.get("client", "password"), None);

        move_to_file(&store, &mut profile, Some("s3cret")).unwrap();
        assert_eq!(store.get("example").unwrap(), None);
        assert_eq!(profile.password_store(), PasswordStore::File);
        assert_eq!(profile.doc.get("client", "password"), Some("s3cret"));
        // The emptied [slipdock] section stays; that is harmless.
        assert_eq!(
            profile.doc.to_text(),
            "[client]\nhost = h\npassword = s3cret\n\n[slipdock]\n"
        );
    }
}
