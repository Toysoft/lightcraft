//! Looking into Nextcloud folders (`nextcloud.browse`) and importing them (`library.importNextcloud`).
//!
//! An import runs on a worker thread, behind activity rows. It lists the folders, 8 at a time, and
//! sends the photos it finds as [`Event::Found`] chunks: size, kind and format from the listing,
//! dimensions and capture time from the Memories app when the server has it. Then it reads the
//! metadata of the photos that had no camera yet, from Memories' EXIF or else the start of the
//! file, as [`Event::Meta`] chunks. No original is downloaded, and nothing reaches the catalog off
//! the session: [`crate::Session::drain_remote`] applies the events, into the library the import
//! started in only. How each import went is kept for the UI ([`Remote::imports`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use craft_nextcloud::{Entry, Memories, MemoriesPhoto, RemoteMeta, RemotePath};
use lightcraft_catalog::{MediaKind, Op, PhotoId, Source, is_remote};
use serde_json::{Value, json};

use super::{Event, ImportOutcome, ImportProgress, MetaRead, Remote, hooks};
use crate::activity::{Cancel, TaskGuard};
use crate::media::ProbeInfo;
use crate::{EngineError, Session};

/// Folders listed, and photos' metadata read, at once.
const WORKERS: usize = 8;
/// Bytes first read from the start of a photo for its metadata. Measured on real files (NC5), the
/// first 64 KiB of an ARW (and a JPEG's EXIF) hold its camera, lens, exposure and capture time.
const FIRST_WINDOW: u64 = 64 << 10;
/// Read when the first window had no camera: a CR3's metadata lies within its first 512 KiB.
pub(crate) const META_WINDOW: u64 = 1 << 20;
/// Photos per event: a drain costs a pass over the catalog, so they come in chunks of this many…
const CHUNK: usize = 500;
/// …or once the first of a chunk has waited this long.
const CHUNK_WAIT: Duration = Duration::from_millis(250);
/// Imports whose outcome is kept ([`Remote::imports`]).
pub(crate) const MAX_IMPORTS: usize = 8;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `nextcloud.browse`: the folders in `dir` (by name) and how many photos it holds itself.
pub(crate) fn browse(remote: &Remote, dir: &RemotePath) -> Result<Value, String> {
    let entries = remote.list(dir)?;
    let mut folders: Vec<(String, String)> =
        entries.iter().filter(|e| e.is_dir && !hidden(e)).filter_map(|e| Some((e.name.clone(), dir.join(&e.name).ok()?.to_string()))).collect();
    folders.sort_by_cached_key(|(name, _)| name.to_lowercase());
    let photos = entries.iter().filter(|e| !e.is_dir && !hidden(e) && photo(e)).count();
    let folders: Vec<Value> = folders.into_iter().map(|(name, path)| json!({"name": name, "path": path})).collect();
    Ok(json!({"dir": dir.to_string(), "folders": folders, "photos": photos}))
}

/// Left out, like the hidden files of a local import (e.g. `._IMG_1.JPG` from a Mac).
fn hidden(e: &Entry) -> bool {
    e.name.starts_with('.')
}

fn photo(e: &Entry) -> bool {
    crate::import::is_supported(Path::new(&e.name))
}

/// An import as [`Remote`] keeps it: its outcome, how to cancel it, and the undo step its photos go into.
pub(crate) struct ImportRecord {
    outcome: ImportOutcome,
    cancel: Arc<AtomicBool>,
    /// [`Session::commits`] and the undo stack's length right after this import's last step: while
    /// they still hold, nothing was done since, and the next batch joins that step.
    undo: Option<(u64, usize)>,
}

impl Remote {
    /// How the last 8 imports went (or stand), oldest first.
    pub fn imports(&self) -> Vec<ImportOutcome> {
        lock(&self.imports).iter().map(|r| r.outcome.clone()).collect()
    }

