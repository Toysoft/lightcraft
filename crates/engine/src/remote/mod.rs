//! Nextcloud photo sources: the accounts this computer has, their app passwords (in a
//! [`CredentialStore`], never in the settings file), one [`Client`] per account and what the last
//! request said about each account ([`AccountState`]).
//!
//! Settings live in one JSON file (`{"accounts": [Account…], "cacheBudgetMb": 20000}`, see
//! [`crate::Session::with_default_nextcloud`]). It is hostile input: a missing, corrupt or oversized file
//! means no accounts, never an error.
//!
//! Signing in ([`Remote::sign_in`]) is Login Flow v2: the user approves LightCraft in a browser while a
//! thread asks the server every 2 s, shown as a cancellable `nextcloud` activity row. How the last few
//! sign-ins went is kept for the UI ([`Remote::sign_ins`]).
//!
//! Photos load through the engine's file hooks ([`hooks`]): from the originals downloaded to this
//! computer ([`Remote::local_original`]), the server's previews and a Range read of the file's start.
//!
//! Develop holds the open photo's original on disk ([`Remote::hold`]) and downloads the next one ahead
//! ([`Remote::prefetch`]); "Make Available Offline" pins copies ([`Remote::make_available_offline`]).
//!
//! Work done off the session (an import's listing and metadata, see [`import`]; a finished download)
//! comes back as [`Event`]s, which [`crate::Session::drain_remote`] applies to the catalog.

pub(crate) mod hooks;
pub(crate) mod import;
mod originals;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use craft_nextcloud::rustls::crypto::CryptoProvider;
pub use craft_nextcloud::{Account, Fingerprint, RemotePath};
use craft_nextcloud::{Client, Error, Hold, SignIn, SignInState};
pub use originals::NOT_DOWNLOADED;
#[cfg(test)]
pub(crate) use originals::budget_bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::activity::{Activity, Cancel};
use crate::denoise::{read_capped, write_atomic};

/// Largest settings file read; a bigger one is ignored.
const MAX_SETTINGS: usize = 1 << 20;
/// Default disk budget for downloaded originals (20 GB).
const DEFAULT_CACHE_BUDGET_MB: u64 = 20_000;
/// How long one failed connection marks an account unreachable.
const UNREACHABLE_FOR: Duration = Duration::from_secs(30);
/// How long an answer from an account's server counts as "it is reachable" ([`Remote::exists`]).
const REACHABLE_FOR: Duration = Duration::from_secs(30);
/// How often a waiting sign-in asks the server whether the user approved it.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// How many sign-in outcomes are kept ([`Remote::sign_ins`]).
const MAX_SIGN_INS: usize = 8;

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

/// App passwords in the OS keyring (Keychain, Credential Manager, Secret Service), under the service
/// "LightCraft" and the account's key; the store the apps install ([`crate::Session::with_default_nextcloud`]).
#[cfg(feature = "keyring")]
pub struct KeyringCredentials;

