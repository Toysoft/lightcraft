//! Locating a library folder that is somewhere else now (see [`crate::relocate`]; issue #1112).
//!
//! Scenarios, in the words of someone whose folder is not where the library looks for it:
//!
//! * Given a folder of the library that was moved or renamed outside LightCraft (or whose
//!   catalog came from another computer), when the person says where it is now, its photos —
//!   those in the folders inside it and in Recently Deleted too — point at the same places below
//!   the new folder. No file is touched, and it is one undo step.
//! * A path written by Windows (`D:\Photos\…`) is located on a computer that spells paths `/…`.
//! * What the library knows about the folder (its colour label) and the folder being shown
//!   follow; undo brings both back.
//! * A folder that holds none of the photos' files is refused: it is not the folder. One that
//!   holds only some is refused unless the person says so; the others then stay missing there.
//!   Looking for the files can run apart from the session, and can be stopped.
//! * A folder whose files the library already has is refused, as are the folder itself, the
//!   library's own folder, a path that is no folder, and a folder that is not the library's.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use lightcraft_catalog::{ColorLabel, Op, PhotoId, Source};
use serde_json::{Value, json};

use crate::Session;
use crate::relocate::{Relocation, count_found};

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-relocate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).components().collect::<PathBuf>().to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A small procedural PNG (distinct per seed).
fn write_png(path: &str, seed: u8) {
    let (w, h) = (24usize, 16usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 9) as u8, (i / w * 13) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A library holding `trip/a.png` and `trip/day1/b.png`.
fn library(dir: &Scratch) -> Session {
    write_png(&dir.path("trip/a.png"), 1);
    write_png(&dir.path("trip/day1/b.png"), 2);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.path("trip")]})).unwrap();
    assert_eq!(s.catalog.len(), 2);
    s
}

/// The folder moved away whole, outside LightCraft: `trip` is now `archive/journey`.
fn moved_away(dir: &Scratch) -> String {
    std::fs::create_dir_all(dir.0.join("archive")).unwrap();
    std::fs::rename(dir.0.join("trip"), dir.0.join("archive/journey")).unwrap();
    dir.path("archive/journey")
}

/// Where the library looks for each photo's file.
fn files(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            Source::File { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

fn relocate(s: &mut Session, from: &str, to: &str) -> Value {
    s.execute("folder.updateLocation", &json!({"folder": from, "path": to})).unwrap()
}

fn refused(s: &mut Session, params: Value) -> String {
    let (before, steps) = (files(s), s.undo.len());
    let e = s.execute("folder.updateLocation", &params).unwrap_err().to_string();
    assert_eq!(files(s), before, "a refusal changes nothing");
    assert_eq!(s.undo.len(), steps, "and leaves no undo step");
    e
}

#[test]
fn a_folder_moved_outside_lightcraft_is_located_and_its_photos_follow() {
    let dir = Scratch::new("follow");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    assert_eq!(s.execute("library.missing", &json!({})).unwrap().as_array().unwrap().len(), 2, "both are missing first");

    let r = relocate(&mut s, &dir.path("trip"), &now);

    assert_eq!((r["relinked"].as_u64(), r["found"].as_u64(), r["notFound"].as_u64()), (Some(2), Some(2), Some(0)), "{r}");
    assert_eq!(r["path"], json!(now));
    assert_eq!(files(&s), [dir.path("archive/journey/a.png"), dir.path("archive/journey/day1/b.png")]);
    assert!(s.execute("library.missing", &json!({})).unwrap().as_array().unwrap().is_empty(), "nothing is missing any more");
    let scan = s.execute("folder.scanChanges", &json!({"path": now})).unwrap();
    for key in ["new", "missing", "moved", "metadata"] {
        assert!(scan[key].as_array().unwrap().is_empty(), "{key}: the library is up to date with the folder: {scan}");
    }
}

#[test]
fn locating_is_one_undo_step_and_touches_no_file() {
    let dir = Scratch::new("undo");
    let mut s = library(&dir);
    let before = files(&s);
    let now = moved_away(&dir);
    let steps = s.undo.len();

    relocate(&mut s, &dir.path("trip"), &now);
    assert_eq!(s.undo.len(), steps + 1);
    let after = files(&s);

    s.undo_step().unwrap();
    assert_eq!(files(&s), before, "undo points the photos back");
    assert!(Path::new(&now).join("a.png").is_file() && !dir.0.join("trip").exists(), "and moves nothing on disk");
    s.redo_step().unwrap();
    assert_eq!(files(&s), after);
    assert!(Path::new(&now).join("day1/b.png").is_file() && !dir.0.join("trip").exists());
}

/// Point every photo of the library at the same place below `root`, as a catalog from
/// another computer would have it.
fn as_written_elsewhere(s: &mut Session, dir: &Scratch, root: &str, sep: &str) {
    let ops = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            Source::File { path } => {
                let rel: Vec<String> =
                    Path::new(path).strip_prefix(&dir.0).unwrap().components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                Some(Op::Relink {
                    id: p.id,
                    file_name: p.file_name.clone(),
                    source: Source::File { path: format!("{root}{}", rel.join(sep)) },
                    format: None,
                })
            }
            _ => None,
        })
        .collect();
    s.commit("elsewhere", Op::Batch { ops }).unwrap();
}

