//! Remote originals on this computer: the [`OriginalsCache`] (one per folder in the process), the
//! etags and sizes listings and `stat`s told us (with none, the cache's newest copy of a file is the
//! one used, so copies are found while the server can't be asked), which files the server no longer
//! has, and whether a remote photo counts as there ([`Remote::exists`]). Downloads (interactive,
//! prefetched, made available offline) and what a finished one tells the catalog ([`complete`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use craft_nextcloud::{Client, Entry, Error, OriginalsCache, RemotePath};
use lightcraft_catalog::{Op, Source};
use serde_json::json;

use super::{AccountState, Event, REACHABLE_FOR, Remote};
use crate::Session;
use crate::activity::{Cancel, TaskGuard, Unit};
use crate::media::ProbeInfo;

/// Work that doesn't download (grid thumbnails, batch jobs, the app's UI thread) found no local copy
/// of the original.
pub const NOT_DOWNLOADED: &str = "The original isn't downloaded yet; open it in Develop to download it";

/// The smallest disk budget for downloaded originals, in MB.
pub(crate) const MIN_CACHE_BUDGET_MB: u64 = 1000;

/// `mb` (at least [`MIN_CACHE_BUDGET_MB`]) in bytes.
pub(crate) fn budget_bytes(mb: u64) -> u64 {
    mb.max(MIN_CACHE_BUDGET_MB).saturating_mul(1_000_000)
}

/// Why the originals cache can't be used: another process has it ([`open_cache`]).
pub(crate) const CACHE_IN_USE: &str = "Another LightCraft window or lightcraft-cli is using the Nextcloud download cache; close it and try again";

/// The cache in `dir` with `budget` bytes, opened once per process (a second open of one folder
/// fails: craft locks `<dir>/.lock` while the cache is open). While another process has it, the
/// cache isn't available here ([`CACHE_IN_USE`]; asked again on the next use). A cache opened before
/// with another budget takes this one, evicting down to it.
fn open_cache(dir: &Path, budget: u64) -> Result<OriginalsCache, String> {
    static OPEN: Mutex<Vec<(PathBuf, OriginalsCache, u64)>> = Mutex::new(Vec::new());
    let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, cache, had)) = open.iter_mut().find(|(d, ..)| d == dir) {
        if *had != budget {
            cache.set_budget(budget);
            *had = budget;
        }
        return Ok(cache.clone());
    }
    let cache = OriginalsCache::open(dir, budget).map_err(|e| match e {
        Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock => CACHE_IN_USE.to_string(),
        e => format!("The Nextcloud download cache {} can't be used: {e}", dir.display()),
    })?;
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