#[cfg(feature = "keyring")]
impl CredentialStore for KeyringCredentials {
    fn get(&self, a: &Account) -> Result<Option<String>, String> {
        craft_nextcloud::load_password("LightCraft", a).map_err(|e| e.to_string())
    }
    fn set(&self, a: &Account, password: &str) -> Result<(), String> {
        craft_nextcloud::store_password("LightCraft", a, password).map_err(|e| e.to_string())
    }
    fn delete(&self, a: &Account) -> Result<(), String> {
        craft_nextcloud::delete_password("LightCraft", a).map_err(|e| e.to_string())
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

/// Where one sign-in stands ([`SignInOutcome::state`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SignInProgress {
    /// Waiting for the user to approve it in the browser.
    Waiting,
    SignedIn,
    Failed,
    /// Cancelled from its activity row.
    Cancelled,
    /// Not approved before craft's deadline (20 minutes).
    TimedOut,
}

/// How a sign-in went, by its activity row's id (`task` in the `waiting` reply).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SignInOutcome {
    pub task: u64,
    pub state: SignInProgress,
    /// The account signed in, or being signed in again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Why it didn't sign in, for the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    /// Etag and size of remote files, from listings and `stat`s (this session).
    known: HashMap<RemotePath, (String, u64)>,
    /// Files the server said it no longer has, until a listing has them again.
    gone: HashSet<RemotePath>,
    /// The names in each folder listed this session.
    listed: HashMap<RemotePath, HashSet<String>>,
    /// Clients built from now on give up on a server silent for this long (craft's default if `None`).
    stall_limit: Option<Duration>,
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

fn no_store() -> String {
    "Nextcloud sign-in isn't available in this window".into()
}

/// What a failed sign-in or client says to the user: craft's text, naming LightCraft where craft says "Craft".
fn message(e: &Error) -> String {
    match e {
        Error::InvalidAddress(d) => format!("That isn't a Nextcloud address LightCraft can use ({d}); check what you typed"),
        e => e.to_string(),
    }
}

/// A sign-in that ended other than signed in: how, and why in words for the user.
type SignInEnd = (SignInProgress, String);

/// What work off the session found, for [`crate::Session::drain_remote`] to apply. `Found` and `Meta`
/// come from import `task` (its activity row's id) into the library of `generation`
/// ([`crate::Session::library_generation`]); another library's are dropped.
pub enum Event {
    /// Photos an import listed, to add (path, what the listing and Memories told: size, kind,
    /// format; dimensions and capture time when Memories knows them).
    Found { task: u64, generation: u64, photos: Vec<(String, crate::media::ProbeInfo)> },
    /// Metadata an import read, filling what the photos lack.
    Meta { task: u64, generation: u64, reads: Vec<MetaRead> },
    /// The original at `path` was downloaded: the photos with that path (in whatever library is
    /// open) are completed with what the whole file says (`info`, read by the downloading thread;
    /// `None` when it couldn't be described).
    Downloaded { path: String, info: Option<Box<crate::media::ProbeInfo>> },
}

/// The metadata read for the photo at `path`. With `described`, `info` describes the whole file
/// (dimensions, preview-only), else only its metadata and capture time.
pub struct MetaRead {
    pub path: String,
    pub info: crate::media::ProbeInfo,
    pub described: bool,
}

/// Where an import stands ([`ImportOutcome::state`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportProgress {
    Running,
    Done,
    /// Stopped by an error ([`ImportOutcome::error`]).
    Failed,
    /// Cancelled from its activity row, or by opening another library.
    Cancelled,
}

/// How an import went, by its activity row's id (`task` in the `wait: false` reply).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub task: u64,
    pub state: ImportProgress,
    /// Photos added to the library so far (or brought back from Recently Deleted).
    pub added: usize,
    /// Folders below the ones asked for that couldn't be listed.
    pub failed_folders: usize,
    /// Why it failed, for the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The session's Nextcloud accounts. Shared (`Arc`) with the threads that load remote photos.
pub struct Remote {
    /// Sign-ins and downloads show their rows here.
    activity: Activity,
    inner: Mutex<Inner>,
    /// Held while an original downloads: one at a time.
    downloads: Mutex<()>,
    /// Interactive loads waiting for [`Remote::downloads`]: prefetches and offline copies give it up
    /// to them.
    pub(crate) loads_waiting: AtomicUsize,
    /// The photo open in Develop, the hold keeping its original from eviction, and what cancels its
    /// prefetch when one was running as it opened ([`Remote::hold`]).
    held: Mutex<Option<(String, Hold, Option<Arc<AtomicBool>>)>>,
    /// The path last prefetched (or found on disk) and what cancels its prefetch ([`Remote::prefetch`]).
    prefetching: Mutex<Option<(String, Arc<AtomicBool>)>>,
    /// The last [`MAX_SIGN_INS`] sign-ins, oldest first. Kept across [`Remote::configure`]: they are
    /// this session's.
    sign_ins: Mutex<VecDeque<SignInOutcome>>,
    /// Waiting for [`crate::Session::drain_remote`], oldest first.
    events: Mutex<Vec<Event>>,
    /// The last [`import::MAX_IMPORTS`] imports, oldest first ([`Remote::imports`]).
    imports: Mutex<VecDeque<import::ImportRecord>>,
    /// Sign-ins started from now on are past craft's deadline at once (tests of the timeout).
    #[cfg(test)]
    pub(crate) expire_sign_ins: AtomicBool,
    /// When set, an import waits for a message here before it lists and before it reads metadata
    /// (tests of its activity rows).
    #[cfg(test)]
    pub(crate) import_steps: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// When set, a download waits for a message here once its row is up (tests of the rows).
    #[cfg(test)]
    pub(crate) download_steps: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Remote {
    /// No accounts and nowhere to keep any until [`Remote::configure`].
    pub fn new(activity: Activity) -> Remote {
        Remote {
            activity,
            inner: Mutex::default(),
            downloads: Mutex::default(),
            loads_waiting: AtomicUsize::new(0),
            held: Mutex::default(),
            prefetching: Mutex::default(),
            sign_ins: Mutex::default(),
            events: Mutex::default(),
            imports: Mutex::default(),
            #[cfg(test)]
            expire_sign_ins: AtomicBool::new(false),
            #[cfg(test)]
            import_steps: Mutex::default(),
            #[cfg(test)]
            download_steps: Mutex::default(),
        }
    }

    /// Queue `event` for [`crate::Session::drain_remote`].
    pub(crate) fn push_event(&self, event: Event) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }

