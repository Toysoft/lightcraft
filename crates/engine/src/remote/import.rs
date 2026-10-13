//! Looking into Nextcloud folders (`nextcloud.browse`) and importing them (`library.importNextcloud`).
//!
//! An import runs on a worker thread, behind activity rows. It lists the folders, 8 at a time, and
//! sends the photos it finds as [`Event::Found`] batches: size, kind and format from the listing,
//! dimensions and capture time from the Memories app when the server has it. Then it reads the
//! metadata of the photos that had no camera yet, from Memories' EXIF or else the first MiB of the
//! file, as [`Event::Meta`]. No original is downloaded, and nothing reaches the catalog off the
//! session: [`crate::Session::drain_remote`] applies the events.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use craft_nextcloud::{Entry, Memories, MemoriesPhoto, RemoteMeta, RemotePath};
use lightcraft_catalog::{MediaKind, Op, PhotoId, Source, is_remote};
use serde_json::{Value, json};

use super::{Event, Remote, hooks};
use crate::activity::{Cancel, TaskGuard};
use crate::media::ProbeInfo;
use crate::{EngineError, Session};

/// Folders listed, and photos' metadata read, at once.
const WORKERS: usize = 8;
/// Bytes read from the start of a photo for its metadata. Measured on real files (NC5), the first
/// 64 KiB of an ARW and 512 KiB of a CR3 hold its camera, lens, exposure and capture time.
const META_WINDOW: u64 = 1 << 20;

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

/// One import, as its worker sees it.
struct Job {
    remote: Arc<Remote>,
    account: String,
    dirs: Vec<RemotePath>,
    /// Remote photos that had a camera when the import started: their metadata isn't read again.
    has_camera: HashSet<String>,
    cancel: Arc<AtomicBool>,
}

/// A photo whose metadata the import reads once the folders are listed.
struct Todo {
    path: RemotePath,
    /// On the Memories timeline.
    fileid: Option<u64>,
}

/// `library.importNextcloud`: import the photos in `dirs` (folders of account `account`, which this
/// computer has) and their subfolders. Returns `{"started": true}` at once; with `wait`, applies
/// the events as they come and returns `{"added", "skipped", "metadata"}` once the import is done.
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
    let job = Job { remote: s.remote.clone(), account, dirs, has_camera, cancel };
    let worker = std::thread::Builder::new()
        .name("nextcloud-import".into())
        .spawn(move || {
            // last-resort guard: a panic ends this import, with its rows
            let r = crate::guard::catch("The Nextcloud import", || run(&job, row)).and_then(|r| r);
            if let Err(e) = &r {
                log::warn!("Nextcloud import: {e}");
            }
            r
        })
        .map_err(|e| EngineError::Other(format!("could not start the import: {e}")))?;
    if !wait {
        return Ok(json!({"started": true}));
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
    Ok(json!({"added": tally.added, "skipped": tally.skipped, "metadata": tally.metadata}))
}

/// The worker: list, then read metadata. `row` is the import's row, held while it lists.
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
    drop(row);
    if !job.cancel.load(Relaxed) {
        read_metadata(job, memories, &todo);
    }
    Ok(())
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
    batch: Vec<(String, ProbeInfo)>,
    todo: Vec<Todo>,
    /// Why the walk stopped: a folder asked for couldn't be listed, or the tree is too big.
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