#[test]
fn a_folder_of_a_windows_catalog_is_located_on_this_computer() {
    let dir = Scratch::new("windows");
    let mut s = library(&dir);
    as_written_elsewhere(&mut s, &dir, "D:\\Photos\\", "\\");
    assert_eq!(files(&s), ["D:\\Photos\\trip\\a.png", "D:\\Photos\\trip\\day1\\b.png"]);

    let r = relocate(&mut s, "D:\\Photos\\trip", &dir.path("trip"));

    assert_eq!(r["relinked"], json!(2), "{r}");
    assert_eq!(files(&s), [dir.path("trip/a.png"), dir.path("trip/day1/b.png")], "spelled the way this computer spells paths");
}

#[test]
fn a_whole_disk_is_located_only_on_request() {
    let dir = Scratch::new("disk");
    let mut s = library(&dir);
    as_written_elsewhere(&mut s, &dir, "D:\\", "\\");

    let e = refused(&mut s, json!({"folder": "D:\\", "path": dir.path("")}));
    assert!(e.contains("disk: true"), "{e}");

    let r = s.execute("folder.updateLocation", &json!({"folder": "D:\\", "path": dir.path(""), "disk": true})).unwrap();
    assert_eq!(r["relinked"], json!(2), "{r}");
    assert_eq!(files(&s), [dir.path("trip/a.png"), dir.path("trip/day1/b.png")]);
}

#[test]
fn photos_in_recently_deleted_follow_their_folder() {
    let dir = Scratch::new("trash");
    let mut s = library(&dir);
    let id = s.catalog.photos().find(|p| p.file_name == "a.png").map(|p| p.id).unwrap();
    s.execute("photo.delete", &json!({"ids": [id.0]})).unwrap();
    let now = moved_away(&dir);

    let r = relocate(&mut s, &dir.path("trip"), &now);

    // (the one in the trash is not looked for, but restoring it must find its file)
    assert_eq!((r["relinked"].as_u64(), r["found"].as_u64()), (Some(2), Some(1)), "{r}");
    assert_eq!(files(&s), [dir.path("archive/journey/a.png"), dir.path("archive/journey/day1/b.png")]);
}

#[test]
fn the_folders_colour_labels_follow() {
    let dir = Scratch::new("labels");
    let mut s = library(&dir);
    s.execute("folder.label", &json!({"path": dir.path("trip"), "label": "red"})).unwrap();
    s.execute("folder.label", &json!({"path": dir.path("trip/day1"), "label": "blue"})).unwrap();
    let now = moved_away(&dir);

    relocate(&mut s, &dir.path("trip"), &now);

    assert_eq!(s.catalog.folder_color_label(&now), Some(ColorLabel::Red));
    assert_eq!(s.catalog.folder_color_label(&dir.path("archive/journey/day1")), Some(ColorLabel::Blue));
    assert_eq!(s.catalog.folder_color_label(&dir.path("trip")), None, "nothing is left at the old place");
    s.undo_step().unwrap();
    assert_eq!(s.catalog.folder_color_label(&dir.path("trip")), Some(ColorLabel::Red), "undo brings the labels back with the photos");
    assert_eq!(s.catalog.folder_color_label(&now), None);
}

