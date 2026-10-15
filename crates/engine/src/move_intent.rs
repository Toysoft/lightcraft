//! A record of file moves in progress, written into the library before the first file moves and
//! removed once the library has saved where the files went. If LightCraft is stopped part-way (a
//! force-quit, a crash, a power cut), the record is still there the next time the library opens,
//! and [`Session::recover_moves`] brings the library up to date with the disk:
//!
//! * a file that reached its new place (and is gone from the old one): its photos point there,
//!   and its XMP sidecars follow it;
//! * a file still at its old place and not at the new one: nothing to do;
//! * a file at both places: the copy at the new place is ours (nothing was there when the record
//!   was written, and moves never replace a file). It is removed when it is the same file or holds
//!   the same bytes; the library keeps the original. Otherwise both stay and the person is told;
//! * a file at neither place (its disk isn't connected): the record keeps it for the next time.
//!
//! The records live in `<library>/moves/`, one file per batch, so a background move and a move
//! the session makes itself (a rename, an Undo) never share one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lightcraft_catalog::store::{FsStore, Store};
use lightcraft_catalog::{Op, Source};
use serde::{Deserialize, Serialize};

use crate::rename::{MoveFs, folded_path};
use crate::{EngineError, Result, Session};

/// The folder of the library that holds the records.
const DIR: &str = "moves";
/// The records' format, so a later one can tell an older file apart.
const VERSION: u32 = 1;
/// Records larger than this aren't read (a damaged or planted file): 64 MiB is about half a
/// million moves.
const MAX_BYTES: u64 = 64 << 20;

#[derive(Serialize, Deserialize, Default)]
struct Record {
    version: u32,
    /// (from, to) for each file, in the order they move.
    moves: Vec<(String, String)>,
}

/// A record on disk: removed once the library has saved the moves it names
/// ([`Session::settle_moves`]).
#[derive(Debug)]
pub(crate) struct MoveRecord {
    path: PathBuf,
}

/// Write the record of `moves` into the library folder `library`, durably, before any of them
/// is made. Callable from a worker thread.
pub(crate) fn write(library: &Path, moves: &[(String, String)]) -> std::io::Result<MoveRecord> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = library.join(DIR);
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let name = format!("move-{}-{stamp}-{}.json", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let bytes = serde_json::to_vec(&Record { version: VERSION, moves: moves.to_vec() }).map_err(std::io::Error::other)?;
    FsStore::open(&dir)?.write_atomic(&name, &bytes)?;
    Ok(MoveRecord { path: dir.join(name) })
}

/// What the records left by an earlier run came to (`library.info` and a notice).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovered {
    /// Photos now pointing at the place their file had reached.
    pub relinked: usize,
    /// Copies at the new place removed (the original stayed where the library has it).
    pub copies_removed: usize,
    /// Files whose two places couldn't be decided on (named in the notice), or whose sidecars
    /// couldn't follow.
    pub problems: Vec<String>,
    /// Moves kept for the next time: neither place could be read (a disk not connected).
    pub waiting: usize,
}

impl Recovered {
    /// The notice for the person, or `None` when there is nothing to say.
    pub fn notice(&self) -> Option<String> {
        if self.relinked == 0 && self.problems.is_empty() && self.waiting == 0 {
            return None;
        }
        let mut text = String::new();
        if self.relinked > 0 {
            let s = if self.relinked == 1 { "" } else { "s" };
            text.push_str(&format!(
                "{} photo{s} were being moved when LightCraft last closed: the library now points at where their files are. Edit → Undo moves them back.",
                self.relinked
            ));
        }
        if self.waiting > 0 {
            let s = if self.waiting == 1 { "" } else { "s" };
            text.push_str(&format!(
                " {} file{s} that were being moved can't be found at either place (a disk that isn't connected?): LightCraft looks again the next time this library opens.",
                self.waiting
            ));
        }
        for p in &self.problems {
            text.push(' ');
            text.push_str(p);
        }
        Some(text.trim().to_string())
    }
}

/// Where one recorded move stands on disk.
enum Found {
    /// Reached `to`: point the photos there.
    Moved,
    /// Still at `from` only, or settled already: nothing to do.
    Stayed,
    /// Neither place answers: keep the move for next time.
    Unknown,
}

