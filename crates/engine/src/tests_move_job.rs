//! Moving photos to another folder as a job the app runs off the UI thread (`move_job`), and the
//! record that lets the next open follow a move cut short (`move_intent`):
//!
//! * stopped part-way, the photos that moved stay moved (one undo step brings them back), the rest
//!   stay where they were;
//! * while a move runs, nothing else moves files or opens another library, and the moving photos'
//!   sidecars are written once they arrive, at the new place;
//! * the checks that read the disk refuse on the worker, before anything moves;
//! * a record is on disk before the first file moves and gone once the library is saved;
//! * a move cut short (the app killed) is followed the next time the library opens.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::rename::{MoveFs, RealFs};

/// A scratch folder with a library, `local/` and `share/`, gone with the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-movejob-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("local")).unwrap();
        std::fs::create_dir_all(dir.join("share")).unwrap();
        Scratch(dir)
    }
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().to_string()
    }
    fn library(&self) -> PathBuf {
        self.0.join("library")
    }
    fn records(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.library().join("moves"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &Scratch) -> Session {
    let mut s = Session::new();
    s.open_library(dir.library(), false).unwrap();
    s
}

/// A photo file `path` in the library (journaled, so it survives a reopen).
fn add(s: &mut Session, path: &str, body: &str) -> PhotoId {
    std::fs::write(path, body).unwrap();
    let id = s.catalog.alloc_photo_id();
    let name = Path::new(path).file_name().unwrap().to_string_lossy().to_string();
    let p = Photo::new(id, Source::File { path: path.into() }, &name, "JPEG", 60, 40, "2026-01-01T10:00:00");
    s.execute_fn("test.add", |s| {
        s.commit("Add", Op::AddPhoto { photo: Box::new(p) })?;
        Ok(Value::Null)
    })
    .unwrap();
    id
}

fn file_of(s: &Session, id: PhotoId) -> String {
    match &s.catalog.photo(id).unwrap().source {
        Source::File { path } => path.clone(),
        _ => panic!("not a file"),
    }
}

/// The disk, stopping the move (its activity row's ✕) once `after` photo files have moved.
struct StopAfter<'a> {
    s: &'a crate::activity::Activity,
    task: u64,
    after: Cell<usize>,
}

impl MoveFs for StopAfter<'_> {
    fn exists(&self, p: &Path) -> bool {
        RealFs.exists(p)
    }
    fn is_file(&self, p: &Path) -> bool {
        RealFs.is_file(p)
    }
    fn same_file(&self, a: &Path, b: &Path) -> bool {
        RealFs.same_file(a, b)
    }
    fn rename_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        RealFs.rename_no_replace(a, b)?;
        if a.extension().is_some_and(|x| x == "jpg") {
            let left = self.after.get().saturating_sub(1);
            self.after.set(left);
            if left == 0 {
                self.s.cancel(self.task).unwrap();
            }
        }
        Ok(())
    }
    fn copy_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        RealFs.copy_no_replace(a, b)
    }
    fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        RealFs.remove_file(p)
    }
    fn stem_sibling(&self, sidecar: &Path) -> Option<String> {
        RealFs.stem_sibling(sidecar)
    }
}

#[test]
fn a_stopped_move_keeps_what_moved_and_one_undo_moves_it_back() {
    let dir = Scratch::new("stop");
    let mut s = open(&dir);
    let a = add(&mut s, &dir.path("local/a.jpg"), "A");
    std::fs::write(dir.path("local/a.xmp"), "<edits/>").unwrap();
    let b = add(&mut s, &dir.path("local/b.jpg"), "B");
    let c = add(&mut s, &dir.path("local/c.jpg"), "C");
    let steps = s.undo.len();
    let (work, finish) = crate::move_job::start(&mut s, &json!({"ids": [a.0, b.0, c.0], "folder": dir.path("share")}), true).unwrap().unwrap();
    let row = s.activity.list().into_iter().find(|t| t.kind == "move").unwrap();
    assert!(row.cancellable, "the row has ✕");
    assert_eq!((row.label.as_str(), row.total), ("Moving photos", 3));
    let activity = s.activity.clone();
    let out = work.run_with(&StopAfter { s: &activity, task: finish.task_id(), after: Cell::new(2) });
    let r = s.execute_fn("photo.moveToFolder", |s| finish.finish(s, out)).unwrap();
    assert_eq!(r, json!({"moved": 2, "of": 3, "folder": dir.path("share"), "stopped": true}));
    // the two that moved point at the share, with a's sidecar; c never left
    assert_eq!(file_of(&s, a), dir.path("share/a.jpg"));
    assert_eq!(file_of(&s, b), dir.path("share/b.jpg"));
    assert_eq!(file_of(&s, c), dir.path("local/c.jpg"));
    assert!(Path::new(&dir.path("share/a.xmp")).is_file() && !Path::new(&dir.path("local/a.xmp")).exists());
    assert!(Path::new(&dir.path("local/c.jpg")).is_file() && !Path::new(&dir.path("share/c.jpg")).exists());
    assert!(s.activity.list().is_empty(), "the row is gone");
    assert!(!s.move_running());
    // one step, for what moved
    assert_eq!(s.undo.len(), steps + 1);
    assert_eq!(s.undo.last().unwrap().label, "Move 2 Photos to “share”");
    s.execute("edit.undo", &json!({})).unwrap();
    for (id, f) in [(a, "a"), (b, "b"), (c, "c")] {
        assert_eq!(file_of(&s, id), dir.path(&format!("local/{f}.jpg")));
        assert!(Path::new(&file_of(&s, id)).is_file());
    }
    assert!(Path::new(&dir.path("local/a.xmp")).is_file());
}

