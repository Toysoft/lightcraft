//! Moving photos' files to the Trash (`photo.trashFiles`): the one place where taking a photo
//! out of the library also touches its file. Delete, Delete Permanently, Empty Recently Deleted
//! and Remove Folder from Library never do.
//!
//! The tests use a Trash of their own (a folder in a scratch directory): none of them puts
//! anything in the real one.
//!
//! Scenarios:
//! - Given a photo with a file and an XMP sidecar, when its files are moved to the Trash, then
//!   the photo is out of the library (not in Recently Deleted either) and both files are in the
//!   Trash, as one undo step; undo puts the files back where they were and the photo back with
//!   its rating; redo moves them to the Trash again.
//! - Photos in Recently Deleted go the same way, one by one or all at once
//!   (`library.emptyRecentlyDeleted {files: "trash"}`).
//! - Delete, Delete Permanently, Empty Recently Deleted and Remove Folder from Library still
//!   leave every file where it is.
//! - Given two photos of the library that are one file (a copy), then the file stays while one
//!   of them is in the library and goes with the last.
//! - Given a raw and a JPEG that share one sidecar (`IMG.xmp`), then the sidecar stays with the
//!   one that stays.
//! - Given a disk without a Trash, then its photos stay in the library and are named in the
//!   answer; only `noTrash: "delete"` erases their files, and then undo brings the photos back
//!   as missing.
//! - A file that can't be moved to the Trash stays, with its photo, and the answer says why;
//!   the others go.
//! - A photo whose file is already gone leaves the library; nothing is moved.
//! - Undo when a file is no longer in the Trash, or when something else is now at its place:
//!   the photo comes back, nothing is overwritten, and the answer says what could not be put back.
//! - Photos only browsed in Local are not touched; photos without a file just leave the library.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::trash::{Bin, BinError};

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-trash-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        Scratch(dir)
    }
    fn file(&self, rel: &str, contents: &str) -> String {
        let p = self.0.join("photos").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p.to_string_lossy().to_string()
    }
    fn at(&self, rel: &str) -> String {
        self.0.join("photos").join(rel).to_string_lossy().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Trash that is a folder: files moved to it get a numbered name. Paths containing `nas/` are
/// on a disk without a Trash; paths containing `locked` can't be moved.
struct FolderBin {
    dir: PathBuf,
    next: Mutex<u32>,
}

impl Bin for FolderBin {
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, BinError> {
        let name = path.to_string_lossy().to_string();
        if name.contains("nas/") {
            return Err(BinError::NoTrash);
        }
        if name.contains("locked") {
            return Err(BinError::Failed("the file is locked".into()));
        }
        let mut n = self.next.lock().unwrap();
        *n += 1;
        let to = self.dir.join(format!("{}-{}", *n, path.file_name().unwrap().to_string_lossy()));
        std::fs::rename(path, &to).map_err(|e| BinError::Failed(e.to_string()))?;
        Ok(Some(to))
    }
}

fn session(dir: &Scratch) -> Session {
    let mut s = Session::new();
    s.trash = Arc::new(FolderBin { dir: dir.0.join("bin"), next: Mutex::new(0) });
    s
}

fn add(s: &mut Session, path: &str) -> u64 {
    let id = s.catalog.alloc_photo_id();
    let name = Path::new(path).file_name().unwrap().to_string_lossy().to_string();
    let p = Photo::new(id, Source::File { path: path.into() }, &name, "JPEG", 60, 40, "2026-01-01T10:00:00");
    s.commit("Add Photo", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id.0
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

fn in_bin(dir: &Scratch) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.0.join("bin")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

fn has(s: &Session, id: u64) -> bool {
    s.catalog.photo(PhotoId(id)).is_some()
}

/// (Photo commands want a selection to be enabled, whatever `ids` says.)
fn run(s: &mut Session, command: &str, ids: &[u64], more: Value) -> crate::Result<Value> {
    select(s, ids);
    let mut params = json!({"ids": ids});
    if let (Some(p), Some(m)) = (params.as_object_mut(), more.as_object()) {
        p.extend(m.clone());
    }
    s.execute(command, &params)
}

fn trash(s: &mut Session, ids: &[u64]) -> Value {
    run(s, "photo.trashFiles", ids, json!({})).unwrap()
}

fn select(s: &mut Session, ids: &[u64]) {
    s.selection.ids = ids.iter().map(|i| PhotoId(*i)).collect();
    s.selection.active = s.selection.ids.first().copied();
}

#[test]
fn a_photo_s_file_and_sidecar_go_to_the_trash_and_undo_puts_them_back() {
    let dir = Scratch::new("basic");
    let (a, b) = (dir.file("a.jpg", "A"), dir.file("b.jpg", "B"));
    let sidecar = dir.file("a.xmp", "<xmp/>");
    let mut s = session(&dir);
    let (ia, ib) = (add(&mut s, &a), add(&mut s, &b));
    select(&mut s, &[ia]);
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    let steps = s.undo.len();
    let r = trash(&mut s, &[ia]);
    assert_eq!((r["removed"].as_u64(), r["trashed"].as_u64()), (Some(1), Some(2)), "{r}");
    assert!(!has(&s, ia) && has(&s, ib), "out of the library, not in Recently Deleted");
    assert!(!exists(&a) && !exists(&sidecar) && exists(&b));
    assert_eq!(in_bin(&dir), vec!["1-a.jpg", "2-a.xmp"]);
    assert_eq!(s.undo.len(), steps + 1, "one step");
    assert_eq!(s.undo.last().unwrap().label, "Move Photo's Files to Trash");
    // undo: the files where they were, the photo as it was
    let undone = s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(undone["undone"], "Move Photo's Files to Trash");
    assert!(exists(&a) && exists(&sidecar) && in_bin(&dir).is_empty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "A");
    assert_eq!(s.catalog.photo(PhotoId(ia)).unwrap().rating, 4);
    assert!(!s.catalog.photo(PhotoId(ia)).unwrap().deleted);
    // redo: to the Trash again; undo once more brings them back again
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(!has(&s, ia) && !exists(&a) && !exists(&sidecar));
    assert_eq!(in_bin(&dir).len(), 2);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(has(&s, ia) && exists(&a) && exists(&sidecar));
}

#[test]
fn several_photos_go_as_one_step_and_the_selection_is_what_goes_without_ids() {
    let dir = Scratch::new("several");
    let paths: Vec<String> = (1..=3).map(|i| dir.file(&format!("p{i}.jpg"), "x")).collect();
    let mut s = session(&dir);
    let ids: Vec<u64> = paths.iter().map(|p| add(&mut s, p)).collect();
    select(&mut s, &[ids[0], ids[2]]);
    let steps = s.undo.len();
    let r = s.execute("photo.trashFiles", &json!({})).unwrap();
    assert_eq!(r["removed"], 2);
    assert!(!exists(&paths[0]) && exists(&paths[1]) && !exists(&paths[2]));
    assert_eq!(s.undo.len(), steps + 1);
    assert_eq!(s.undo.last().unwrap().label, "Move 2 Photos' Files to Trash");
    assert_eq!(s.selection.active, Some(PhotoId(ids[1])), "the photo left is the one selected");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(paths.iter().all(|p| exists(p)));
}

#[test]
fn photos_in_recently_deleted_go_too_one_by_one_or_all_at_once() {
    let dir = Scratch::new("deleted");
    let paths: Vec<String> = (1..=3).map(|i| dir.file(&format!("p{i}.jpg"), "x")).collect();
    let mut s = session(&dir);
    let ids: Vec<u64> = paths.iter().map(|p| add(&mut s, p)).collect();
    run(&mut s, "photo.delete", &ids, json!({})).unwrap();
    assert!(paths.iter().all(|p| exists(p)), "Delete touches no file");
    trash(&mut s, &[ids[0]]);
    assert!(!has(&s, ids[0]) && !exists(&paths[0]) && exists(&paths[1]));
    let steps = s.undo.len();
    let r = s.execute("library.emptyRecentlyDeleted", &json!({"files": "trash"})).unwrap();
    assert_eq!((r["deleted"].as_u64(), r["trashed"].as_u64()), (Some(2), Some(2)), "{r}");
    assert!(!exists(&paths[1]) && !exists(&paths[2]));
    assert_eq!(s.undo.len(), steps + 1);
    // undo: back in Recently Deleted, files back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(exists(&paths[1]) && s.catalog.photo(PhotoId(ids[1])).unwrap().deleted);
    assert!(s.execute("library.emptyRecentlyDeleted", &json!({"files": "shred"})).is_err());
}

#[test]
fn the_other_ways_out_of_the_library_still_leave_every_file() {
    let dir = Scratch::new("untouched");
    let paths: Vec<String> = (1..=4).map(|i| dir.file(&format!("d{i}/p.jpg"), "x")).collect();
    let mut s = session(&dir);
    let ids: Vec<u64> = paths.iter().map(|p| add(&mut s, p)).collect();
    run(&mut s, "photo.delete", &[ids[0], ids[1], ids[2]], json!({})).unwrap();
    run(&mut s, "photo.deletePermanently", &[ids[0]], json!({})).unwrap();
    s.execute("library.emptyRecentlyDeleted", &json!({})).unwrap();
    s.execute("library.removeFolder", &json!({"path": dir.at("d4")})).unwrap();
    assert!(paths.iter().all(|p| exists(p)), "no file was touched");
    assert!(in_bin(&dir).is_empty());
}

#[test]
fn a_file_two_photos_share_stays_until_the_last_of_them_goes() {
    let dir = Scratch::new("shared");
    let a = dir.file("a.jpg", "A");
    let mut s = session(&dir);
    let (one, two) = (add(&mut s, &a), add(&mut s, &a));
    let r = trash(&mut s, &[one]);
    assert_eq!((r["removed"].as_u64(), r["trashed"].as_u64(), r["shared"].as_u64()), (Some(1), Some(0), Some(1)), "{r}");
    assert!(!has(&s, one) && has(&s, two) && exists(&a), "the other photo still uses the file");
    let r = trash(&mut s, &[two]);
    assert_eq!(r["trashed"], 1);
    assert!(!exists(&a));
    // both at once: the file goes once
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(has(&s, one) && has(&s, two) && exists(&a));
    let r = trash(&mut s, &[one, two]);
    assert_eq!((r["removed"].as_u64(), r["trashed"].as_u64()), (Some(2), Some(1)), "{r}");
    assert!(!exists(&a));
}

#[test]
fn a_sidecar_shared_by_a_raw_and_a_jpeg_stays_with_the_one_that_stays() {
    let dir = Scratch::new("pair");
    let (raw, jpg) = (dir.file("IMG_1.dng", "R"), dir.file("IMG_1.jpg", "J"));
    let (shared, own) = (dir.file("IMG_1.xmp", "<xmp/>"), dir.file("IMG_1.jpg.xmp", "<own/>"));
    let mut s = session(&dir);
    let (ir, ij) = (add(&mut s, &raw), add(&mut s, &jpg));
    trash(&mut s, &[ij]);
    assert!(!exists(&jpg) && !exists(&own), "the JPEG and the sidecar that is its alone");
    assert!(exists(&raw) && exists(&shared), "the raw keeps the sidecar they shared");
    trash(&mut s, &[ir]);
    assert!(!exists(&raw) && !exists(&shared), "with the last of them the sidecar goes");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(exists(&raw) && exists(&shared));
}

#[test]
fn photos_on_a_disk_without_a_trash_stay_unless_their_files_may_be_erased() {
    let dir = Scratch::new("far-disk");
    let (near, far) = (dir.file("a.jpg", "A"), dir.file("nas/b.jpg", "B"));
    let far_sidecar = dir.file("nas/b.xmp", "<xmp/>");
    let mut s = session(&dir);
    let (ia, ib) = (add(&mut s, &near), add(&mut s, &far));
    let r = trash(&mut s, &[ia, ib]);
    assert_eq!(r["removed"], 1);
    assert_eq!(r["noTrash"], json!([ib]), "named, so the question can be asked about them");
    assert!(has(&s, ib) && exists(&far) && exists(&far_sidecar), "it stays, file and all");
    assert!(!has(&s, ia) && !exists(&near));
    // asked for by name only: erased for good
    assert!(run(&mut s, "photo.trashFiles", &[ib], json!({"noTrash": "shred"})).is_err());
    assert!(has(&s, ib) && exists(&far));
    let steps = s.undo.len();
    let r = run(&mut s, "photo.trashFiles", &[ib], json!({"noTrash": "delete"})).unwrap();
    assert_eq!((r["removed"].as_u64(), r["erased"].as_u64(), r["trashed"].as_u64()), (Some(1), Some(2), Some(0)), "{r}");
    assert!(!has(&s, ib) && !exists(&far) && !exists(&far_sidecar));
    assert_eq!(in_bin(&dir), vec!["1-a.jpg"], "nothing of it is in the Trash");
    assert_eq!(s.undo.len(), steps + 1);
    assert_eq!(s.undo.last().unwrap().label, "Erase Photo's Files");
    // undo brings the photo back (its edits and metadata are kept); the file stays gone
    let undone = s.execute("edit.undo", &json!({})).unwrap()["undone"].as_str().unwrap().to_string();
    assert!(has(&s, ib) && !exists(&far));
    assert!(undone.contains("erased"), "undo says the file is not coming back: {undone}");
}

#[test]
fn a_file_that_can_t_go_to_the_trash_stays_with_its_photo_and_the_answer_says_why() {
    let dir = Scratch::new("stuck");
    let (ok, locked) = (dir.file("a.jpg", "A"), dir.file("locked.jpg", "L"));
    let mut s = session(&dir);
    let (ia, il) = (add(&mut s, &ok), add(&mut s, &locked));
    let r = trash(&mut s, &[il, ia]);
    assert_eq!(r["removed"], 1);
    assert_eq!(r["failed"][0]["id"], il);
    assert!(r["failed"][0]["why"].as_str().unwrap().contains("locked"), "{r}");
    assert!(has(&s, il) && exists(&locked) && !has(&s, ia) && !exists(&ok));
    // nothing could go: an error that says why, and nothing recorded to undo
    let steps = s.undo.len();
    let e = run(&mut s, "photo.trashFiles", &[il], json!({})).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    assert_eq!(s.undo.len(), steps);
    // …and erasing is never the answer to that: `noTrash` is for disks without a Trash only
    assert!(run(&mut s, "photo.trashFiles", &[il], json!({"noTrash": "delete"})).is_err());
    assert!(exists(&locked));
}

#[test]
fn a_photo_whose_file_is_gone_leaves_the_library_and_nothing_is_moved() {
    let dir = Scratch::new("missing");
    let mut s = session(&dir);
    let gone = add(&mut s, &dir.at("gone.jpg"));
    let r = trash(&mut s, &[gone]);
    assert_eq!((r["removed"].as_u64(), r["trashed"].as_u64(), r["missing"].as_u64()), (Some(1), Some(0), Some(1)), "{r}");
    assert!(!has(&s, gone) && in_bin(&dir).is_empty());
}

#[test]
fn undo_never_overwrites_and_says_what_could_not_be_put_back() {
    let dir = Scratch::new("undo");
    let (a, b, c) = (dir.file("a.jpg", "A"), dir.file("b.jpg", "B"), dir.file("c.jpg", "C"));
    let mut s = session(&dir);
    let ids = [add(&mut s, &a), add(&mut s, &b), add(&mut s, &c)];
    trash(&mut s, &ids);
    // the Trash was emptied of one, and something else is now where another was
    std::fs::remove_file(dir.0.join("bin/1-a.jpg")).unwrap();
    std::fs::write(&b, "NEW").unwrap();
    let undone = s.execute("edit.undo", &json!({})).unwrap()["undone"].as_str().unwrap().to_string();
    assert!(ids.iter().all(|id| has(&s, *id)), "the photos are back in the library");
    assert!(exists(&c) && !exists(&a));
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "NEW", "what is there now is not replaced");
    assert!(dir.0.join("bin/2-b.jpg").exists(), "and the trashed file is still in the Trash");
    assert!(undone.contains("2 files could not be put back"), "{undone}");
    // redo takes away what undo put back (and the photos); what never came back is not touched
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(ids.iter().all(|id| !has(&s, *id)));
    assert!(!exists(&c));
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "NEW", "a file that is not the photo's own stays");
}

#[test]
fn browsed_photos_are_left_alone_and_photos_without_a_file_just_leave() {
    let dir = Scratch::new("local");
    let local_path = dir.file("local.jpg", "L");
    let mut s = session(&dir);
    let id = s.catalog.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: local_path.clone() }, "local.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00");
    p.local = true;
    s.commit("Browse", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let e = run(&mut s, "photo.trashFiles", &[id.0], json!({})).unwrap_err().to_string();
    assert!(e.contains("My Photos"), "{e}");
    assert!(exists(&local_path) && has(&s, id.0));
    // a photo that is no file (a demo scene)
    let demo = s.catalog.alloc_photo_id();
    let p = Photo::new(demo, Source::Demo { scene: 1 }, "demo.jpg", "JPEG", 60, 40, "2026-01-01T10:00:00");
    s.commit("Add Photo", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let r = trash(&mut s, &[demo.0]);
    assert_eq!((r["removed"].as_u64(), r["trashed"].as_u64()), (Some(1), Some(0)));
    // nothing named, nothing selected; a photo that is not there
    select(&mut s, &[id.0]);
    let e = s.execute("photo.trashFiles", &json!({"ids": []})).unwrap_err().to_string();
    assert!(e.contains("no photos"), "{e}");
    let e = s.execute("photo.trashFiles", &json!({"ids": [4242]})).unwrap_err().to_string();
    assert!(e.contains("4242"), "{e}");
    assert!(s.execute("photo.trashFiles", &json!({"ids": "all"})).is_err());
    assert!(exists(&local_path));
}