    /// Keep a running import's outcome; past [`MAX_IMPORTS`] the oldest finished one is forgotten.
    pub(crate) fn begin_import(&self, task: u64, cancel: Arc<AtomicBool>) {
        let mut all = lock(&self.imports);
        let outcome = ImportOutcome { task, state: ImportProgress::Running, added: 0, failed_folders: 0, error: None };
        all.push_back(ImportRecord { outcome, cancel, undo: None });
        while all.len() > MAX_IMPORTS {
            let oldest = all.iter().position(|r| r.outcome.state != ImportProgress::Running).unwrap_or(0);
            all.remove(oldest);
        }
    }

    /// `f` on import `task`'s record, if it is still kept.
    fn update_import(&self, task: u64, f: impl FnOnce(&mut ImportRecord)) {
        if let Some(r) = lock(&self.imports).iter_mut().find(|r| r.outcome.task == task) {
            f(r);
        }
    }

    /// Another library opened: running imports stop (what they still send is dropped, see [`Event`]).
    pub(crate) fn library_changed(&self) {
        for r in lock(&self.imports).iter().filter(|r| r.outcome.state == ImportProgress::Running) {
            r.cancel.store(true, Relaxed);
        }
    }

    /// Tests: wait for the next message on [`Remote::import_steps`], if set (at most 10 s).
    #[cfg(test)]
    fn import_step(&self) {
        if let Some(steps) = &*lock(&self.import_steps) {
            let _ = steps.recv_timeout(Duration::from_secs(10));
        }
    }
}

/// One import, as its worker sees it.
struct Job {
    remote: Arc<Remote>,
    /// Its activity row's id.
    task: u64,
    /// The library it imports into ([`Session::library_generation`]).
    generation: u64,
    account: String,
    dirs: Vec<RemotePath>,
    /// Remote photos that had a camera when the import started: their metadata isn't read again.
    has_camera: HashSet<String>,
    cancel: Arc<AtomicBool>,
    failed_folders: AtomicUsize,
}

impl Job {
    /// An error when the account can't be asked any more: removed, signed out, or its server not answering.
    fn account_ok(&self) -> Result<(), String> {
        self.remote.client(&self.account)?;
        self.remote.reachable(&self.account)
    }

    fn found(&self, photos: Vec<(String, ProbeInfo)>) {
        self.remote.push_event(Event::Found { task: self.task, generation: self.generation, photos });
    }

    fn read(&self, reads: Vec<MetaRead>) {
        self.remote.push_event(Event::Meta { task: self.task, generation: self.generation, reads });
    }

    /// Note how the import ended.
    fn finish(&self, ended: &Result<(), String>) {
        let (state, error) = match ended {
            Err(e) => {
                log::warn!("Nextcloud import: {e}");
                (ImportProgress::Failed, Some(e.clone()))
            }
            Ok(()) if self.cancel.load(Relaxed) => (ImportProgress::Cancelled, None),
            Ok(()) => (ImportProgress::Done, None),
        };
        let failed_folders = self.failed_folders.load(Relaxed);
        self.remote.update_import(self.task, |r| {
            r.outcome.state = state;
            r.outcome.failed_folders = failed_folders;
            r.outcome.error = error;
        });
    }
}

/// Items gathered for one event: taken once there are [`CHUNK`], or once the first has waited [`CHUNK_WAIT`].
struct Chunk<T> {
    items: Vec<T>,
    first: Option<Instant>,
}

impl<T> Default for Chunk<T> {
    fn default() -> Self {
        Chunk { items: Vec::new(), first: None }
    }
}

impl<T> Chunk<T> {
    fn push(&mut self, item: T) {
        self.first.get_or_insert_with(Instant::now);
        self.items.push(item);
    }