#[test]
fn a_running_move_holds_back_other_file_moves_and_another_library() {
    let dir = Scratch::new("busy");
    let mut s = open(&dir);
    let a = add(&mut s, &dir.path("local/a.jpg"), "A");
    let b = add(&mut s, &dir.path("local/b.jpg"), "B");
    // a step that moved files, to undo while the next move runs
    s.execute("photo.moveToFolder", &json!({"ids": [b.0], "folder": dir.path("share")})).unwrap();
    let (work, finish) = crate::move_job::start(&mut s, &json!({"ids": [a.0], "folder": dir.path("share")}), true).unwrap().unwrap();
    assert!(s.move_running());
    let busy = |r: crate::Result<Value>| {
        let e = r.unwrap_err().to_string();
        assert!(e.contains("being moved"), "{e}");
    };
    busy(s.execute("photo.moveToFolder", &json!({"ids": [b.0], "folder": dir.path("local")})));
    s.selection = crate::Selection::single(b);
    busy(s.execute("photo.rename", &json!({"ids": [b.0], "template": "renamed"})));
    busy(s.execute("folder.rename", &json!({"path": dir.path("share"), "name": "nas"})));
    busy(s.execute("edit.undo", &json!({})));
    let other = dir.0.join("other-library");
    let e = Session::open_library(&mut s, &other, false).map(|_| ()).unwrap_err().to_string();
    assert!(e.contains("being moved"), "{e}");
    // a switch checks before it closes anything: refused there, the library stays open as it is
    let e = s.check_library(&other).unwrap_err().to_string();
    assert!(e.contains("being moved"), "{e}");
    // an edit that moves no file still goes
    s.execute("photo.rate", &json!({"ids": [b.0], "rating": 3})).unwrap();
    let out = work.run();
    s.execute_fn("photo.moveToFolder", |s| finish.finish(s, out)).unwrap();
    // and once it's over, all of it works again
    s.execute("edit.undo", &json!({})).unwrap();
    s.open_library(&other, false).unwrap();
}

#[test]
fn a_moving_photos_sidecar_is_written_once_it_arrives_where_it_arrives() {
    let dir = Scratch::new("sidecar");
    let mut s = open(&dir);
    s.xmp.auto_write = true;
    let a = add(&mut s, &dir.path("local/a.jpg"), "A");
    let (work, finish) = crate::move_job::start(&mut s, &json!({"ids": [a.0], "folder": dir.path("share")}), true).unwrap().unwrap();
    s.selection = crate::Selection::single(a);
    s.execute("photo.rate", &json!({"ids": [a.0], "rating": 4})).unwrap();
    assert_eq!(s.catalog.photo(a).unwrap().rating, 4, "the edit is kept");
    assert!(!Path::new(&dir.path("local/a.xmp")).exists(), "nothing is written behind the moving file");
    assert_eq!(s.unsaved_sidecars(), 0, "held for the move, not a failed write to warn about");
    let out = work.run();
    s.execute_fn("photo.moveToFolder", |s| finish.finish(s, out)).unwrap();
    let xmp = std::fs::read_to_string(dir.path("share/a.xmp")).expect("written at the new place");
    assert!(xmp.contains("xmp:Rating=\"4\"") || xmp.contains("<xmp:Rating>4</xmp:Rating>"), "{xmp}");
    assert!(!Path::new(&dir.path("local/a.xmp")).exists());
    assert_eq!(s.unsaved_sidecars(), 0);
}

