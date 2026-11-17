//! Secrets live in the OS credential store (macOS Keychain / Windows Credential Manager),
//! never in settings.toml or SQLite.

use pulsecut_core::{Error, Result};

pub const SERVICE: &str = "app.pulsecut.PulseCut";

/// Well-known secret names.
pub mod keys {
    pub const OBS_PASSWORD: &str = "obs-websocket";
    pub const YOUTUBE_API_KEY: &str = "youtube-api-key";
    pub const YOUTUBE_OAUTH: &str = "youtube-oauth";
    pub const YOUTUBE_CLIENT_SECRET: &str = "youtube-client-secret";
    pub const INSTAGRAM_TOKEN: &str = "instagram-token";
    pub const TIKTOK_OAUTH: &str = "tiktok-oauth";
    pub const TIKTOK_CLIENT_SECRET: &str = "tiktok-client-secret";
    pub const TWITCH_CLIENT_SECRET: &str = "twitch-client-secret";
}

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>>;
    fn set(&self, name: &str, value: &str) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
}

pub struct Keychain;

impl SecretStore for Keychain {
    fn get(&self, name: &str) -> Result<Option<String>> {
        let e = keyring::Entry::new(SERVICE, name).map_err(|e| Error::other(e.to_string()))?;
        match e.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::other(format!("keychain: {e}"))),
        }
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        keyring::Entry::new(SERVICE, name)
            .and_then(|e| e.set_password(value))
            .map_err(|e| Error::other(format!("keychain: {e}")))
    }
    fn delete(&self, name: &str) -> Result<()> {
        match keyring::Entry::new(SERVICE, name).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::other(format!("keychain: {e}"))),
        }
    }
}

/// In-memory store for tests and the offline demo.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}
