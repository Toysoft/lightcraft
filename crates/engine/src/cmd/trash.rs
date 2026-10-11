//! `photo.trashFiles`: photos leave the library for good and their files go to the system Trash
//! (see [`crate::trash`]). The one command that touches a photo's file on the way out.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{PhotoId, Source};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_selection, str_param};
use crate::rename::MoveFs;
use crate::trash::{BinError, TrashStep, Trashed, Whereabouts};
use crate::{Result, Session};

/// How many photos one call may name.
const MAX_PHOTOS: usize = 1_000_000;

/// What `noTrash` says to do with files on a disk without a Trash.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoTrash {
    /// The photo and its file stay; the answer names them.
    Keep,
    /// Their files are erased for good.
    Erase,
}

fn no_trash_param(p: &Value, c: &str) -> Result<NoTrash> {
    match p.get("noTrash") {
        None | Some(Value::Null) => Ok(NoTrash::Keep),
        Some(v) => match v.as_str() {
            Some("keep") => Ok(NoTrash::Keep),
            Some("delete") => Ok(NoTrash::Erase),
            _ => Err(bad(c, "`noTrash` is \"keep\" (the default) or \"delete\"")),
        },
    }
}

/// The sidecars beside `original` that are there now (both namings; `.xmp` and `.XMP` are one
/// file on a disk that ignores case), with whether a photo beside it still uses it: a sidecar
/// named for the stem (`IMG_1.xmp`) is also `IMG_1.dng`'s while that file is there.
fn sidecars_now(original: &str) -> Vec<(PathBuf, bool)> {
    let fs = crate::rename::RealFs;
    let mut out: Vec<(PathBuf, bool)> = Vec::new();
    for naming in [crate::sidecar::SidecarNaming::Stem, crate::sidecar::SidecarNaming::Full] {
        let lower = crate::sidecar::sidecar_path(original, naming);
        for path in [lower.clone(), lower.with_extension("XMP")] {
            if !fs.is_file(&path) || out.iter().any(|(o, _)| fs.same_file(o, &path)) {
                continue;
            }
            let shared = naming == crate::sidecar::SidecarNaming::Stem && fs.stem_sibling(&path).is_some();
            out.push((path, shared));
        }
    }
    out
}

