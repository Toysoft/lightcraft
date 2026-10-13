//! Remote originals on this computer: the [`OriginalsCache`] (one per folder in the process), the
//! etags and sizes listings and `stat`s told us, which files the server no longer has, and whether
//! a remote photo counts as there ([`Remote::exists`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use craft_nextcloud::{Client, Entry, Error, OriginalsCache, RemotePath};

use super::{AccountState, REACHABLE_FOR, Remote};
use crate::activity::{Cancel, Unit};

/// A background load (grid thumbnail, prefetch, batch job) found no local copy of the original.
pub const NOT_DOWNLOADED: &str = "The original isn't downloaded yet";

/// The smallest disk budget for downloaded originals, in MB.
pub(crate) const MIN_CACHE_BUDGET_MB: u64 = 1000;

/// `mb` (at least [`MIN_CACHE_BUDGET_MB`]) in bytes.
pub(crate) fn budget_bytes(mb: u64) -> u64 {
    mb.max(MIN_CACHE_BUDGET_MB).saturating_mul(1_000_000)
}

/// The cache in `dir` with `budget` bytes, opened once per process (craft's caches race when one
/// folder is opened twice); a cache opened before with another budget takes this one, evicting down to it.
fn open_cache(dir: &Path, budget: u64) -> Result<OriginalsCache, String> {
    static OPEN: Mutex<Vec<(PathBuf, OriginalsCache, u64)>> = Mutex::new(Vec::new());
    let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, cache, had)) = open.iter_mut().find(|(d, _, _)| d == dir) {
        if *had != budget {
            cache.set_budget(budget);
            *had = budget;
        }
        return Ok(cache.clone());
    }
    let cache = OriginalsCache::open(dir, budget).map_err(|e| e.to_string())?;
    open.push((dir.to_path_buf(), cache.clone(), budget));
    Ok(cache)
}

/// `26 MB`: decimal megabytes, at least 1.
fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes.saturating_add(500_000) / 1_000_000).max(1))
}

/// A request that was skipped because the account's server didn't answer a moment ago.
fn offline() -> String {
    Error::Unreachable("it didn't answer in the last 30 s".into()).to_string()
}

impl Remote {
    /// The originals cache in the configured folder, with the budget from the settings (opened on first use).
    pub(crate) fn cache(&self) -> Result<OriginalsCache, String> {
        let (dir, mb) = {
            let inner = self.lock();
            (inner.config.cache_dir.clone(), inner.settings.cache_budget_mb)
        };
        let dir = dir.ok_or("This computer has no folder for downloaded Nextcloud originals")?;
        open_cache(&dir, budget_bytes(mb))
    }

