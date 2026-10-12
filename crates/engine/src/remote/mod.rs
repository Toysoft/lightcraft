//! Nextcloud photo sources: the accounts this computer has, their app passwords (in a
//! [`CredentialStore`], never in the settings file), one [`Client`] per account and what the last
//! request said about each account ([`AccountState`]).
//!
//! Settings live in one JSON file (`{"accounts": [Account…], "cacheBudgetMb": 20000}`, see
//! [`crate::Session::with_default_nextcloud`]). It is hostile input: a missing, corrupt or oversized file
//! means no accounts, never an error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use craft_nextcloud::rustls::crypto::CryptoProvider;
pub use craft_nextcloud::{Account, RemotePath};
use craft_nextcloud::{Client, Error};
use serde::{Deserialize, Serialize};

use crate::activity::Activity;
use crate::denoise::{read_capped, write_atomic};

/// Largest settings file read; a bigger one is ignored.
const MAX_SETTINGS: usize = 1 << 20;
/// Default disk budget for downloaded originals (20 GB).
const DEFAULT_CACHE_BUDGET_MB: u64 = 20_000;
/// How long one failed connection marks an account unreachable.
const UNREACHABLE_FOR: Duration = Duration::from_secs(30);

/// Where app passwords are kept (the OS keyring in the apps; memory in tests).
pub trait CredentialStore: Send + Sync {
    fn get(&self, a: &Account) -> Result<Option<String>, String>;
    fn set(&self, a: &Account, password: &str) -> Result<(), String>;
    fn delete(&self, a: &Account) -> Result<(), String>;
}

/// App passwords in memory, by account key: for tests and hosts without a keyring.
#[derive(Default)]
pub struct MemoryCredentials(Mutex<HashMap<String, String>>);

impl CredentialStore for MemoryCredentials {
    fn get(&self, a: &Account) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap_or_else(PoisonError::into_inner).get(&a.key()).cloned())
    }
    fn set(&self, a: &Account, password: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).insert(a.key(), password.to_string());
        Ok(())
    }
    fn delete(&self, a: &Account) -> Result<(), String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&a.key());
        Ok(())
    }
}

/// Where a host keeps Nextcloud settings, downloaded originals and app passwords; `None` = nowhere.
#[derive(Clone, Default)]
pub struct RemoteConfig {
    pub settings_file: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub credentials: Option<Arc<dyn CredentialStore>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountState {
    SignedIn,
    /// No app password, or the server rejected it: requests fail until the account signs in again.
    SignedOut,
    /// The last request couldn't reach the server (for [`UNREACHABLE_FOR`]).
    Unreachable,
    SigningIn,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    accounts: Vec<Account>,
    cache_budget_mb: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { accounts: Vec::new(), cache_budget_mb: DEFAULT_CACHE_BUDGET_MB }
    }
}

#[derive(Default)]
struct Inner {
    config: RemoteConfig,
    settings: Settings,
    clients: HashMap<String, Client>,
    /// What the last request said about an account, and when; none = signed in.
    states: HashMap<String, (AccountState, Instant)>,
}

impl Inner {
    fn account(&self, key: &str) -> Option<&Account> {
        self.settings.accounts.iter().find(|a| a.key() == key)
    }

    fn state_at(&self, key: &str, now: Instant) -> AccountState {
        match self.states.get(key) {
            Some((AccountState::Unreachable, at)) if now.saturating_duration_since(*at) >= UNREACHABLE_FOR => AccountState::SignedIn,
            Some((state, _)) => *state,
            None => AccountState::SignedIn,
        }
    }

    fn state(&self, key: &str) -> AccountState {
        self.state_at(key, Instant::now())
    }

    /// Write `settings` to the settings file (if any), then keep them.
    fn save(&mut self, settings: Settings) -> Result<(), String> {
        if let Some(path) = &self.config.settings_file {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            }
            write_atomic(path, &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?)?;
        }
        self.settings = settings;
        Ok(())
    }
}

fn unknown(key: &str) -> String {
    format!("No Nextcloud account {key} on this computer; add it with File ▸ Add Nextcloud Account…")
}

fn sign_in_again(key: &str) -> String {
    format!("Sign in to Nextcloud ({key}) again: Settings ▸ Nextcloud")
}

/// The session's Nextcloud accounts. Shared (`Arc`) with the threads that load remote photos.
pub struct Remote {
    // Sign-in and downloads (NC4, NC5) show their rows here.
    #[allow(dead_code)]
    activity: Activity,
    inner: Mutex<Inner>,
}