#[test]
fn a_move_whose_worker_dies_is_followed_and_its_held_sidecars_are_written() {
    let dir = Scratch::new("abandon");
    let mut s = open(&dir);
    s.xmp.auto_write = true;
    let a = add(&mut s, &dir.path("local/a.jpg"), "A");
    let (work, finish) = crate::move_job::start(&mut s, &json!({"ids": [a.0], "folder": dir.path("share")}), true).unwrap().unwrap();
    s.selection = crate::Selection::single(a);
    s.execute("photo.rate", &json!({"ids": [a.0], "rating": 5})).unwrap();
    // the files moved, then the worker died before saying so: no outcome reaches the session
    drop(work.run());
    let r = s.execute_fn("photo.moveToFolder", |s| Ok(json!(finish.abandon(s)))).unwrap();
    assert_eq!(r["relinked"], 1, "{r}");
    assert_eq!(file_of(&s, a), dir.path("share/a.jpg"), "the library follows the file");
    let xmp = std::fs::read_to_string(dir.path("share/a.xmp")).expect("the held sidecar is written where the photo is");
    assert!(xmp.contains("xmp:Rating=\"5\"") || xmp.contains("<xmp:Rating>5</xmp:Rating>"), "{xmp}");
    assert!(!s.move_running() && s.activity.list().is_empty());
    assert!(dir.records().is_empty(), "{:?}", dir.records());
}

#[test]
fn checks_that_read_the_disk_refuse_on_the_worker_before_anything_moves() {
    let dir = Scratch::new("refuse");
    let mut s = open(&dir);
    let a = add(&mut s, &dir.path("local/a.jpg"), "A");
    let b = add(&mut s, &dir.path("local/b.jpg"), "B");
    // the name is taken after the move was planned: the worker sees it
    let (work, finish) = crate::move_job::start(&mut s, &json!({"ids": [a.0, b.0], "folder": dir.path("share")}), true).unwrap().unwrap();
    std::fs::write(dir.path("share/b.jpg"), "someone else's").unwrap();
    let out = work.run();
    let e = s.execute_fn("photo.moveToFolder", |s| finish.finish(s, out)).unwrap_err().to_string();
    assert!(e.contains("b.jpg already in") && e.contains("nothing was moved"), "{e}");
    assert_eq!(file_of(&s, a), dir.path("local/a.jpg"));
    assert!(Path::new(&dir.path("local/a.jpg")).is_file() && !Path::new(&dir.path("share/a.jpg")).exists());
    assert_eq!(std::fs::read_to_string(dir.path("share/b.jpg")).unwrap(), "someone else's");
    assert!(dir.records().is_empty(), "no record for a move that never began");
    assert!(!s.move_running());
}

/// The disk, checking that the move's record is in the library before the first file moves.
struct Recorded<'a> {
    dir: &'a Scratch,
    seen: Cell<bool>,
}

impl MoveFs for Recorded<'_> {
    fn exists(&self, p: &Path) -> bool {
        RealFs.exists(p)
    }
    fn is_file(&self, p: &Path) -> bool {
        RealFs.is_file(p)
    }
    fn same_file(&self, a: &Path, b: &Path) -> bool {
        RealFs.same_file(a, b)
    }
    fn rename_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        if !self.seen.get() {
            let records = self.dir.records();
            assert_eq!(records.len(), 1, "the record is written before the first file moves");
            let text = std::fs::read_to_string(&records[0]).unwrap();
            assert!(text.contains("a.jpg") && text.contains("share"), "{text}");
            self.seen.set(true);
        }
        RealFs.rename_no_replace(a, b)
    }
    fn copy_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        RealFs.copy_no_replace(a, b)
    }
    fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        RealFs.remove_file(p)
    }
    fn stem_sibling(&self, sidecar: &Path) -> Option<String> {
        RealFs.stem_sibling(sidecar)
    }
}

#[test]
fn the_record_is_on_disk_before_the_first_file_moves_and_gone_once_saved() {
    let dir = Scratch::new("record");
    let mut s = open(&dir);
    let a = add(&mut s, &dir.path("local/a.jpg"), "A");
    let (work, finish) = crate::move_job::start(&mut s, &json!({"ids": [a.0], "folder": dir.path("share")}), true).unwrap().unwrap();
    let fs = Recorded { dir: &dir, seen: Cell::new(false) };
    let out = work.run_with(&fs);
    assert!(fs.seen.get());
    s.execute_fn("photo.moveToFolder", |s| finish.finish(s, out)).unwrap();
    assert!(dir.records().is_empty(), "{:?}", dir.records());
    // an Undo that moves files keeps a record the same way, and leaves none behind
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(file_of(&s, a), dir.path("local/a.jpg"));
    assert!(dir.records().is_empty(), "{:?}", dir.records());
}