/// List `job.dirs` and every folder below them, sending the photos as [`Event::Found`] batches;
/// the photos whose metadata to read.
fn walk(job: &Job, row: &TaskGuard, timeline: &HashMap<u64, MemoriesPhoto>) -> Result<Vec<Todo>, String> {
    let state = Mutex::new(Walk { queue: job.dirs.iter().cloned().collect(), seen: job.dirs.iter().cloned().collect(), ..Walk::default() });
    let lock = || state.lock().unwrap_or_else(PoisonError::into_inner);
    let wake = Condvar::new();
    let batch = crate::import::batch_size();
    let work = || {
        loop {
            let dir = {
                let mut w = lock();
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
            let mut w = lock();
            w.busy = w.busy.saturating_sub(1);
            match listed {
                Ok(entries) => w.add(job, &dir, entries, timeline),
                Err(e) if job.dirs.contains(&dir) => w.error = Some(e),
                Err(e) => log::warn!("Nextcloud import: {dir}: {e}"),
            }
            if w.batch.len() >= batch {
                job.remote.push_event(Event::Found(std::mem::take(&mut w.batch)));
            }
            row.progress(w.found as u64, 0);
            drop(w);
            wake.notify_all();
        }
    };
    on_workers(WORKERS, &work);
    let mut w = std::mem::take(&mut *lock());
    if !w.batch.is_empty() {
        job.remote.push_event(Event::Found(std::mem::take(&mut w.batch)));
    }
    w.error.map_or(Ok(w.todo), Err)
}

/// What a listing says about photo `e` (and Memories, `day`): no hash, as for every remote photo.
fn listed(e: &Entry, day: Option<&MemoriesPhoto>) -> ProbeInfo {
    let format = crate::files::ext_upper(&e.name);
    // (a size no decoder would open is the server's mistake: unknown)
    let max = lightcraft_codecs::DecodeOptions::default().max_pixels;
    let (width, height) = day.map(|d| (d.w, d.h)).filter(|(w, h)| u64::from(*w) * u64::from(*h) <= max).unwrap_or((0, 0));
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

/// Read the metadata of `todo`, 8 at a time, as [`Event::Meta`]s, behind a cancellable `scan` row.
fn read_metadata(job: &Job, memories: Option<Memories<'_>>, todo: &[Todo]) {
    if todo.is_empty() {
        return;
    }
    let row = job.remote.activity.start("scan", "Reading metadata", Cancel::Flag(job.cancel.clone()));
    let total = todo.len() as u64;
    row.progress(0, total);
    #[cfg(test)]
    job.remote.import_step();
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let work = || {
        while !job.cancel.load(Relaxed) {
            let Some(t) = todo.get(next.fetch_add(1, Relaxed)) else { return };
            match crate::guard::catch("Reading Nextcloud metadata", || metadata(&job.remote, memories, t)).and_then(|r| r) {
                Ok((info, described)) => job.remote.push_event(Event::Meta { path: t.path.to_string(), info: Box::new(info), described }),
                Err(e) => log::debug!("Nextcloud import: metadata of {}: {e}", t.path),
            }
            row.progress(done.fetch_add(1, Relaxed).saturating_add(1) as u64, total);
        }
    };
    on_workers(WORKERS.min(todo.len()), &work);
}

/// The metadata of `t`: Memories' EXIF when it's on the timeline, else from the file's first
/// [`META_WINDOW`] bytes. `true` when those were the whole file (or an image they describe): then the
/// description counts (dimensions, preview-only); from part of a raw only its metadata does.
fn metadata(remote: &Remote, memories: Option<Memories<'_>>, t: &Todo) -> Result<(ProbeInfo, bool), String> {
    if let (Some(m), Some(id)) = (memories, t.fileid) {
        match m.info(id) {
            Ok(meta) => return Ok((exif(meta), false)),
            // (the file's start may still tell)
            Err(e) => remote.note_error(&t.path.account_key(), &e),
        }
    }
    let head = remote.read_head(&t.path, META_WINDOW)?;
    let name = t.path.name().unwrap_or_default();
    // fewer bytes than asked for: the file ended (the listing's size isn't trusted for this)
    let size = if (head.len() as u64) < META_WINDOW { head.len() as u64 } else { u64::MAX };
    if let Ok(info) = hooks::describe(name, &head, size) {
        return Ok((ProbeInfo { content_hash: None, ..info }, true));
    }
    let (meta, captured) = crate::files::meta_of(&lightcraft_meta::extract(&head));
    Ok((ProbeInfo { meta, captured, ..ProbeInfo::default() }, false))
}

/// Memories' EXIF details as the probe of the file would give them.
fn exif(m: RemoteMeta) -> ProbeInfo {
    let read = lightcraft_meta::Metadata {
        make: m.make,
        model: m.model,
        lens_model: m.lens,
        exposure_time: m.exposure_time,
        f_number: m.f_number,
        iso: m.iso,
        focal_length: m.focal_length,
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

/// Apply the queued [`Event`]s: photos found join the catalog (one undoable step), metadata fills
/// what photos lack (not an undo step).
pub(crate) fn apply_events(s: &mut Session, tally: &mut Tally) {
    let events = s.remote.take_events();
    if events.is_empty() {
        return;
    }
    let (mut found, mut metas) = (Vec::new(), Vec::new());
    for e in events {
        match e {
            Event::Found(items) => found.extend(items),
            Event::Meta { path, info, described } => metas.push((path, info, described)),
        }
    }
    // (the photos first: a photo's metadata never comes before the photo)
    if !found.is_empty() {
        match add(s, found) {
            Ok(r) => {
                tally.added += r.imported.len() + r.restored.len();
                tally.skipped += r.duplicates.len();
            }
            Err(e) => log::warn!("Nextcloud import: {e}"),
        }
    }
    if metas.is_empty() {
        return;
    }
    let ids: HashMap<String, PhotoId> = {
        let paths: HashSet<&str> = metas.iter().map(|(path, ..)| path.as_str()).collect();
        s.catalog
            .photos()
            .filter(|p| p.copy_of.is_none())
            .filter_map(|p| match &p.source {
                Source::File { path } if paths.contains(path.as_str()) => Some((path.clone(), p.id)),
                _ => None,
            })
            .collect()
    };
    for (path, info, described) in metas {
        let Some(ph) = ids.get(&path).and_then(|id| s.catalog.photo(*id)) else { continue };
        let mut ops = crate::cmd::convert::fill_missing_meta(ph.id, ph, &info);
        if described {
            // (the size stays the listing's)
            ops.extend(crate::cmd::convert::content_op(ph.id, ph, ProbeInfo { file_size: ph.file_size, ..*info }));
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

/// Add the photos `found` lists (mode Add; paths the catalog has are skipped).
fn add(s: &mut Session, found: Vec<(String, ProbeInfo)>) -> crate::Result<crate::import::ImportReport> {
    let opts = crate::import::ImportOptions::default();
    // (the probes a scan in the import dialog left stay for its own import)
    let probes = std::mem::take(&mut s.import_probes);
    let job = crate::import::ImportJob::new(s, opts.clone());
    s.import_probes = probes;
    let mut job = job?;
    let prepared = job.prepare_infos(found);
    let now = job.now().to_string();
    crate::import::commit_prepared(s, &opts, &now, prepared)
}

impl Remote {
    /// Tests: wait for the next message on [`Remote::import_steps`], if set (at most 10 s).
    #[cfg(test)]
    fn import_step(&self) {
        if let Some(steps) = &*self.import_steps.lock().unwrap_or_else(PoisonError::into_inner) {
            let _ = steps.recv_timeout(Duration::from_secs(10));
        }
    }
}