impl Session {
    /// Write the record of `moves` (made by this session, on this thread) before making them;
    /// `None` when the library has no folder of its own (nothing survives a restart there) or
    /// there's nothing to move.
    pub(crate) fn record_moves(&self, moves: &[(String, String)]) -> Result<Option<MoveRecord>> {
        let Some(dir) = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone()) else { return Ok(None) };
        if moves.is_empty() {
            return Ok(None);
        }
        write(&dir, moves).map(Some).map_err(|e| EngineError::Other(format!("can't note the move in the library ({e}): nothing was moved")))
    }

    /// The moves `record` names are in the catalog now: the record goes once that is saved
    /// (after the command, see [`Session::clear_settled_moves`]).
    pub(crate) fn settle_moves(&mut self, record: Option<MoveRecord>) {
        if let Some(r) = record {
            self.settled_moves.push(r.path);
        }
    }

    /// Remove the records of settled moves once everything the library holds is saved; while a
    /// save is failing they stay, and the next open finds the files where the record says.
    pub(crate) fn clear_settled_moves(&mut self) {
        if self.settled_moves.is_empty() || !self.pending_log.is_empty() {
            return;
        }
        for path in std::mem::take(&mut self.settled_moves) {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("move record {}: {e}", path.display());
            }
        }
    }

    /// Bring the library up to date with moves left unfinished (see the module docs): those of an
    /// earlier run when the library opens, or of a move whose worker died. The relinks are one undo
    /// step; what was done is returned for a notice.
    pub fn recover_moves(&mut self) -> Recovered {
        self.recover_moves_with(&crate::rename::RealFs)
    }

    pub(crate) fn recover_moves_with(&mut self, fs: &dyn MoveFs) -> Recovered {
        let mut out = Recovered::default();
        let Some(dir) = self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(DIR)) else { return out };
        let Ok(entries) = std::fs::read_dir(&dir) else { return out };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        let mut ops = Vec::new();
        // (record, the moves to keep in it): written back, or removed when none is left
        let mut done: Vec<(PathBuf, Vec<(String, String)>)> = Vec::new();
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.ends_with(".tmp") {
                // a record cut short while it was written: nothing had moved yet
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !name.ends_with(".json") {
                continue;
            }
            let record = match read_record(&path) {
                Ok(r) => r,
                Err(e) => {
                    out.problems.push(format!("A record of a move ({}) can't be read ({e}); it was left as it is.", path.display()));
                    continue;
                }
            };
            let mut keep = Vec::new();
            for (from, to) in record.moves {
                match self.recover_one(fs, &from, &to, &mut ops, &mut out) {
                    Found::Moved | Found::Stayed => {}
                    Found::Unknown => {
                        out.waiting += 1;
                        keep.push((from, to));
                    }
                }
            }
            done.push((path, keep));
        }
        let n = ops.len();
        if n > 0 {
            let s = if n == 1 { "" } else { "s" };
            if let Err(e) = self.commit(&format!("Finish Moving {n} Photo{s}"), Op::Batch { ops }) {
                out.problems.push(format!("The library couldn't follow the files that were being moved ({e}); Find Missing Photos relinks them."));
                return out;
            }
            out.relinked = n;
            if let Err(e) = self.persist() {
                // the records stay: the next open finds the files where they are and does this again
                out.problems.push(format!("Not saved: {e}"));
                return out;
            }
        }
        for (path, keep) in done {
            let kept = if keep.is_empty() {
                std::fs::remove_file(&path).map_err(|e| e.to_string())
            } else {
                serde_json::to_vec(&Record { version: VERSION, moves: keep }).map_err(|e| e.to_string()).and_then(|bytes| {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    FsStore::open(&dir).and_then(|mut st| st.write_atomic(&name, &bytes)).map_err(|e| e.to_string())
                })
            };
            if let Err(e) = kept {
                log::warn!("move record {}: {e}", path.display());
            }
        }
        out
    }

    /// One recorded move: where the file is, and what the library does about it.
    fn recover_one(&mut self, fs: &dyn MoveFs, from: &str, to: &str, ops: &mut Vec<Op>, out: &mut Recovered) -> Found {
        let (f, t) = (Path::new(from), Path::new(to));
        // the photos still pointing at the old place (a master and its virtual copies)
        let ids: Vec<_> = self
            .catalog
            .photos()
            .filter(|p| matches!(&p.source, Source::File { path } if Path::new(path) == f))
            .map(|p| (p.id, p.file_name.clone()))
            .collect();
        if ids.is_empty() {
            // the library already follows the move (it was saved), or the photo is gone
            return Found::Stayed;
        }
        // the same file spelled another way (a change of letter case only): nothing to remove, ever
        let respelling = folded_path(from) == folded_path(to) && fs.same_file(f, t);
        let moved = if respelling {
            fs.listed(t) && !fs.listed(f)
        } else {
            match (fs.exists(f), fs.is_file(t)) {
                (false, true) => true,
                (true, false) => false,
                (true, true) => {
                    // the copy at `to` is ours: drop it when that loses nothing
                    let same = fs.same_file(f, t) || crate::rename::same_contents(f, t).unwrap_or(false);
                    if same && fs.remove_file(t).is_ok() {
                        out.copies_removed += 1;
                    } else {
                        out.problems.push(format!("{to} was being moved and is now at both places: the library keeps {from}."));
                    }
                    false
                }
                (false, false) => return Found::Unknown,
            }
        };
        remove_partials(t);
        if !moved {
            return Found::Stayed;
        }
        if !respelling && let Err(e) = crate::rename::finish_sidecars(fs, from, to) {
            out.problems.push(format!("A sidecar of {to} couldn't follow it ({e})."));
        }
        let (old_name, new_name) = (file_name(f), file_name(t));
        for (id, file_name) in ids {
            // a rename changes the name the library shows; a move to another folder keeps it
            let file_name = if file_name == old_name { new_name.clone() } else { file_name };
            ops.push(Op::SetFile { id, file_name, source: Source::File { path: to.to_string() } });
        }
        Found::Moved
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn read_record(path: &Path) -> std::result::Result<Record, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_BYTES {
        return Err(format!("{len} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let r: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if r.version > VERSION {
        return Err(format!("written by a newer LightCraft (format {})", r.version));
    }
    Ok(r)
}

/// Hidden partial copies a cut-off copy left beside `to` (`.<name>.lc-partial-<pid>-<k>`, see
/// `RealFs::copy_no_replace`): never a photo, safe to remove.
fn remove_partials(to: &Path) {
    let (Some(dir), Some(name)) = (to.parent(), to.file_name()) else { return };
    let prefix = format!(".{}.lc-partial-", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