    /// A local copy of the original at `path`: a cached one, else (interactive work only, see
    /// [`crate::memory::may_download`]) downloaded, one at a time, with a cancellable `download`
    /// activity row. Background work gets [`NOT_DOWNLOADED`] instead. While the server doesn't answer, an
    /// interactive load gets the newest copy on disk (it may be out of date).
    pub fn local_original(&self, path: &str) -> Result<PathBuf, String> {
        let rp = RemotePath::parse(path).map_err(|e| e.to_string())?;
        let key = rp.account_key();
        let cache = self.cache()?;
        let (known, state) = {
            let inner = self.lock();
            (inner.known.get(&rp).cloned(), inner.state(&key))
        };
        if let Some((etag, _)) = &known
            && let Some(local) = cache.cached(&rp, etag)
        {
            return Ok(local);
        }
        let client = self.client(&key)?;
        let mut size = known.map(|(_, size)| size);
        let may_download = crate::memory::may_download();
        // ponytail: one `stat` to learn the etag `cached` needs; a "newest copy" accessor in craft would save it
        if size.is_none() && state != AccountState::Unreachable {
            match self.stat(&client, &rp) {
                Ok(entry) => {
                    if let Some(local) = cache.cached(&rp, &entry.etag) {
                        return Ok(local);
                    }
                    size = Some(entry.size);
                }
                // `fetch` serves the newest copy on disk for these (as it says why it couldn't ask)
                Err(Error::Unreachable(_) | Error::RateLimited | Error::CertificateChanged | Error::UnknownCertificate { .. }) if may_download => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if !may_download {
            return Err(if state == AccountState::Unreachable { offline() } else { NOT_DOWNLOADED.into() });
        }
        // ponytail: one download at a time; track downloads per path if a loupe waiting behind another shows up
        let _one = self.downloads.lock().unwrap_or_else(PoisonError::into_inner);
        let name = rp.name().unwrap_or_default();
        let label = match size {
            Some(size) => format!("Downloading {name} ({})", megabytes(size)),
            None => format!("Downloading {name}"),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let task = self.activity.start("download", &label, Cancel::Flag(cancel.clone()));
        task.set_unit(Unit::Bytes);
        let total = size.unwrap_or(0);
        match cache.fetch(&client, &rp, &cancel, &mut |done, of| task.progress(done, of.unwrap_or(total))) {
            Ok(fetched) => Ok(fetched.path),
            Err(e) => Err(self.failed(&rp, &e)),
        }
    }

    /// Whether the photo at `path` counts as there: its account is on this computer and signed in, its
    /// server answered in the last 30 s (asked again with a `stat` of the root otherwise), the server
    /// didn't say the file is gone, and a listing of its folder this session (if any) has it.
    pub fn exists(&self, path: &str) -> bool {
        let Ok(rp) = RemotePath::parse(path) else { return false };
        let key = rp.account_key();
        let root = {
            let inner = self.lock();
            let Some(account) = inner.account(&key) else { return false };
            let listed = rp.parent().and_then(|dir| inner.listed.get(&dir));
            if inner.gone.contains(&rp) || listed.is_some_and(|names| !rp.name().is_some_and(|n| names.contains(n))) {
                return false;
            }
            match inner.states.get(&key) {
                Some((AccountState::SignedIn, at)) if at.elapsed() < REACHABLE_FOR => return true,
                _ if inner.state(&key) != AccountState::SignedIn => return false,
                _ => account.root(),
            }
        };
        let Ok(client) = self.client(&key) else { return false };
        match client.stat(&root) {
            Ok(_) => {
                self.note_ok(&key);
                true
            }
            Err(e) => {
                self.note_error(&key, &e);
                false
            }
        }
    }

    /// What a listing of `dir` says: its files' etags and sizes, which names are there (see
    /// [`Remote::exists`]), and that a listed file is no longer gone.
    #[allow(dead_code)] // the import (NC6) lists folders
    pub(crate) fn note_listing(&self, dir: &RemotePath, entries: &[Entry]) {
        let mut inner = self.lock();
        let mut names = HashSet::new();
        for e in entries {
            names.insert(e.name.clone());
            let Ok(path) = dir.join(&e.name) else { continue };
            inner.gone.remove(&path);
            if !e.is_dir {
                inner.known.insert(path, (e.etag.clone(), e.size));
            }
        }
        inner.listed.insert(dir.clone(), names);
    }

    /// `stat` of `path`, remembered (etag, size; not gone); a failure is noted ([`Remote::failed`]).
    fn stat(&self, client: &Client, path: &RemotePath) -> Result<Entry, Error> {
        match client.stat(path) {
            Ok(entry) => {
                self.note_ok(&path.account_key());
                let mut inner = self.lock();
                inner.gone.remove(path);
                inner.known.insert(path.clone(), (entry.etag.clone(), entry.size));
                Ok(entry)
            }
            Err(e) => {
                self.failed(path, &e);
                Err(e)
            }
        }
    }

    /// The etag and size of `path`: remembered, else asked (not while its server doesn't answer).
    pub(crate) fn etag_and_size(&self, path: &RemotePath) -> Result<(String, u64), String> {
        let key = path.account_key();
        let (known, state) = {
            let inner = self.lock();
            (inner.known.get(path).cloned(), inner.state(&key))
        };
        if let Some(known) = known {
            return Ok(known);
        }
        let client = self.client(&key)?;
        if state == AccountState::Unreachable {
            return Err(offline());
        }
        self.stat(&client, path).map(|e| (e.etag, e.size)).map_err(|e| e.to_string())
    }

    /// The server's preview of `path` (`None`: it has none), unless its server didn't answer a moment ago.
    pub(crate) fn server_preview(&self, path: &RemotePath, max_edge: u32) -> Result<Option<Vec<u8>>, String> {
        let key = path.account_key();
        let client = self.client(&key)?;
        if self.lock().state(&key) == AccountState::Unreachable {
            return Err(offline());
        }
        client.preview(path, max_edge).map_err(|e| self.failed(path, &e))
    }

    /// The first `len` bytes of `path`, unless its server didn't answer a moment ago.
    pub(crate) fn read_head(&self, path: &RemotePath, len: u64) -> Result<Vec<u8>, String> {
        let key = path.account_key();
        let client = self.client(&key)?;
        if self.lock().state(&key) == AccountState::Unreachable {
            return Err(offline());
        }
        client.read_range(path, 0..len).map_err(|e| self.failed(path, &e))
    }

    /// A request about `path` failed: the account's state follows ([`Remote::note_error`]), a file the
    /// server no longer has is gone (until a listing has it again). Returns the message for the user.
    fn failed(&self, path: &RemotePath, e: &Error) -> String {
        self.note_error(&path.account_key(), e);
        if matches!(e, Error::NotFound) {
            self.lock().gone.insert(path.clone());
        }
        e.to_string()
    }

    /// The server answered: it is reachable (for [`REACHABLE_FOR`]); a signed-out account stays signed out.
    fn note_ok(&self, key: &str) {
        let mut inner = self.lock();
        if matches!(inner.state(key), AccountState::SignedIn | AccountState::Unreachable) {
            inner.states.insert(key.to_string(), (AccountState::SignedIn, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R14: a cache already open takes the budget it is asked for later (a reconfigure, a changed setting).
    #[test]
    fn an_open_cache_takes_a_new_budget() {
        let dir = std::env::temp_dir().join(format!("lc-nextcloud-budget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // a copy as craft names them: <account hash>/<path hash>-<etag hash>.<ext>
        let copy = dir.join("0123456789abcdef").join(format!("{}-{}.jpg", "a".repeat(64), "b".repeat(16)));
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::write(&copy, [0u8; 100]).unwrap();
        assert_eq!(open_cache(&dir, 1000).unwrap().usage().total, 100);
        assert_eq!(open_cache(&dir, 10).unwrap().usage().total, 0, "evicted down to the new budget");
        assert!(!copy.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