impl Remote {
    /// No accounts and nowhere to keep any until [`Remote::configure`].
    pub fn new(activity: Activity) -> Remote {
        Remote { activity, inner: Mutex::default() }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Use `config` from now on: reads its settings file and forgets clients and account states.
    pub fn configure(&self, config: RemoteConfig) {
        let settings = config
            .settings_file
            .as_deref()
            .and_then(|p| read_capped(p, MAX_SETTINGS))
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        *self.lock() = Inner { config, settings, ..Inner::default() };
    }

    pub fn accounts(&self) -> Vec<(Account, AccountState)> {
        let inner = self.lock();
        inner.settings.accounts.iter().map(|a| (a.clone(), inner.state(&a.key()))).collect()
    }

    /// The client for `account_key` (`user@host[:port]`), built once with the stored app password.
    pub fn client(&self, account_key: &str) -> Result<Client, String> {
        // the account is configured and not signed out: checked again after the unlocked password read,
        // since another request may have been rejected (or the account removed) meanwhile
        let usable = |inner: &Inner| -> Result<Account, String> {
            let account = inner.account(account_key).cloned().ok_or_else(|| unknown(account_key))?;
            if inner.state(account_key) == AccountState::SignedOut {
                return Err(sign_in_again(account_key));
            }
            Ok(account)
        };
        let (account, store) = {
            let inner = self.lock();
            let account = usable(&inner)?;
            if let Some(c) = inner.clients.get(account_key) {
                return Ok(c.clone());
            }
            (account, inner.config.credentials.clone())
        };
        // the store may be the OS keyring (slow, may prompt): not under the lock
        let password = match store {
            Some(store) => store.get(&account)?,
            None => None,
        };
        let Some(password) = password else {
            self.note_error(account_key, &Error::Unauthorized);
            return Err(sign_in_again(account_key));
        };
        let client = Client::new(account, password, provider()).map_err(|e| e.to_string())?;
        let mut inner = self.lock();
        usable(&inner)?;
        inner.clients.insert(account_key.to_string(), client.clone());
        Ok(client)
    }

    /// What a failed request says about its account: a rejected password signs it out (requests
    /// short-circuit until it signs in again), a connection failure marks it unreachable for 30 s.
    /// The one place errors change an account's state.
    pub(crate) fn note_error(&self, account_key: &str, e: &Error) {
        let mut inner = self.lock();
        let state = match e {
            Error::Unauthorized => {
                inner.clients.remove(account_key);
                AccountState::SignedOut
            }
            Error::Unreachable(_) if inner.state(account_key) != AccountState::SignedOut => AccountState::Unreachable,
            _ => return,
        };
        inner.states.insert(account_key.to_string(), (state, Instant::now()));
    }

    /// Forget an account on this computer: its app password, its settings entry and its client. Its
    /// photos stay in the catalog (offline).
    pub fn remove_account(&self, key: &str) -> Result<(), String> {
        let (account, store) = {
            let inner = self.lock();
            (inner.account(key).cloned().ok_or_else(|| unknown(key))?, inner.config.credentials.clone())
        };
        // the password goes first: a failure keeps the account so the user can retry
        if let Some(store) = store {
            store.delete(&account)?;
        }
        let mut inner = self.lock();
        inner.clients.remove(key);
        inner.states.remove(key);
        let mut settings = inner.settings.clone();
        settings.accounts.retain(|a| a.key() != key);
        inner.save(settings)
    }

    /// Disk budget for downloaded originals, in MB (10^6 bytes).
    pub fn cache_budget_mb(&self) -> u64 {
        self.lock().settings.cache_budget_mb
    }

    pub fn set_cache_budget_mb(&self, mb: u64) -> Result<(), String> {
        let mut inner = self.lock();
        let settings = Settings { cache_budget_mb: mb, ..inner.settings.clone() };
        inner.save(settings)
    }
}

/// The TLS crypto provider (pure Rust, like `lightcraft-fetch`).
pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls_rustcrypto::provider())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_lasts_thirty_seconds() {
        let r = Remote::new(Activity::default());
        r.note_error("bob@x", &Error::Unreachable("timed out".into()));
        let inner = r.lock();
        let now = Instant::now();
        assert_eq!(inner.state_at("bob@x", now), AccountState::Unreachable);
        assert_eq!(inner.state_at("bob@x", now + UNREACHABLE_FOR), AccountState::SignedIn);
    }
}