    /// The queued events, oldest first; the queue is empty afterwards.
    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Use `config` from now on: reads its settings file and forgets clients, account states and what
    /// it knew about remote files.
    pub fn configure(&self, config: RemoteConfig) {
        let mut settings: Settings = config
            .settings_file
            .as_deref()
            .and_then(|p| read_capped(p, MAX_SETTINGS))
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // (a hand-edited file may hold 0)
        settings.cache_budget_mb = settings.cache_budget_mb.max(originals::MIN_CACHE_BUDGET_MB);
        *self.lock() = Inner { config, settings, ..Inner::default() };
    }

    /// Clients built from now on give up on a server that sends nothing for `limit` (tests).
    #[cfg(test)]
    pub(crate) fn set_stall_limit(&self, limit: Duration) {
        let mut inner = self.lock();
        inner.stall_limit = Some(limit);
        inner.clients.clear();
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
            if matches!(inner.state(account_key), AccountState::SignedOut | AccountState::SigningIn) {
                return Err(sign_in_again(account_key));
            }
            Ok(account)
        };
        let (account, store, stall_limit) = {
            let inner = self.lock();
            let account = usable(&inner)?;
            if let Some(c) = inner.clients.get(account_key) {
                return Ok(c.clone());
            }
            (account, inner.config.credentials.clone(), inner.stall_limit)
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
        let mut client = Client::new(account, password, provider()).map_err(|e| message(&e))?;
        if let Some(limit) = stall_limit {
            client = client.with_stall_limit(limit).map_err(|e| message(&e))?;
        }
        let mut inner = self.lock();
        usable(&inner)?;
        inner.clients.insert(account_key.to_string(), client.clone());
        Ok(client)
    }