#[test]
fn the_folder_being_shown_follows_and_comes_back_on_undo() {
    let dir = Scratch::new("shown");
    let mut s = library(&dir);
    s.execute("library.source", &json!({"kind": "libraryFolder", "path": dir.path("trip/day1")})).unwrap();
    let now = moved_away(&dir);

    relocate(&mut s, &dir.path("trip"), &now);
    assert_eq!(s.library_folder.as_deref(), Some(dir.path("archive/journey/day1").as_str()));
    assert_eq!(s.visible().len(), 1, "its photo is still the one shown");

    s.undo_step().unwrap();
    assert_eq!(s.library_folder.as_deref(), Some(dir.path("trip/day1").as_str()));
    s.redo_step().unwrap();
    assert_eq!(s.library_folder.as_deref(), Some(dir.path("archive/journey/day1").as_str()));
}

#[test]
fn a_folder_that_holds_none_of_the_photos_is_refused() {
    let dir = Scratch::new("none");
    let mut s = library(&dir);
    moved_away(&dir);
    // one level too high: the files are in `archive/journey`, not in `archive`
    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": dir.path("archive")}));
    assert!(e.contains("none of the 2 files"), "{e}");
    // and saying "anyway" does not make it the folder
    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": dir.path("archive"), "incomplete": true}));
    assert!(e.contains("none of the 2 files"), "{e}");
}

#[test]
fn a_folder_that_holds_only_some_is_located_only_on_request() {
    let dir = Scratch::new("some");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    std::fs::remove_file(Path::new(&now).join("a.png")).unwrap();

    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": now}));
    assert!(e.contains("1 of the 2 files") && e.contains("incomplete: true"), "{e}");

    let r = s.execute("folder.updateLocation", &json!({"folder": dir.path("trip"), "path": now, "incomplete": true})).unwrap();
    assert_eq!((r["relinked"].as_u64(), r["found"].as_u64(), r["notFound"].as_u64()), (Some(2), Some(1), Some(1)), "{r}");
    // the one that isn't there is missing at the new place, where Synchronize Folder finds it
    let missing = s.execute("library.missing", &json!({})).unwrap();
    assert_eq!(missing.as_array().unwrap().len(), 1);
    assert_eq!(missing[0]["path"], json!(dir.path("archive/journey/a.png")));
}

#[test]
fn a_folder_whose_files_the_library_already_has_is_refused() {
    let dir = Scratch::new("taken");
    let mut s = library(&dir);
    write_png(&dir.path("copy/a.png"), 3);
    write_png(&dir.path("copy/day1/b.png"), 4);
    s.execute("library.import", &json!({"paths": [dir.path("copy")]})).unwrap();
    assert_eq!(s.catalog.len(), 4);
    std::fs::remove_dir_all(dir.0.join("trip")).unwrap();

    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": dir.path("copy")}));
    assert!(e.contains("already in the library"), "{e}");
}

#[test]
fn only_a_missing_or_moved_library_folder_and_a_real_folder_are_taken() {
    let dir = Scratch::new("refusals");
    let mut s = library(&dir);
    let trip = dir.path("trip");
    std::fs::create_dir_all(dir.0.join("empty")).unwrap();

    for (params, why) in [
        (json!({"path": trip}), "missing `folder`"),
        (json!({"folder": trip}), "missing `path`"),
        (json!({"folder": trip, "path": trip}), "already looks"),
        (json!({"folder": trip, "path": format!("{trip}{}", std::path::MAIN_SEPARATOR)}), "already looks"),
        (json!({"folder": trip, "path": dir.path("nowhere")}), "is not a folder"),
        (json!({"folder": trip, "path": dir.path("trip/a.png")}), "is not a folder"),
        (json!({"folder": dir.path("empty"), "path": trip}), "no photo in the library"),
        (json!({"folder": "/", "path": trip}), "startup disk"),
    ] {
        let e = refused(&mut s, params.clone());
        assert!(e.contains(why), "{params}: {e}");
    }
}

#[test]
fn the_librarys_own_folder_is_no_place_for_photos() {
    let dir = Scratch::new("own");
    write_png(&dir.path("trip/a.png"), 1);
    let lib = dir.path("lib");
    let mut s = Session::new().with_fs();
    s.open_library(Path::new(&lib), false).unwrap();
    s.execute("library.import", &json!({"paths": [dir.path("trip")]})).unwrap();
    std::fs::create_dir_all(dir.0.join("lib/inside")).unwrap();

    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": dir.path("lib/inside")}));
    assert!(e.contains("library's own"), "{e}");
}