#[test]
fn a_move_cut_short_is_followed_when_the_library_opens_again() {
    let dir = Scratch::new("crash");
    let (a, b, c, d, e, h);
    {
        let mut s = open(&dir);
        a = add(&mut s, &dir.path("local/a.jpg"), "A");
        std::fs::write(dir.path("local/a.xmp"), "<a-edits/>").unwrap();
        b = add(&mut s, &dir.path("local/b.jpg"), "B");
        c = add(&mut s, &dir.path("local/c.jpg"), "C");
        d = add(&mut s, &dir.path("local/d.jpg"), "D");
        std::fs::create_dir_all(dir.path("usb")).unwrap();
        e = add(&mut s, &dir.path("usb/e.jpg"), "E");
        h = add(&mut s, &dir.path("local/h.jpg"), "H");
        let mut moves: Vec<(String, String)> =
            ["a", "b", "c", "d"].iter().map(|f| (dir.path(&format!("local/{f}.jpg")), dir.path(&format!("share/{f}.jpg")))).collect();
        moves.push((dir.path("usb/e.jpg"), dir.path("share/e.jpg")));
        moves.push((dir.path("local/h.jpg"), dir.path("share/h.jpg")));
        crate::move_intent::write(&dir.library(), &moves).unwrap();
        // killed part-way: a's file arrived and its sidecar didn't; b's copy arrived and the
        // original wasn't removed yet; c never started; d's disk isn't connected now; e's copy
        // arrived (maybe never checked) and its drive is unplugged now
        std::fs::rename(dir.path("local/a.jpg"), dir.path("share/a.jpg")).unwrap();
        std::fs::copy(dir.path("local/b.jpg"), dir.path("share/b.jpg")).unwrap();
        std::fs::write(dir.path("share/.b.jpg.lc-partial-1-0"), "cut off").unwrap();
        std::fs::remove_file(dir.path("local/d.jpg")).unwrap();
        std::fs::copy(dir.path("usb/e.jpg"), dir.path("share/e.jpg")).unwrap();
        std::fs::rename(dir.path("usb"), dir.path("usb-unplugged")).unwrap();
        // h: cut between linking the new name and unlinking the old (same volume)
        std::fs::hard_link(dir.path("local/h.jpg"), dir.path("share/h.jpg")).unwrap();
        // (the app is gone: no finish, no save of a move)
    }
    let mut s = open(&dir);
    assert_eq!(file_of(&s, a), dir.path("share/a.jpg"), "the library follows the file that arrived");
    assert_eq!(std::fs::read_to_string(dir.path("share/a.xmp")).unwrap(), "<a-edits/>", "and its sidecar follows it");
    assert!(!Path::new(&dir.path("local/a.xmp")).exists());
    assert_eq!(file_of(&s, b), dir.path("local/b.jpg"), "the original stays where the library has it");
    assert!(!Path::new(&dir.path("share/b.jpg")).exists(), "our copy of it is removed");
    assert!(!Path::new(&dir.path("share/.b.jpg.lc-partial-1-0")).exists(), "a partial copy is removed");
    assert_eq!(file_of(&s, c), dir.path("local/c.jpg"));
    assert_eq!(file_of(&s, d), dir.path("local/d.jpg"));
    assert_eq!(file_of(&s, e), dir.path("usb/e.jpg"), "a source whose folder is gone may only be unplugged: not followed");
    assert_eq!(file_of(&s, h), dir.path("local/h.jpg"));
    assert!(Path::new(&dir.path("local/h.jpg")).is_file() && !Path::new(&dir.path("share/h.jpg")).exists(), "the second name goes, the file stays");
    let notice = s.take_library_warnings().join(" ");
    assert!(notice.contains("1 photo was being moved") && notice.contains("2 files that were being moved"), "{notice}");
    // d's and e's moves are kept for the next open; the rest of the record is done
    let records = dir.records();
    assert_eq!(records.len(), 1);
    let text = std::fs::read_to_string(&records[0]).unwrap();
    assert!(text.contains("d.jpg") && text.contains("e.jpg") && !text.contains("a.jpg"), "{text}");
    // the relink is one undo step that moves the file back
    assert_eq!(s.undo.last().unwrap().label, "Finish Moving 1 Photo");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(file_of(&s, a), dir.path("local/a.jpg"));
    assert!(Path::new(&dir.path("local/a.jpg")).is_file());
    drop(s);
    // and the relink was saved: it survives the next open, which has nothing more to do for a
    let s = open(&dir);
    assert_eq!(file_of(&s, a), dir.path("local/a.jpg"));
}