/// Take `ids` out of the library for good and move their files to the Trash, as one undo step.
/// Everything that can go goes; what can't is named in the answer and stays, photo and file.
pub(crate) fn trash_files(s: &mut Session, ids: &[PhotoId], no_trash: NoTrashParam, c: &str) -> Result<Value> {
    let no_trash = no_trash.0;
    if ids.is_empty() {
        return Err(bad(c, "no photos to move to the Trash"));
    }
    if ids.len() > MAX_PHOTOS {
        return Err(bad(c, format!("more than {MAX_PHOTOS} photos in one call")));
    }
    if let Some(gone) = ids.iter().find(|id| s.catalog.photo(**id).is_none()) {
        return Err(bad(c, format!("no such photo {}", gone.0)));
    }
    let named: HashSet<PhotoId> = ids.iter().copied().collect();
    // photos only browsed in Local are not the library's to take out
    let browsed = ids.iter().filter(|id| s.catalog.photo(**id).is_some_and(|p| p.local)).count();
    if browsed == named.len() {
        return Err(bad(c, "photos browsed in Local are not in the library: add them to My Photos first"));
    }
    // a file other photos of the library are (copies), which stay: it stays with them
    let mut kept_paths: HashSet<&str> = HashSet::new();
    for p in s.catalog.photos().filter(|p| !named.contains(&p.id)) {
        if let Source::File { path } = &p.source {
            kept_paths.insert(path.as_str());
        }
    }
    let bin = s.trash.clone();
    let fs = crate::rename::RealFs;
    let (mut removed, mut no_trash_ids, mut failed): (Vec<PhotoId>, Vec<u64>, Vec<Value>) = (Vec::new(), Vec::new(), Vec::new());
    let (mut taken, mut gone_paths): (Vec<Trashed>, Vec<String>) = (Vec::new(), Vec::new());
    let (mut missing, mut shared) = (0usize, 0usize);
    let mut done: HashSet<String> = HashSet::new();
    let mut seen: HashSet<PhotoId> = HashSet::new();
    // the photos' own files first; the sidecars after, once it is known which photos stay
    for id in ids.iter().filter(|id| seen.insert(**id)) {
        let Some(p) = s.catalog.photo(*id) else { continue };
        if p.local {
            continue;
        }
        let Source::File { path } = &p.source else {
            // no file of its own (a demo scene): it just leaves
            removed.push(*id);
            continue;
        };
        if kept_paths.contains(path.as_str()) {
            shared += 1;
            removed.push(*id);
            continue;
        }
        if done.contains(path) {
            // another of the photos named is this file, which went with it
            removed.push(*id);
            continue;
        }
        if !fs.exists(Path::new(path)) {
            missing += 1;
            removed.push(*id);
            continue;
        }
        match bin.trash(Path::new(path)) {
            Ok(at) => {
                taken
                    .push(Trashed { from: path.clone(), now: at.map_or(Whereabouts::InTrash, |a| Whereabouts::At(a.to_string_lossy().to_string())) });
            }
            Err(BinError::NoTrash) if no_trash == NoTrash::Erase => match bin.erase(Path::new(path)) {
                Ok(()) => taken.push(Trashed { from: path.clone(), now: Whereabouts::Erased }),
                Err(e) => {
                    failed.push(json!({"id": id.0, "path": path, "why": format!("{path}: can't be erased: {e}")}));
                    continue;
                }
            },
            Err(BinError::NoTrash) => {
                no_trash_ids.push(id.0);
                continue;
            }
            Err(BinError::Failed(why)) => {
                failed.push(json!({"id": id.0, "path": path, "why": why}));
                continue;
            }
        }
        done.insert(path.clone());
        gone_paths.push(path.clone());
        removed.push(*id);
    }
    // a file a photo that could not go is: not gone after all? (it was never taken: the paths
    // in `gone_paths` are those of photos that went). Their sidecars follow them, but for a
    // sidecar a photo beside it still uses. One that can't go stays behind; nothing is lost.
    let erased_originals: HashSet<String> = taken.iter().filter(|t| t.now == Whereabouts::Erased).map(|t| t.from.clone()).collect();
    for original in &gone_paths {
        for (sidecar, in_use) in sidecars_now(original) {
            if in_use {
                continue;
            }
            let from = sidecar.to_string_lossy().to_string();
            if erased_originals.contains(original) {
                if bin.erase(&sidecar).is_ok() {
                    taken.push(Trashed { from, now: Whereabouts::Erased });
                }
            } else if let Ok(at) = bin.trash(&sidecar) {
                taken.push(Trashed { from, now: at.map_or(Whereabouts::InTrash, |a| Whereabouts::At(a.to_string_lossy().to_string())) });
            }
        }
    }
    let erased = taken.iter().filter(|t| t.now == Whereabouts::Erased).count();
    let trashed = taken.len() - erased;
    if removed.is_empty() {
        // nothing went. A disk without a Trash is a question to ask, not an error; anything
        // else says why
        if failed.is_empty() {
            return Ok(
                json!({"removed": 0, "trashed": 0, "erased": 0, "missing": 0, "shared": 0, "noTrash": no_trash_ids, "failed": [], "skipped": browsed}),
            );
        }
        let why = failed.first().and_then(|f| f["why"].as_str()).unwrap_or("the files could not be moved to the Trash").to_string();
        let more = if failed.len() > 1 { format!(" (and {} more)", failed.len() - 1) } else { String::new() };
        return Err(bad(c, format!("{why}{more}: nothing was changed")));
    }
    let before = s.visible_cloned();
    let n = removed.len();
    let whose = if n == 1 { "Photo's".to_string() } else { format!("{n} Photos'") };
    let label = if trashed == 0 && erased > 0 { format!("Erase {whose} Files") } else { format!("Move {whose} Files to Trash") };
    let op = s.catalog.delete_photos_permanently_ops(&removed);
    if let Err(e) = s.commit(&label, op) {
        // the library did not take the change: the files go back where they were
        let (_, note) = TrashStep::PutBack(taken).run(bin.as_ref());
        return Err(crate::EngineError::Other(match note {
            Some(note) => format!("{e}; {note}"),
            None => e.to_string(),
        }));
    }
    if let Some(entry) = s.undo.last_mut() {
        entry.trash = Some(TrashStep::PutBack(taken));
    }
    super::library::select_after_deletion(s, &before);
    Ok(json!({
        "removed": n,
        "trashed": trashed,
        "erased": erased,
        "missing": missing,
        "shared": shared,
        "noTrash": no_trash_ids,
        "failed": failed,
        "skipped": browsed,
    }))
}

/// `noTrash` as a command read it.
pub(crate) struct NoTrashParam(NoTrash);

impl NoTrashParam {
    pub(crate) fn read(p: &Value, c: &str) -> Result<NoTrashParam> {
        no_trash_param(p, c).map(NoTrashParam)
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "photo.trashFiles",
        "Move Files to Trash",
        [],
        None,
        "{ids?, noTrash?: keep|delete} → {removed, trashed, erased, missing, shared, noTrash: [ids], failed: [{id, path, why}], skipped} — the photos (the selection, or `ids`; in the library or in Recently Deleted) leave the library for good and their files, with their XMP sidecars, go to the system Trash; one undo step, and undo moves the files back when the Trash says where they are. `trashed` and `erased` count files. A file other photos of the library are (copies) stays with them (`shared`), as does a sidecar a photo beside it still uses; a file already gone is `missing`. Photos on a disk without a Trash stay, file and all, and are named in `noTrash`: only a second call with `noTrash: \"delete\"` erases their files for good (undo then brings the photos back as missing). A photo whose file can't be moved stays and is in `failed`; when nothing could go, that is the error. Photos only browsed in Local are skipped. Not available on Windows or in the browser yet. Delete, Delete Permanently, Empty Recently Deleted and Remove Folder from Library never touch a file",
        has_selection,
        |s, p| {
            const C: &str = "photo.trashFiles";
            let no_trash = NoTrashParam::read(p, C)?;
            if let Some(v) = p.get("ids").filter(|v| !v.is_null()) {
                let list = v.as_array().ok_or_else(|| bad(C, "`ids` must be a list of photo ids"))?;
                if let Some(x) = list.iter().find(|x| x.as_u64().is_none()) {
                    return Err(bad(C, format!("`ids` must be a list of photo ids, not {x}")));
                }
            }
            let _ = str_param(p, "noTrash");
            let ids = s.targets(p);
            trash_files(s, &ids, no_trash, C)
        }
    )]
}