#[test]
fn the_plan_names_each_file_once_and_the_count_can_be_stopped() {
    let dir = Scratch::new("count");
    let mut s = library(&dir);
    // a virtual copy shares its photo's file: one file to look for
    let id = s.catalog.photos().find(|p| p.file_name == "a.png").map(|p| p.id).unwrap();
    s.execute("photo.virtualCopy", &json!({"ids": [id.0]})).unwrap();
    let now = moved_away(&dir);

    let plan = Relocation::new(&s, &dir.path("trip"), &now, false).unwrap();
    assert_eq!(plan.files().len(), 2, "{:?}", plan.files());
    let progress = crate::import::ScanProgress::default();
    assert_eq!(count_found(&plan.files(), &progress), 2);
    assert_eq!((progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed)), (2, 2));

    // ✕ on its row in the activity stack
    let stopped = crate::import::ScanProgress::default();
    stopped.cancel.store(true, Ordering::Relaxed);
    assert_eq!(count_found(&plan.files(), &stopped), 0, "nothing more is looked for");

    let r = relocate(&mut s, &dir.path("trip"), &now);
    assert_eq!((r["relinked"].as_u64(), r["found"].as_u64()), (Some(3), Some(2)), "the copy follows its photo: {r}");
    assert!(s.catalog.photo(PhotoId(id.0)).is_some());
}

#[test]
fn hostile_parameters_are_refused_not_crashed_on() {
    let dir = Scratch::new("hostile");
    let mut s = library(&dir);
    for params in [
        json!({"folder": 5, "path": []}),
        json!({"folder": "", "path": ""}),
        json!({"folder": "..", "path": "."}),
        json!({"folder": dir.path("trip"), "path": "\0"}),
        json!({"folder": dir.path("trip"), "path": dir.path("trip"), "incomplete": "yes"}),
        json!({"folder": "\\\\?\\", "path": dir.path("trip"), "disk": true}),
    ] {
        refused(&mut s, params);
    }
}

#[test]
fn a_new_place_already_browsed_in_local_does_not_bring_its_files_in_twice() {
    let dir = Scratch::new("browsed");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    // looking for the photos, the person browsed the folder where they are now
    s.execute("library.browse", &json!({"path": now})).unwrap();
    s.execute("library.browse", &json!({"path": dir.path("archive/journey/day1")})).unwrap();
    let records = s.catalog.len();
    assert!(records > 2, "browsing catalogued the files as Local records");

    let r = relocate(&mut s, &dir.path("trip"), &now);
    assert_eq!(r["relinked"], json!(2), "{r}");

    let scan = s.execute("folder.scanChanges", &json!({"path": now})).unwrap();
    assert!(scan["new"].as_array().unwrap().is_empty(), "the library's photos are these files: {scan}");
    assert_eq!(s.catalog.photos().filter(|p| p.in_library()).count(), 2);
}