/// Who a download is for ([`Remote::original`]).
#[derive(Clone, Copy)]
enum Want<'a> {
    /// An interactive load: a `Downloading` row of its own, cancelled from there.
    Load,
    /// [`Remote::prefetch`]: a `Prefetching` row, cancelled by the next prefetch, or, once its photo
    /// is the one open ([`Remote::hold`]), by moving on from it.
    Prefetch(&'a Arc<AtomicBool>),
    /// [`Remote::make_available_offline`]: a `Downloading` row, cancelled with the command's.
    Offline(&'a Arc<AtomicBool>),
    // (neither of the last two asks a server that didn't answer a moment ago: nobody waits for them)
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
    /// activity row (cancelled too by the [`crate::memory::cancellable`] flag of the work asking).
    /// Background work gets [`NOT_DOWNLOADED`] instead. While the server doesn't answer (since a
    /// moment ago), nothing waits for it: the copy on disk, which may be out of date, else the offline
    /// error at once.
    pub fn local_original(&self, path: &str) -> Result<PathBuf, String> {
        self.original(path, Want::Load)
    }

    /// [`Remote::local_original`] for `want`. A real download (not a copy on disk, not a stale one)
    /// is described here, off the session's thread, and queued as [`Event::Downloaded`], which
    /// completes the photo's record.
    fn original(&self, path: &str, want: Want<'_>) -> Result<PathBuf, String> {
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
        if state == AccountState::Unreachable {
            return self.cached_copy(&rp).ok_or_else(offline);
        }
        let cancel = match want {
            // (a render that moved on to another photo stops its download)
            Want::Load => crate::memory::cancel_flag().unwrap_or_else(|| Arc::new(AtomicBool::new(false))),
            Want::Prefetch(cancel) | Want::Offline(cancel) => cancel.clone(),
        };
        let client = self.client(&key)?;
        let mut size = known.map(|(_, size)| size);
        let may_download = crate::memory::may_download();
        // ponytail: one `stat` to learn the etag `cached` needs; a "newest copy" accessor in craft would save it
        if size.is_none() && state != AccountState::Unreachable {
            // (moved on already: not even a `stat`)
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled.to_string());
            }
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
        // (that `stat`, or another request meanwhile, found the server not answering: not asked again)
        if self.lock().state(&key) == AccountState::Unreachable {
            return self.cached_copy(&rp).ok_or_else(offline);
        }
        if !may_download {
            return Err(NOT_DOWNLOADED.into());
        }
        // (the lock goes once the bytes are here; the row once the copy is described too)
        let (fetched, downloaded, _task) = {
            let _one = self.download_turn(want, &cancel)?;
            // (another download brought it meanwhile: the prefetch of the photo just opened)
            if let Some(local) = self.cached_copy(&rp) {
                return Ok(local);
            }
            let name = rp.name().unwrap_or_default();
            let label = match (want, size) {
                (Want::Prefetch(_), _) => format!("Prefetching {name}"),
                (_, Some(size)) => format!("Downloading {name} ({})", megabytes(size)),
                (_, None) => format!("Downloading {name}"),
            };
            let task = self.activity.start("download", &label, Cancel::Flag(cancel.clone()));
            task.set_unit(Unit::Bytes);
            #[cfg(test)]
            self.download_step();
            // (cancelled since it took its turn: not even a `stat`)
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled.to_string());
            }
            let total = size.unwrap_or(0);
            // (bytes came: neither a copy on disk nor a stale one)
            let mut downloaded = false;
            let fetched = cache.fetch(&client, &rp, &cancel, &mut |done, of| {
                downloaded = true;
                task.progress(done, of.unwrap_or(total));
            });
            (fetched, downloaded, task)
        };
        match fetched {
            Ok(fetched) => {
                if downloaded {
                    // (the next download needn't wait for this)
                    let info = describe_copy(&cache, &rp, &fetched.path, want).map_err(|e| log::warn!("Nextcloud: {path}: {e}")).ok();
                    self.push_event(Event::Downloaded { path: path.to_string(), info: info.map(Box::new) });
                }
                Ok(fetched.path)
            }
            // a cancelled download says nothing about the server (a stalled one ends in its error)
            Err(_) if cancel.load(Ordering::Relaxed) => Err(Error::Cancelled.to_string()),
            Err(e) => Err(self.failed(&rp, &e)),
        }
    }

    /// The download lock, for `want`: an interactive load waits its turn; a prefetch or an offline
    /// copy gives the lock up while a load waits for it, and stops once cancelled.
    fn download_turn(&self, want: Want<'_>, cancel: &AtomicBool) -> Result<MutexGuard<'_, ()>, String> {
        // ponytail: one download at a time; track downloads per path if a loupe waiting behind another shows up
        let lock = || self.downloads.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(want, Want::Load) {
            let _waiting = Waiting::new(&self.loads_waiting);
            let one = lock();
            // (moved on while another download ran: no row, no request)
            return if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled.to_string()) } else { Ok(one) };
        }
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled.to_string());
            }
            let one = lock();
            if self.loads_waiting.load(Ordering::SeqCst) == 0 {
                // (moved on while another download ran)
                return if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled.to_string()) } else { Ok(one) };
            }
            drop(one);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Tests: wait for the next message on [`Remote::download_steps`], if set (at most 10 s).
    #[cfg(test)]
    fn download_step(&self) {
        if let Some(steps) = &*self.download_steps.lock().unwrap_or_else(PoisonError::into_inner) {
            let _ = steps.recv_timeout(std::time::Duration::from_secs(10));
        }
    }

    /// The copy of `path` on this computer, no request: at the etag last seen, else (none seen yet, as
    /// after a restart) the cache's newest one, which may be out of date. A load holds `path` first
    /// (`hooks::with_original`), so the copy stays on disk until it is read.
    fn cached_copy(&self, path: &RemotePath) -> Option<PathBuf> {
        let known = self.lock().known.get(path).map(|(etag, _)| etag.clone());
        let cache = self.cache().ok()?;
        match known {
            Some(etag) => cache.cached(path, &etag),
            None => cache.newest(path),
        }
    }

    /// Whether the original of `path` is on this computer ([`Remote::cached_copy`]); no request.
    pub fn has_local_copy(&self, path: &str) -> bool {
        RemotePath::parse(path).ok().and_then(|rp| self.cached_copy(&rp)).is_some()
    }

    /// Keep the original of `path` (the photo open in Develop) on disk until the next call; `None`, or
    /// a photo that isn't on Nextcloud, keeps none. The same path again changes nothing. A prefetch
    /// of `path` running goes on: the next prefetch doesn't cancel it (a load of `path` waits for it
    /// and finds its copy instead of downloading it again), but moving on to another photo, or to
    /// `None`, does.
    pub fn hold(&self, path: Option<&str>) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let adopted = path.and_then(|p| {
            let mut running = self.prefetching.lock().unwrap_or_else(PoisonError::into_inner);
            if running.as_ref().is_some_and(|(r, _)| r == p) { running.take().map(|(_, cancel)| cancel) } else { None }
        });
        if let Some(open) = held.as_mut().filter(|(p, _, _)| Some(p.as_str()) == path) {
            // (prefetched again while open: the first prefetch goes on, this one stops)
            match (&open.2, adopted) {
                (Some(_), Some(again)) => again.store(true, Ordering::Relaxed),
                (None, again) => open.2 = again,
                (Some(_), None) => {}
            }
            return;
        }
        if let Some((_, _, Some(cancel))) = held.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let open = path.and_then(|p| {
            let rp = RemotePath::parse(p).ok()?;
            Some((p.to_string(), self.cache().ok()?.hold(&rp)))
        });
        match open {
            Some((p, hold)) => *held = Some((p, hold, adopted)),
            None => {
                if let Some(cancel) = adopted {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// Download the original of `path` (the photo Develop shows next) on a thread of its own, behind
    /// a cancellable `Prefetching {name}` row, one at a time with every other download and after the
    /// interactive loads waiting for one. Another path, or `None`, cancels the one running (unless it
    /// became the photo open, see [`Remote::hold`]); the one last asked for, or a photo whose original
    /// is on this computer, is left alone. A server that didn't answer a moment ago isn't asked.
    ///
    /// Callers call `hold(open)` before `prefetch(next)`: the other order cancels the open photo's
    /// prefetch before `hold` can keep it.
    pub fn prefetch(self: &Arc<Self>, path: Option<&str>) {
        let mut running = self.prefetching.lock().unwrap_or_else(PoisonError::into_inner);
        if running.as_ref().is_some_and(|(p, _)| Some(p.as_str()) == path) {
            return;
        }
        if let Some((_, cancel)) = running.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let Some((path, rp)) = path.and_then(|p| Some((p, RemotePath::parse(p).ok()?))) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        // (remembered when it's here too: asked again, every frame, it isn't looked for again)
        *running = Some((path.to_string(), cancel.clone()));
        if self.cached_copy(&rp).is_some() {
            return;
        }
        drop(running);
        let (remote, path) = (self.clone(), path.to_string());
        let spawned = std::thread::Builder::new().name("nextcloud-prefetch".into()).spawn(move || {
            // last-resort guard: a prefetch never takes the app down
            match crate::guard::catch("Prefetching a Nextcloud original", || remote.original(&path, Want::Prefetch(&cancel))) {
                Ok(Ok(_)) => {}
                Ok(Err(e)) | Err(e) => log::debug!("Nextcloud prefetch of {path}: {e}"),
            }
        });
        if let Err(e) = spawned {
            log::warn!("could not start a Nextcloud prefetch: {e}");
        }
    }

    /// "Make Available Offline": pin each of `paths`, then download it unless it is on this computer
    /// (pinned first, so the run's own downloads can't evict the copies it made). A photo that fails
    /// doesn't stop the rest (nor waits on a server that didn't answer a moment ago); `cancel` (the
    /// command's row, or the download's own) stops the run.
    /// Progress (photos done of all) goes to `row`. Returns (photos on this computer and pinned,
    /// photos that failed).
    pub(crate) fn make_available_offline(&self, paths: &[String], cancel: &Arc<AtomicBool>, row: &TaskGuard) -> (usize, usize) {
        let (mut made, mut failed) = (0, 0);
        for (i, path) in paths.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let pinned = RemotePath::parse(path).map_err(|e| e.to_string()).and_then(|rp| {
                row.detail(rp.name().unwrap_or_default());
                self.cache()?.pin(&rp).map_err(|e| e.to_string())
            });
            match pinned.and_then(|()| self.original(path, Want::Offline(cancel))) {
                Ok(_) => made += 1,
                Err(_) if cancel.load(Ordering::Relaxed) => break,
                Err(e) => {
                    log::warn!("Make Available Offline: {path}: {e}");
                    failed += 1;
                }
            }
            row.progress(i.saturating_add(1) as u64, paths.len() as u64);
        }
        (made, failed)
    }

    /// Whether the photo at `path` counts as there: its account is on this computer and signed in, its
    /// server answered in the last 30 s (asked again with a `stat` of the root otherwise) or, while it
    /// doesn't answer, its original is on this computer, the server didn't say the file is gone, and a
    /// listing of its folder this session (if any) has it.
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
                _ if inner.state(&key) == AccountState::Unreachable => None,
                _ if inner.state(&key) != AccountState::SignedIn => return false,
                _ => Some(account.root()),
            }
        };
        // (offline editing goes on with the original, not the smart preview)
        let Some(root) = root else { return self.has_local_copy(path) };
        let Ok(client) = self.client(&key) else { return false };
        match client.stat(&root) {
            Ok(_) => {
                self.note_ok(&key);
                true
            }
            Err(e) => {
                self.note_error(&key, &e);
                let unreachable = self.lock().state(&key) == AccountState::Unreachable;
                unreachable && self.has_local_copy(path)
            }
        }
    }

    /// What a listing of `dir` says: its files' etags and sizes, which names are there (see
    /// [`Remote::exists`]), and that a listed file is no longer gone.
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

    /// The entries of folder `dir`, remembered ([`Remote::note_listing`]), unless its server didn't
    /// answer a moment ago.
    pub(crate) fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, String> {
        let key = dir.account_key();
        let client = self.client(&key)?;
        self.reachable(&key)?;
        match client.list(dir) {
            Ok(entries) => {
                self.note_ok(&key);
                self.note_listing(dir, &entries);
                Ok(entries)
            }
            Err(e) => Err(self.failed(dir, &e)),
        }
    }

    /// An error (the offline text) while account `key`'s server is taken as not answering.
    pub(crate) fn reachable(&self, key: &str) -> Result<(), String> {
        if self.lock().state(key) == AccountState::Unreachable { Err(offline()) } else { Ok(()) }
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

    /// The start of the file at `path` that holds its tags (1 MiB, as the import reads), for All
    /// Metadata of a photo whose original isn't on this computer.
    pub(crate) fn read_header(&self, path: &str) -> Result<Vec<u8>, String> {
        self.read_head(&RemotePath::parse(path).map_err(|e| e.to_string())?, super::import::META_WINDOW)
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

/// Counts an interactive load waiting for the download lock while it lives
/// ([`Remote::loads_waiting`]): no path out of the wait, unwinding included, leaves it counted
/// (which would keep prefetches and offline copies waiting for ever).
struct Waiting<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> Waiting<'a> {
    fn new(count: &'a std::sync::atomic::AtomicUsize) -> Waiting<'a> {
        count.fetch_add(1, Ordering::SeqCst);
        Waiting(count)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What the whole downloaded copy `local` of `path` says about it: the import's probe with the
/// remote name (the copy's extension is the cache's) and no content hash (keys stay
/// `file:{path}:{size}`). On the downloading thread, the copy held from eviction meanwhile.
fn describe_copy(cache: &OriginalsCache, path: &RemotePath, local: &Path, want: Want<'_>) -> Result<ProbeInfo, String> {
    let _hold = cache.hold(path);
    let len = std::fs::metadata(local).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
    let gate = crate::memory::work_gate();
    // (a loupe waits for it: counted, never waiting; a prefetch or an offline copy waits its turn)
    let _permit = if matches!(want, Want::Load) { gate.acquire_urgent(len) } else { gate.acquire(len) };
    let bytes = std::fs::read(local).map_err(|e| format!("{}: {e}", local.display()))?;
    crate::files::probe_unhashed(path.name().unwrap_or_default(), &bytes)
}

/// Complete the photos whose originals were `downloaded` (path, what the whole file says, read by
/// the downloading thread): dimensions, the camera fields they lack, the capture time the file
/// records (for a photo still on the listing's estimate), lens data and as-shot white balance, as Reload does, but not as an undo step. Their
/// decoded thumbnails go (one may be the server's preview standing in, see `hooks`; a bigger source,
/// decoded from the original, stays), and their smart previews are built in the background when the
/// library has a folder for them (from the copy on disk: that build never downloads). No file is
/// read here.
pub(crate) fn complete(s: &mut Session, downloaded: Vec<(String, Option<Box<ProbeInfo>>)>) {
    let infos: HashMap<String, Option<Box<ProbeInfo>>> = downloaded.into_iter().collect();
    let photos: Vec<_> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            Source::File { path } if infos.contains_key(path) => Some((p.id, path.clone())),
            _ => None,
        })
        .collect();
    for (id, path) in &photos {
        let id = *id;
        s.media.forget_thumb(id);
        let (Some(ph), Some(Some(info))) = (s.catalog.photo(id), infos.get(path)) else { continue };
        let info: &ProbeInfo = info;
        // (a virtual copy's camera fields are its own, as in Reload; the file's capture time replaces
        // only a listing's estimate: a photo the metadata phase read (it has a camera) keeps its
        // time, which may be the user's)
        let meta = if ph.copy_of.is_some() {
            Vec::new()
        } else if ph.meta.camera.is_empty() {
            super::import::meta_ops(ph, info)
        } else {
            crate::cmd::convert::fill_missing_meta(id, ph, info)
        };
        let ops: Vec<Op> = crate::cmd::convert::content_op(id, ph, info.clone())
            .into_iter()
            .chain(meta)
            .chain(crate::cmd::convert::lens_ops(id, ph, info))
            .chain(crate::cmd::convert::as_shot_wb_op(id, ph, info))
            .collect();
        if !ops.is_empty()
            && let Err(e) = s.apply_system(Op::Batch { ops })
        {
            log::warn!("Nextcloud: completing photo {}: {e}", id.0);
        }
    }
    if !photos.is_empty() && s.media.smart_dir.is_some() {
        let p = json!({"ids": photos.iter().map(|(id, _)| id.0).collect::<Vec<_>>(), "background": true});
        if let Err(e) = crate::cmd::previews::smart(s, &p) {
            log::warn!("Nextcloud: smart previews of downloaded originals: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// N2: a load that stops waiting, however it stops, is no longer counted.
    #[test]
    fn a_waiting_load_is_uncounted_when_it_unwinds() {
        let count = std::sync::atomic::AtomicUsize::new(0);
        let r = std::panic::catch_unwind(|| {
            let _waiting = Waiting::new(&count);
            assert_eq!(count.load(Ordering::SeqCst), 1);
            panic!("the wait ends badly");
        });
        assert!(r.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

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