    /// The items when they are due (any there are with `all`).
    fn take(&mut self, all: bool) -> Option<Vec<T>> {
        let due = self.items.len() >= CHUNK || self.first.is_some_and(|t| t.elapsed() >= CHUNK_WAIT);
        if self.items.is_empty() || !(all || due) {
            return None;
        }
        self.first = None;
        Some(std::mem::take(&mut self.items))
    }
}

/// A photo whose metadata the import reads once the folders are listed.
struct Todo {
    path: RemotePath,
    /// On the Memories timeline.
    fileid: Option<u64>,
}

/// `library.importNextcloud`: import the photos in `dirs` (folders of account `account`, which this
/// computer has) and their subfolders. Returns `{"started": true, "task"}` at once; with `wait`,
/// applies the events as they come and returns `{"added", "skipped", "metadata", "failedFolders"}`
/// once the import is done.
pub(crate) fn start(s: &mut Session, account: String, dirs: Vec<RemotePath>, wait: bool) -> crate::Result<Value> {
    // an account that isn't here or must sign in again fails now, not in the worker
    s.remote.client(&account).map_err(EngineError::Other)?;
    let has_camera = s
        .catalog
        .photos()
        .filter(|p| !p.meta.camera.is_empty())
        .filter_map(|p| match &p.source {
            Source::File { path } if is_remote(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    let cancel = Arc::new(AtomicBool::new(false));
    let row = s.activity.start("import", "Importing from Nextcloud", Cancel::Flag(cancel.clone()));
    let task = row.id();
    s.remote.begin_import(task, cancel.clone());
    let job = Job {
        remote: s.remote.clone(),
        task,
        generation: s.library_generation(),
        account,
        dirs,
        has_camera,
        cancel,
        failed_folders: AtomicUsize::new(0),
    };
    let spawned = std::thread::Builder::new().name("nextcloud-import".into()).spawn(move || {
        // last-resort guard: a panic ends this import, with its rows
        let ended = crate::guard::catch("The Nextcloud import", || run(&job, row)).and_then(|r| r);
        job.finish(&ended);
        ended
    });
    let worker = match spawned {
        Ok(w) => w,
        Err(e) => {
            let e = format!("could not start the import: {e}");
            s.remote.update_import(task, |r| {
                r.outcome.state = ImportProgress::Failed;
                r.outcome.error = Some(e.clone());
            });
            return Err(EngineError::Other(e));
        }
    };
    if !wait {
        return Ok(json!({"started": true, "task": task}));
    }
    let mut tally = Tally::default();
    loop {
        // (what the worker sent before it ended is applied once more after)
        let finished = worker.is_finished();
        apply_events(s, &mut tally);
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.join().map_err(|_| EngineError::Other("the Nextcloud import stopped unexpectedly".into()))?.map_err(EngineError::Other)?;
    let failed_folders = s.remote.imports().iter().find(|o| o.task == task).map_or(0, |o| o.failed_folders);
    Ok(json!({"added": tally.added, "skipped": tally.skipped, "metadata": tally.metadata, "failedFolders": failed_folders}))
}

/// The worker: list, then read metadata. `row` is the import's row.
fn run(job: &Job, row: TaskGuard) -> Result<(), String> {
    #[cfg(test)]
    job.remote.import_step();
    let client = job.remote.client(&job.account)?;
    job.remote.reachable(&job.account)?;
    let memories = client.memories().unwrap_or_else(|e| {
        job.remote.note_error(&job.account, &e);
        None
    });
    let timeline: HashMap<u64, MemoriesPhoto> = match memories.map(|m| m.days()) {
        Some(Ok(days)) => days.into_iter().map(|d| (d.fileid, d)).collect(),
        Some(Err(e)) => {
            log::warn!("Nextcloud import: the Memories timeline: {e}");
            job.remote.note_error(&job.account, &e);
            HashMap::new()
        }
        None => HashMap::new(),
    };
    let todo = walk(job, &row, &timeline)?;
    if job.cancel.load(Relaxed) || todo.is_empty() {
        return Ok(());
    }
    read_metadata(job, memories, &todo, row)
}

/// A walk of the folder tree, shared by the listing workers.
#[derive(Default)]
struct Walk {
    /// Folders to list.
    queue: VecDeque<RemotePath>,
    /// Folders queued so far: a folder given twice (or inside another given one) is listed once.
    seen: HashSet<RemotePath>,
    /// Workers listing a folder right now.
    busy: usize,
    /// Entries looked at.
    entries: usize,
    /// Photos found, and those not sent yet.
    found: usize,
    batch: Chunk<(String, ProbeInfo)>,
    todo: Vec<Todo>,
    /// Why the walk stopped: a folder asked for couldn't be listed, the account was lost, or the
    /// tree is too big.
    error: Option<String>,
}

impl Walk {
    /// What a listing of `dir` adds: its folders to list, its photos.
    fn add(&mut self, job: &Job, dir: &RemotePath, entries: Vec<Entry>, timeline: &HashMap<u64, MemoriesPhoto>) {
        for e in entries {
            self.entries += 1;
            if self.entries > crate::walk::MAX_ENTRIES {
                let max = crate::walk::MAX_ENTRIES;
                self.error = Some(format!("Stopped after {max} files and folders; import fewer folders at a time"));
                return;
            }
            // (a name that isn't a path segment, or a folder past 64 levels, is never a path)
            let Ok(path) = dir.join(&e.name) else { continue };
            if hidden(&e) {
                continue;
            }
            if e.is_dir {
                if self.seen.insert(path.clone()) {
                    self.queue.push_back(path);
                }
                continue;
            }
            if !photo(&e) {
                continue;
            }
            let day = e.fileid.and_then(|id| timeline.get(&id));
            let key = path.to_string();
            if !job.has_camera.contains(&key) {
                self.todo.push(Todo { path, fileid: day.map(|d| d.fileid) });
            }
            self.batch.push((key, listed(&e, day)));
            self.found += 1;
        }
    }
}

/// List `job.dirs` and every folder below them, sending the photos as [`Event::Found`] chunks;
/// the photos whose metadata to read. A subfolder that can't be listed is counted
/// ([`ImportOutcome::failed_folders`]); a folder asked for, or the account, failing stops the walk.
fn walk(job: &Job, row: &TaskGuard, timeline: &HashMap<u64, MemoriesPhoto>) -> Result<Vec<Todo>, String> {
    let state = Mutex::new(Walk { queue: job.dirs.iter().cloned().collect(), seen: job.dirs.iter().cloned().collect(), ..Walk::default() });
    let wake = Condvar::new();
    let work = || {
        loop {
            let dir = {
                let mut w = lock(&state);
                loop {
                    if w.error.is_some() || job.cancel.load(Relaxed) {
                        return;
                    }
                    if let Some(dir) = w.queue.pop_front() {
                        w.busy += 1;
                        break dir;
                    }
                    if w.busy == 0 {
                        return;
                    }
                    // (timed: a worker that died can't leave the others waiting)
                    w = wake.wait_timeout(w, Duration::from_millis(100)).unwrap_or_else(PoisonError::into_inner).0;
                }
            };
            row.detail(dir.name().unwrap_or(&job.account));
            let listed = crate::guard::catch("Listing a Nextcloud folder", || job.remote.list(&dir)).and_then(|r| r);
            // (asked outside the walk's lock: it may read the keyring)
            let lost = if listed.is_err() { job.account_ok().err() } else { None };
            let mut w = lock(&state);
            w.busy = w.busy.saturating_sub(1);
            match listed {
                Ok(entries) => w.add(job, &dir, entries, timeline),
                Err(e) if job.dirs.contains(&dir) => w.error = Some(e),
                Err(_) if lost.is_some() => w.error = lost,
                Err(e) => {
                    log::warn!("Nextcloud import: {dir}: {e}");
                    job.failed_folders.fetch_add(1, Relaxed);
                }
            }
            if let Some(photos) = w.batch.take(false) {
                job.found(photos);
            }
            row.progress(w.found as u64, 0);
            drop(w);
            wake.notify_all();
        }
    };
    on_workers(WORKERS, &work);
    let mut w = std::mem::take(&mut *lock(&state));
    if let Some(photos) = w.batch.take(true) {
        job.found(photos);
    }
    w.error.map_or(Ok(w.todo), Err)
}

/// What a listing says about photo `e` (and Memories, `day`): no hash, as for every remote photo.
fn listed(e: &Entry, day: Option<&MemoriesPhoto>) -> ProbeInfo {
    let format = crate::files::ext_upper(&e.name);
    // (a size no decoder would open is the server's mistake: unknown)
    let max = lightcraft_codecs::DecodeOptions::default().max_pixels;
    let (width, height) = day.map(|d| (d.w, d.h)).filter(|(w, h)| *w > 0 && *h > 0 && u64::from(*w) * u64::from(*h) <= max).unwrap_or((0, 0));
    ProbeInfo {
        width,
        height,
        // what the probe of the file calls it
        format: if format == "JPG" { "JPEG".into() } else { format },
        kind: if hooks::previewed(&e.name) { MediaKind::Image } else { MediaKind::Raw },
        file_size: e.size,
        captured: day.and_then(|d| d.epoch).and_then(wall_time),
        content_hash: None,
        ..ProbeInfo::default()
    }
}

/// Unix seconds as the wall time they stand for (`YYYY-MM-DDTHH:MM:SS`, no time zone, as cameras
/// record it and Memories keeps it); `None` outside the years 0000–9999.
fn wall_time(secs: i64) -> Option<String> {
    const FIRST: i64 = -62_167_219_200; // 0000-01-01T00:00:00
    const LAST: i64 = 253_402_300_799; // 9999-12-31T23:59:59
    (FIRST..=LAST).contains(&secs).then(|| lightcraft_catalog::dates::civil(secs))
}

/// Read the metadata of `todo`, 8 at a time, as [`Event::Meta`] chunks, behind a cancellable
/// `scan` row (which replaces `import_row`). Stops, with the reason, once the account can't be asked.
fn read_metadata(job: &Job, memories: Option<Memories<'_>>, todo: &[Todo], import_row: TaskGuard) -> Result<(), String> {
    let row = job.remote.activity.start("scan", "Reading metadata", Cancel::Flag(job.cancel.clone()));
    let total = todo.len() as u64;
    row.progress(0, total);
    // (the scan row is up first: the import never shows no row)
    drop(import_row);
    #[cfg(test)]
    job.remote.import_step();
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let (chunk, lost) = (Mutex::new(Chunk::default()), Mutex::new(None));
    let work = || {
        while !job.cancel.load(Relaxed) {
            // a signed-out or unreachable account isn't asked again (the captured client would)
            if let Err(e) = job.account_ok() {
                lock(&lost).get_or_insert(e);
                return;
            }
            let Some(t) = todo.get(next.fetch_add(1, Relaxed)) else { return };
            match crate::guard::catch("Reading Nextcloud metadata", || metadata(&job.remote, memories, t)).and_then(|r| r) {
                Ok((info, described)) => {
                    let mut c = lock(&chunk);
                    c.push(MetaRead { path: t.path.to_string(), info, described });
                    if let Some(reads) = c.take(false) {
                        job.read(reads);
                    }
                }
                Err(e) => log::debug!("Nextcloud import: metadata of {}: {e}", t.path),
            }
            row.progress(done.fetch_add(1, Relaxed).saturating_add(1) as u64, total);
        }
    };
    on_workers(WORKERS.min(todo.len()), &work);
    if let Some(reads) = lock(&chunk).take(true) {
        job.read(reads);
    }
    lock(&lost).take().map_or(Ok(()), Err)
}

/// The metadata of `t`: Memories' EXIF when it's on the timeline, else from the start of the file
/// ([`FIRST_WINDOW`], then [`META_WINDOW`] when that had no camera). `true` when a read was the
/// whole file: then its description counts (dimensions, preview-only); from part of a file only
/// its metadata does.
fn metadata(remote: &Remote, memories: Option<Memories<'_>>, t: &Todo) -> Result<(ProbeInfo, bool), String> {
    if let (Some(m), Some(id)) = (memories, t.fileid) {
        match m.info(id) {
            Ok(meta) => return Ok((exif(meta), false)),
            // (the file's start may still tell)
            Err(e) => remote.note_error(&t.path.account_key(), &e),
        }
    }
    let name = t.path.name().unwrap_or_default();
    let mut read = ProbeInfo::default();
    for window in [FIRST_WINDOW, META_WINDOW] {
        let head = remote.read_head(&t.path, window)?;
        // fewer bytes than asked for: the file ended (the listing's size isn't trusted for this)
        if (head.len() as u64) < window {
            return Ok(match crate::files::probe_bytes(name, &head) {
                Ok(info) => (ProbeInfo { content_hash: None, ..info }, true),
                Err(_) => (metadata_only(&head), false),
            });
        }
        read = metadata_only(&head);
        if !read.meta.camera.is_empty() {
            break;
        }
    }
    Ok((read, false))
}

/// The metadata and capture time in a file's first bytes `head`.
fn metadata_only(head: &[u8]) -> ProbeInfo {
    let (meta, captured) = crate::files::meta_of(&lightcraft_meta::extract(head));
    ProbeInfo { meta, captured, ..ProbeInfo::default() }
}

/// Memories' EXIF details as the probe of the file would give them (no zero or negative exposure,
/// aperture or focal length).
fn exif(m: RemoteMeta) -> ProbeInfo {
    let positive = |v: Option<f64>| v.filter(|v| *v > 0.0);
    let read = lightcraft_meta::Metadata {
        make: m.make,
        model: m.model,
        lens_model: m.lens,
        exposure_time: positive(m.exposure_time),
        f_number: positive(m.f_number),
        iso: m.iso,
        focal_length: positive(m.focal_length),
        ..Default::default()
    };
    let (meta, _) = crate::files::meta_of(&read);
    ProbeInfo { meta, captured: m.captured.and_then(wall_time), ..ProbeInfo::default() }
}

/// Run `work` on `n` threads at once (here, when none can be started), until each returns.
fn on_workers(n: usize, work: &(dyn Fn() + Sync)) {
    std::thread::scope(|sc| {
        let handles: Vec<_> =
            (0..n).filter_map(|i| std::thread::Builder::new().name(format!("nextcloud-import-{i}")).spawn_scoped(sc, work).ok()).collect();
        if handles.is_empty() && n > 0 {
            work();
        }
        for h in handles {
            if h.join().is_err() {
                log::error!("Nextcloud import: a worker stopped unexpectedly");
            }
        }
    });
}

/// What [`apply_events`] did.
#[derive(Default)]
pub(crate) struct Tally {
    /// Photos added (or brought back from Recently Deleted).
    pub added: usize,
    /// Photos the catalog had already.
    pub skipped: usize,
    /// Photos whose metadata changed.
    pub metadata: usize,
}

/// Apply the queued [`Event`]s: an import's into the open library (others are dropped): photos found
/// join the catalog (one undoable step per import), metadata fills what photos lack (not an undo
/// step); then downloaded originals complete their photos ([`super::originals::complete`]).
pub(crate) fn apply_events(s: &mut Session, tally: &mut Tally) {
    let events = s.remote.take_events();
    if events.is_empty() {
        return;
    }
    let library = s.library_generation();
    let mut found: Vec<(u64, Vec<(String, ProbeInfo)>)> = Vec::new();
    let mut reads = Vec::new();
    let mut downloaded = Vec::new();
    for e in events {
        match e {
            // (sent into a library that is no longer open)
            Event::Found { generation, .. } | Event::Meta { generation, .. } if generation != library => {}
            Event::Found { task, photos, .. } => match found.last_mut() {
                Some((t, batch)) if *t == task => batch.extend(photos),
                _ => found.push((task, photos)),
            },
            Event::Meta { reads: r, .. } => reads.extend(r),
            Event::Downloaded { path, info } => downloaded.push((path, info)),
        }
    }
    // (the photos first: a photo's metadata never comes before the photo)
    for (task, photos) in found {
        add(s, task, photos, tally);
    }
    if !reads.is_empty() {
        fill(s, reads, tally);
    }
    if !downloaded.is_empty() {
        super::originals::complete(s, downloaded);
    }
}

/// Add import `task`'s photos (mode Add; paths the catalog has are skipped), joining the import's
/// undo step when nothing was done since its last batch.
fn add(s: &mut Session, task: u64, photos: Vec<(String, ProbeInfo)>, tally: &mut Tally) {
    let opts = crate::import::ImportOptions::default();
    // ponytail: `ImportJob::new` indexes every catalog path (~10-28 ms at 85k photos, measured); chunks
    // keep it to ≤ 4 drains a second per import. Indexing only the batch's paths would make it O(batch).
    // (the probes a scan in the import dialog left stay for its own import)
    let probes = std::mem::take(&mut s.import_probes);
    let job = crate::import::ImportJob::new(s, opts.clone());
    s.import_probes = probes;
    let before = (s.commits(), s.undo.len());
    let report = job.and_then(|mut job| {
        let prepared = job.prepare_infos(photos);
        let now = job.now().to_string();
        crate::import::commit_prepared(s, &opts, &now, prepared)
    });
    let report = match report {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Nextcloud import: {e}");
            return;
        }
    };
    let added = report.imported.len() + report.restored.len();
    tally.added += added;
    tally.skipped += report.duplicates.len();
    let committed = s.commits() != before.0;
    let (mut joins, mut total) = (false, added);
    s.remote.update_import(task, |r| {
        r.outcome.added += added;
        total = r.outcome.added;
        joins = committed && r.undo == Some(before);
    });
    if !committed {
        return;
    }
    if joins {
        s.merge_undo(2, &format!("Add {total} Photo{}", if total == 1 { "" } else { "s" }));
    }
    let mark = (s.commits(), s.undo.len());
    s.remote.update_import(task, |r| r.undo = Some(mark));
}

/// Fill what photos lack from the metadata `reads` (one pass over the catalog).
fn fill(s: &mut Session, reads: Vec<MetaRead>, tally: &mut Tally) {
    let ids: HashMap<String, PhotoId> = {
        let paths: HashSet<&str> = reads.iter().map(|r| r.path.as_str()).collect();
        s.catalog
            .photos()
            .filter(|p| p.copy_of.is_none())
            .filter_map(|p| match &p.source {
                Source::File { path } if paths.contains(path.as_str()) => Some((path.clone(), p.id)),
                _ => None,
            })
            .collect()
    };
    for MetaRead { path, info, described } in reads {
        let Some(ph) = ids.get(&path).and_then(|id| s.catalog.photo(*id)) else { continue };
        let mut ops = crate::cmd::convert::fill_missing_meta(ph.id, ph, &info);
        if described {
            // (the size stays the listing's)
            ops.extend(crate::cmd::convert::content_op(ph.id, ph, ProbeInfo { file_size: ph.file_size, ..info }));
        }
        if ops.is_empty() {
            continue;
        }
        match s.apply_system(Op::Batch { ops }) {
            Ok(()) => tally.metadata += 1,
            Err(e) => log::warn!("Nextcloud import: metadata of {path}: {e}"),
        }
    }
}