#[test]
fn undo_brings_the_local_records_back_and_work_done_while_browsing_is_not_given_up() {
    let dir = Scratch::new("browsed-undo");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    s.execute("library.browse", &json!({"path": now})).unwrap();
    let records = s.catalog.len();
    let local = s.catalog.photos().find(|p| p.local).map(|p| p.id).unwrap();
    // the photo being looked at in Local is one of those that give way
    s.execute("library.select", &json!({"ids": [local.0]})).unwrap();

    let r = relocate(&mut s, &dir.path("trip"), &now);
    assert_eq!(r["replacedLocal"], json!(records - 2), "{r}");
    assert!(s.catalog.photo(local).is_none());
    assert!(s.selection.ids.iter().all(|id| s.catalog.photo(*id).is_some()), "nothing that is gone stays selected");
    s.undo_step().unwrap();
    assert_eq!(s.catalog.len(), records, "undo brings the Local records back");
    assert_eq!(files(&s).iter().filter(|f| f.starts_with(&dir.path("trip"))).count(), 2);

    // a rating given while browsing is the person's work: the library's photo can't replace it
    s.execute("library.select", &json!({"ids": [local.0]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": now}));
    assert!(e.contains("while browsing it in Local"), "{e}");
}

#[test]
fn what_was_found_is_not_applied_to_a_library_opened_meanwhile() {
    let dir = Scratch::new("switched");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    let located = Relocation::new(&s, &dir.path("trip"), &now, false).unwrap().look(&crate::import::ScanProgress::default()).unwrap();
    // File › Open Library while the files were looked for: its photo ids mean other photos
    s.library_identity = std::sync::Arc::new(());
    let (before, steps) = (files(&s), s.undo.len());

    let e = located.commit(&mut s, false).unwrap_err().to_string();

    assert!(e.contains("another library"), "{e}");
    assert_eq!(files(&s), before, "nothing landed in the other library");
    assert_eq!(s.undo.len(), steps);
}

#[test]
fn a_photo_relinked_while_the_files_were_looked_for_is_left_where_it_is() {
    let dir = Scratch::new("meanwhile");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    let located = Relocation::new(&s, &dir.path("trip"), &now, false).unwrap().look(&crate::import::ScanProgress::default()).unwrap();
    // meanwhile the person located one photo by hand, somewhere else
    write_png(&dir.path("elsewhere/a.png"), 1);
    let id = s.catalog.photos().find(|p| p.file_name == "a.png").map(|p| p.id).unwrap();
    s.execute("photo.relink", &json!({"id": id.0, "path": dir.path("elsewhere/a.png")})).unwrap();

    let r = located.commit(&mut s, false).unwrap();

    assert_eq!(r["relinked"], json!(1), "{r}");
    assert_eq!(files(&s), [dir.path("archive/journey/day1/b.png"), dir.path("elsewhere/a.png")], "its own choice stands");
}

/// What `look` found for `trip` → `to`, the files not yet relinked.
fn looked(s: &Session, dir: &Scratch, to: &str) -> crate::relocate::Located {
    Relocation::new(s, &dir.path("trip"), to, false).unwrap().look(&crate::import::ScanProgress::default()).unwrap()
}

#[test]
fn a_new_place_browsed_while_the_files_were_looked_for_still_has_one_record_per_file() {
    let dir = Scratch::new("browsed-meanwhile");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    let located = looked(&s, &dir, &now);
    // meanwhile the person clicked the folder in Local
    s.execute("library.browse", &json!({"path": now})).unwrap();
    let local = s.catalog.photos().filter(|p| p.local).count();
    assert!(local > 0);

    let r = located.commit(&mut s, false).unwrap();

    assert_eq!(r["replacedLocal"], json!(local), "{r}");
    let mut seen = std::collections::HashSet::new();
    assert!(files(&s).into_iter().all(|f| seen.insert(f)), "one record per file: {:?}", files(&s));
}

#[test]
fn a_file_that_became_another_photos_while_the_files_were_looked_for_is_not_shared() {
    let dir = Scratch::new("taken-meanwhile");
    let mut s = library(&dir);
    write_png(&dir.path("other/x.png"), 9);
    s.execute("library.import", &json!({"paths": [dir.path("other")]})).unwrap();
    let now = moved_away(&dir);
    let located = looked(&s, &dir, &now);
    // meanwhile another photo was pointed at one of the files
    let other = s.catalog.photos().find(|p| p.file_name == "x.png").map(|p| p.id).unwrap();
    s.execute("photo.relink", &json!({"id": other.0, "path": dir.path("archive/journey/a.png")})).unwrap();
    let (before, steps) = (files(&s), s.undo.len());

    let e = located.commit(&mut s, false).unwrap_err().to_string();

    assert!(e.contains("already in the library"), "{e}");
    assert_eq!((files(&s), s.undo.len()), (before, steps), "nothing changed");
}

#[test]
fn a_browsed_record_in_recently_deleted_at_the_new_place_is_not_doubled() {
    let dir = Scratch::new("local-trash");
    let mut s = library(&dir);
    let now = moved_away(&dir);
    s.execute("library.browse", &json!({"path": now})).unwrap();
    let local = s.catalog.photos().find(|p| p.local).map(|p| p.id).unwrap();
    s.catalog.apply(Op::SetDeleted { id: local, deleted: true }).unwrap();

    // restoring it later would make a second photo of the file
    let e = refused(&mut s, json!({"folder": dir.path("trip"), "path": now}));
    assert!(e.contains("Recently Deleted included"), "{e}");
}

/// A library that also holds `archive/journey/phone/p.png` (labelled red), with the files of
/// `trip` moved into `archive/journey` behind its back.
fn moved_into_a_folder_of_the_library(dir: &Scratch) -> (Session, String) {
    let mut s = library(dir);
    write_png(&dir.path("archive/journey/phone/p.png"), 7);
    s.execute("library.import", &json!({"paths": [dir.path("archive/journey/phone")]})).unwrap();
    s.execute("folder.label", &json!({"path": dir.path("archive/journey/phone"), "label": "red"})).unwrap();
    std::fs::rename(dir.0.join("trip/a.png"), dir.0.join("archive/journey/a.png")).unwrap();
    std::fs::rename(dir.0.join("trip/day1"), dir.0.join("archive/journey/day1")).unwrap();
    std::fs::remove_dir(dir.0.join("trip")).unwrap();
    (s, dir.path("archive/journey"))
}

#[test]
fn folders_of_the_library_already_at_the_new_place_keep_their_labels() {
    let dir = Scratch::new("labels-there");
    let (mut s, now) = moved_into_a_folder_of_the_library(&dir);
    s.execute("folder.label", &json!({"path": dir.path("trip/day1"), "label": "blue"})).unwrap();

    let r = relocate(&mut s, &dir.path("trip"), &now);

    assert_eq!(r["relinked"], json!(2), "{r}");
    assert_eq!(s.catalog.folder_color_label(&dir.path("archive/journey/phone")), Some(ColorLabel::Red), "it was there all along");
    assert_eq!(s.catalog.folder_color_label(&dir.path("archive/journey/day1")), Some(ColorLabel::Blue), "and the located one's came along");
    s.undo_step().unwrap();
    assert_eq!(s.catalog.folder_color_label(&dir.path("archive/journey/phone")), Some(ColorLabel::Red));
    assert_eq!(s.catalog.folder_color_label(&dir.path("trip/day1")), Some(ColorLabel::Blue));
}

#[test]
fn undo_leaves_a_view_that_did_not_move_where_it_is() {
    let dir = Scratch::new("view-stays");
    let (mut s, now) = moved_into_a_folder_of_the_library(&dir);
    relocate(&mut s, &dir.path("trip"), &now);
    // a folder of the library that was at the new place all along, chosen since
    let phone = dir.path("archive/journey/phone");
    s.execute("library.source", &json!({"kind": "libraryFolder", "path": phone})).unwrap();
    s.undo_step().unwrap();
    assert_eq!(s.library_folder.as_deref(), Some(phone.as_str()), "its photos never moved");
    s.redo_step().unwrap();
    assert_eq!(s.library_folder.as_deref(), Some(phone.as_str()));

    // and a folder browsed in Local is a place on disk
    s.execute("library.browse", &json!({"path": dir.path("archive/journey/day1")})).unwrap();
    let browsed = s.browse.as_ref().map(|b| b.path.clone());
    assert!(browsed.is_some());
    s.undo_step().unwrap();
    assert_eq!(s.browse.as_ref().map(|b| b.path.clone()), browsed, "the folder on disk is where it was");
}

#[test]
fn a_folder_whose_files_went_into_a_folder_inside_it_is_located_there() {
    let dir = Scratch::new("inside");
    let mut s = library(&dir);
    // everything in `trip` was put into `trip/2019`
    std::fs::create_dir_all(dir.0.join("2019")).unwrap();
    std::fs::rename(dir.0.join("trip/a.png"), dir.0.join("2019/a.png")).unwrap();
    std::fs::rename(dir.0.join("trip/day1"), dir.0.join("2019/day1")).unwrap();
    std::fs::rename(dir.0.join("2019"), dir.0.join("trip/2019")).unwrap();

    let r = relocate(&mut s, &dir.path("trip"), &dir.path("trip/2019"));

    assert_eq!((r["relinked"].as_u64(), r["found"].as_u64()), (Some(2), Some(2)), "{r}");
    assert_eq!(files(&s), [dir.path("trip/2019/a.png"), dir.path("trip/2019/day1/b.png")]);
    s.undo_step().unwrap();
    assert_eq!(files(&s), [dir.path("trip/a.png"), dir.path("trip/day1/b.png")]);
}