    /// What a failed request says about its account: a rejected password or a changed certificate
    /// signs it out (requests short-circuit until it signs in again; for a changed certificate that
    /// answers `needsTrust` with the new fingerprint, re-pinned only once the user confirms it with
    /// `trust`), a connection failure marks it unreachable for 30 s.
    /// The one place errors change an account's state.
    pub(crate) fn note_error(&self, account_key: &str, e: &Error) {
        let mut inner = self.lock();
        let state = match e {
            Error::Unauthorized | Error::CertificateChanged => {
                inner.clients.remove(account_key);
                AccountState::SignedOut
            }
            Error::Unreachable(_) if !matches!(inner.state(account_key), AccountState::SignedOut | AccountState::SigningIn) => {
                AccountState::Unreachable
            }
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

    /// Sign in to `server` (as the user typed it) and add the account, or replace it when it's already
    /// here. The first request runs here; it answers `{"state": "needsTrust", "fingerprint"}` for an
    /// untrusted certificate unless `trust` is that fingerprint (a different one is an error). Then a
    /// thread waits for the approval, holding a cancellable activity row, and this returns
    /// `{"state": "waiting", "loginUrl", "task"}` (`task` = the row's id, see [`Remote::sign_ins`]);
    /// with `wait` it returns `{"state": "signedIn", "account"}` once approved (or the failure).
    pub fn sign_in(self: &Arc<Self>, server: &str, trust: Option<Fingerprint>, wait: bool) -> Result<Value, String> {
        self.start_sign_in(server, trust, None, None, wait)
    }

    /// [`Remote::sign_in`] again for account `key`, with its server and pinned certificate. A
    /// certificate other than the pinned one answers `{"state": "needsTrust", "fingerprint",
    /// "changed": true}`; `trust` naming it re-pins the account once approved. The account is
    /// `SigningIn` while the sign-in waits, and signed out if it fails or is cancelled.
    pub fn sign_in_again(self: &Arc<Self>, key: &str, trust: Option<Fingerprint>, wait: bool) -> Result<Value, String> {
        let account = self.lock().account(key).cloned().ok_or_else(|| unknown(key))?;
        self.start_sign_in(&account.server, trust, account.trust, Some(key), wait)
    }

    /// `trust`: the fingerprint the user confirmed for this call; `pinned`: the account's.
    fn start_sign_in(
        self: &Arc<Self>,
        server: &str,
        trust: Option<Fingerprint>,
        pinned: Option<Fingerprint>,
        again: Option<&str>,
        wait: bool,
    ) -> Result<Value, String> {
        if self.lock().config.credentials.is_none() {
            return Err(no_store());
        }
        // A sign-in object isn't kept between calls: answering `needsTrust` means starting again with
        // `trust`, and the certificate it then finds must be the one the user saw.
        let mut sign_in = SignIn::start(server, "LightCraft", provider());
        if let SignInState::NeedsTrust { fingerprint } = *sign_in.state() {
            if trust.or(pinned) == Some(fingerprint) {
                sign_in.confirm_trust(true);
            } else if trust.is_some() {
                return Err("The certificate's fingerprint changed; check it with the server's admin".into());
            } else if pinned.is_some() {
                return Ok(json!({"state": "needsTrust", "fingerprint": fingerprint.to_string(), "changed": true}));
            } else {
                return Ok(json!({"state": "needsTrust", "fingerprint": fingerprint.to_string()}));
            }
        }
        #[cfg(test)]
        if self.expire_sign_ins.load(Ordering::Relaxed) {
            sign_in.set_deadline_for_test(Instant::now());
        }
        let login_url = match sign_in.state() {
            SignInState::Waiting { login_url } => login_url.clone(),
            SignInState::Failed(e) => return Err(message(e)),
            // the start ends waiting or failed once trust is answered
            SignInState::NeedsTrust { .. } | SignInState::Done { .. } => return Err(Error::Malformed("unexpected sign-in state".into()).to_string()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let task = self.activity.start("nextcloud", "Waiting for Nextcloud approval…", Cancel::Flag(cancel.clone()));
        let id = task.id();
        let again = again.map(str::to_string);
        if let Some(key) = &again {
            self.set_state_of(key, |_| true, AccountState::SigningIn);
        }
        self.note_sign_in(SignInOutcome { task: id, state: SignInProgress::Waiting, account: again.clone(), error: None });
        if wait {
            // the caller only hears back once approved: a CLI shows the page to open here (never the password)
            log::warn!("Approve LightCraft in your browser: {login_url}");
            let key = self.wait_for_approval(id, sign_in, &cancel, again.as_deref())?;
            return Ok(json!({"state": "signedIn", "account": key}));
        }
        let remote = self.clone();
        let thread_again = again.clone();
        std::thread::Builder::new()
            .name("nextcloud-sign-in".into())
            .spawn(move || {
                // the row goes when the thread ends, however it ends
                let _task = task;
                if let Err(e) = remote.wait_for_approval(id, sign_in, &cancel, thread_again.as_deref()) {
                    log::warn!("Nextcloud sign-in: {e}");
                }
            })
            .map_err(|e| {
                let e = format!("could not start the sign-in: {e}");
                let _ = self.end_sign_in(id, again.as_deref(), Err((SignInProgress::Failed, e.clone())));
                e
            })?;
        Ok(json!({"state": "waiting", "loginUrl": login_url, "task": id}))
    }

    /// Ask every 2 s until the user approves (then keep the account), the sign-in fails or times
    /// out, or `cancel` is set; then [`Remote::end_sign_in`].
    ///
    /// Everything it changes goes through `Remote` as configured *then*: after a
    /// [`Remote::configure`] meanwhile, an approved account joins the new settings and a forgotten
    /// `SigningIn` state is left alone.
    fn wait_for_approval(&self, task: u64, mut sign_in: SignIn, cancel: &AtomicBool, again: Option<&str>) -> Result<String, String> {
        let cancelled = || (SignInProgress::Cancelled, Error::Cancelled.to_string());
        let ended = loop {
            let asked = Instant::now();
            while asked.elapsed() < POLL_EVERY && !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            if cancel.load(Ordering::Relaxed) {
                break Err(cancelled());
            }
            match sign_in.poll() {
                SignInState::Waiting { .. } => {}
                SignInState::Done { account, app_password } => {
                    break self.add_signed_in(account.clone(), app_password).map_err(|e| (SignInProgress::Failed, e));
                }
                // craft gives up with `Cancelled` at its deadline; only the row's flag is a user's cancel
                SignInState::Failed(Error::Cancelled) if cancel.load(Ordering::Relaxed) => break Err(cancelled()),
                SignInState::Failed(Error::Cancelled) => {
                    break Err((SignInProgress::TimedOut, "The sign-in wasn't approved in time; start it again".into()));
                }
                SignInState::Failed(e) => break Err((SignInProgress::Failed, message(e))),
                SignInState::NeedsTrust { .. } => {
                    break Err((SignInProgress::Failed, Error::Malformed("unexpected sign-in state".into()).to_string()));
                }
            }
        };
        self.end_sign_in(task, again, ended)
    }

    /// Note how sign-in `task` ended, and sign an `again` account still signing in out.
    fn end_sign_in(&self, task: u64, again: Option<&str>, ended: Result<String, SignInEnd>) -> Result<String, String> {
        if let Some(key) = again {
            self.set_state_of(key, |s| s == AccountState::SigningIn, AccountState::SignedOut);
        }
        let outcome = match &ended {
            Ok(key) => SignInOutcome { task, state: SignInProgress::SignedIn, account: Some(key.clone()), error: None },
            Err((state, e)) => SignInOutcome { task, state: *state, account: again.map(str::to_string), error: Some(e.clone()) },
        };
        self.note_sign_in(outcome);
        ended.map_err(|(_, e)| e)
    }

    /// Keep `outcome`, replacing the one for its task; the oldest goes past [`MAX_SIGN_INS`].
    fn note_sign_in(&self, outcome: SignInOutcome) {
        let mut kept = self.sign_ins.lock().unwrap_or_else(PoisonError::into_inner);
        match kept.iter_mut().find(|o| o.task == outcome.task) {
            Some(o) => *o = outcome,
            None => {
                kept.push_back(outcome);
                if kept.len() > MAX_SIGN_INS {
                    kept.pop_front();
                }
            }
        }
    }

    /// How the last 8 sign-ins went (or stand), oldest first.
    pub fn sign_ins(&self) -> Vec<SignInOutcome> {
        self.sign_ins.lock().unwrap_or_else(PoisonError::into_inner).iter().cloned().collect()
    }

    /// An approved sign-in: the password goes to the store, the account (with its trusted
    /// certificate) is added to the settings or replaces the one with its key, and it's signed in
    /// with a client built afresh.
    fn add_signed_in(&self, account: Account, password: &str) -> Result<String, String> {
        let store = self.lock().config.credentials.clone().ok_or_else(no_store)?;
        // the store may be the OS keyring (slow, may prompt): not under the lock
        store.set(&account, password)?;
        let key = account.key();
        let mut inner = self.lock();
        let mut settings = inner.settings.clone();
        match settings.accounts.iter_mut().find(|a| a.key() == key) {
            Some(old) => *old = account,
            None => settings.accounts.push(account),
        }
        inner.save(settings)?;
        inner.clients.remove(&key);
        // ponytail: a request that started with the old password before this sign-in can still answer
        // 401 and sign the account out again (`note_error`); a per-account sign-in generation that
        // `note_error` checks would ignore such late answers, if they show up.
        inner.states.remove(&key);
        Ok(key)
    }

    /// Set account `key`'s state to `to` if its current state passes `when`; nothing for an account
    /// that isn't (or is no longer) configured.
    fn set_state_of(&self, key: &str, when: impl Fn(AccountState) -> bool, to: AccountState) {
        let mut inner = self.lock();
        if inner.account(key).is_some() && when(inner.state(key)) {
            inner.states.insert(key.to_string(), (to, Instant::now()));
        }
    }

    /// Disk budget for downloaded originals, in MB (10^6 bytes).
    pub fn cache_budget_mb(&self) -> u64 {
        self.lock().settings.cache_budget_mb
    }

    /// Set the disk budget for downloaded originals (at least 1000 MB), evicting down to it.
    pub fn set_cache_budget_mb(&self, mb: u64) -> Result<(), String> {
        let mb = mb.max(originals::MIN_CACHE_BUDGET_MB);
        {
            let mut inner = self.lock();
            let settings = Settings { cache_budget_mb: mb, ..inner.settings.clone() };
            inner.save(settings)?;
        }
        // asking for the cache gives it the new budget (one that can't be opened now takes it when it is)
        let _ = self.cache();
        Ok(())
    }
}

/// The TLS crypto provider (pure Rust, like `lightcraft-fetch`); hosts' tests start craft's TLS
/// `FakeServer` with it.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls_rustcrypto::provider())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_eight_sign_ins() {
        let r = Remote::new(Activity::default());
        for task in 1..=10 {
            r.note_sign_in(SignInOutcome { task, state: SignInProgress::Waiting, account: None, error: None });
        }
        r.note_sign_in(SignInOutcome { task: 9, state: SignInProgress::SignedIn, account: Some("a@b".into()), error: None });
        let kept = r.sign_ins();
        assert_eq!(kept.iter().map(|o| o.task).collect::<Vec<_>>(), [3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(kept[6].state, SignInProgress::SignedIn, "updated in place");
    }

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
