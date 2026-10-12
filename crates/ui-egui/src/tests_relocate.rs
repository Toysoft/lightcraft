//! Locating a library folder that is somewhere else now, in the app (see [`crate::relocate`];
//! issue #1112).
//!
//! Scenarios, in the words of someone whose folder is not where the library looks for it:
//!
//! * Synchronize Folder… on a folder that isn't there says so, and its button is Locate Missing
//!   Folder…: choose where the folder is now, the photos point there, and the dialog scans the
//!   folder at its new place (the "isn't there" message is gone).
//! * Choosing a folder that holds none of the photos changes nothing and says why; the dialog
//!   stays, to choose again.
//! * Update Folder Location… on a Folders row does the same without the dialog.
//! * When only some of the photos are in the chosen folder, a dialog asks first; Cancel leaves
//!   the library as it was.
//! * The files are looked for in the background under a row in the activity stack; its ✕ stops
//!   it and nothing is changed.
//! * A dialog opened meanwhile is not taken away by the question, and a Synchronize dialog that
//!   was closed is not opened again.
//!
//! More scenarios, from the review of the pull request, are at the end.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::relocate::Request;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-ui-relocate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    /// The path as the folder tree writes it (forward slashes; widget ids carry it).
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().replace('\\', "/")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_png(path: &str, seed: u8) {
    let (w, h) = (24usize, 16usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 9) as u8, (i / w * 13) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The app with `trip/a.png` and `trip/b.png` imported, then the folder moved to
/// `archive/journey` behind its back. The folder dialog answers `chosen` (a path below the
/// scratch folder).
fn moved_folder(dir: &Scratch, chosen: &str) -> Headless {
    write_png(&dir.path("trip/a.png"), 1);
    write_png(&dir.path("trip/b.png"), 2);
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.execute("library.import", &json!({"paths": [dir.path("trip")]})).unwrap();
    std::fs::create_dir_all(dir.0.join("archive")).unwrap();
    std::fs::rename(dir.0.join("trip"), dir.0.join("archive/journey")).unwrap();
    let chosen = dir.path(chosen);
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let services = Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        pick_folder: Some(Box::new(move || Some(chosen.clone()))),
        ..Default::default()
    };
    let mut h = Headless::new(LightcraftApp::new(session, services), [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "photoGrid", "leftPanel": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

/// Where the library looks for its photos' files.
fn files(h: &Headless) -> Vec<String> {
    let mut v: Vec<String> = h
        .app
        .session
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Some(path.replace('\\', "/")),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

/// With `LIGHTCRAFT_TEST_SHOTS` set, save what is on screen there as `name` (to look at the result).
fn shot(h: &mut Headless, name: &str) {
    if let Ok(p) = std::env::var("LIGHTCRAFT_TEST_SHOTS") {
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&p).join(name).to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
}

/// Right-click the folder's row in Folders and choose the item with widget id `item`.
fn row_menu(h: &mut Headless, folder: &str, item: &str) {
    let row = format!("source:libfolder:{folder}");
    assert!(h.step_until(T, |h| h.app.widgets.iter().any(|(w, _)| *w == row)), "no row {row}");
    let rect = h.app.widgets.iter().find(|(w, _)| *w == row).map(|(_, r)| *r).unwrap_or_else(|| panic!("no row {row}"));
    let c = rect.center();
    let r = h.request("ui.click", json!({"x": c.x, "y": c.y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    h.step();
    let r = h.request("ui.clickWidget", json!({"id": item}), T);
    assert_eq!(r["ok"], true, "{item}: {r}");
    h.step();
}

/// Synchronize Folder… on the folder, scanned: it isn't there.
fn open_offline(h: &mut Headless, folder: &str) {
    row_menu(h, folder, "folderSynchronize");
    let scanned = h.step_until(T, |h| matches!(&h.app.ui.dialog, Some(Dialog::SynchronizeFolder { counts: Some(_), .. })));
    assert!(scanned, "the scan finished: {:?}", h.app.ui.dialog);
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::SynchronizeFolder { counts: Some(c), .. }) if c.offline), "{:?}", h.app.ui.dialog);
}

/// Press the dialog's button and wait until the click has landed (`landed`). A click is queued
/// input: on a loaded machine the dialog can still be settling into place when it arrives, so it
/// is clicked again until it lands.
fn press_ok(h: &mut Headless, landed: impl Fn(&Headless) -> bool) {
    for _ in 0..5 {
        h.step();
        let r = h.request("ui.clickWidget", json!({"id": "button:dialogOk"}), T);
        assert_eq!(r["ok"], true, "{r}");
        if h.step_until(Duration::from_secs(3), &landed) {
            return;
        }
    }
    panic!("the button never took: {:?} · {:?}", h.app.ui.dialog, toast(h));
}

#[test]
fn a_folder_that_isnt_there_is_located_from_its_dialog_and_scanned_at_its_new_place() {
    let dir = Scratch::new("dialog");
    let mut h = moved_folder(&dir, "archive/journey");
    let steps = h.app.session.undo.len();
    open_offline(&mut h, &dir.path("trip"));
    shot(&mut h, "locate-offline.png");

    // the dialog's button is Locate Missing Folder…; the folder dialog answers `archive/journey`
    let now = dir.path("archive/journey");
    press_ok(
        &mut h,
        |h| matches!(&h.app.ui.dialog, Some(Dialog::SynchronizeFolder { path, counts: Some(c), .. }) if path.replace('\\', "/") == now && !c.offline),
    );

    assert_eq!(files(&h), [dir.path("archive/journey/a.png"), dir.path("archive/journey/b.png")], "the photos point at the folder chosen");
    assert_eq!(h.app.session.undo.len(), steps + 1, "one undo step");
    assert!(toast(&h).contains("Folder located"), "{:?}", toast(&h));
    shot(&mut h, "locate-done.png");
    assert!(h.app.relocate.is_none() && h.app.session.activity.list().iter().all(|r| r.kind != "relocate"), "its row is gone");
    match &h.app.ui.dialog {
        Some(Dialog::SynchronizeFolder { counts: Some(c), .. }) => {
            assert_eq!((c.new, c.missing, c.moved), (0, 0, 0), "the library is up to date with the folder: {c:?}")
        }
        d => panic!("{d:?}"),
    }
}

#[test]
fn a_folder_that_holds_none_of_the_photos_is_not_taken_and_the_dialog_stays() {
    let dir = Scratch::new("wrong");
    // one level too high
    let mut h = moved_folder(&dir, "archive");
    let (before, steps) = (files(&h), h.app.session.undo.len());
    open_offline(&mut h, &dir.path("trip"));

    press_ok(&mut h, |h| h.app.relocate.is_none() && toast(h).contains("None of this folder's photos"));

    assert_eq!(files(&h), before, "nothing changed");
    assert_eq!(h.app.session.undo.len(), steps);
    assert!(
        matches!(&h.app.ui.dialog, Some(Dialog::SynchronizeFolder { path, counts: Some(c), .. }) if path.replace('\\', "/") == dir.path("trip") && c.offline),
        "the dialog stays, to choose again: {:?}",
        h.app.ui.dialog
    );
}

#[test]
fn update_folder_location_on_a_folders_row_relinks_without_a_dialog() {
    let dir = Scratch::new("row");
    let mut h = moved_folder(&dir, "archive/journey");

    row_menu(&mut h, &dir.path("trip"), "folderLocate");
    assert!(h.step_until(T, |h| h.app.relocate.is_none() && toast(h).contains("Folder located")), "{:?}", toast(&h));

    assert_eq!(files(&h), [dir.path("archive/journey/a.png"), dir.path("archive/journey/b.png")]);
    assert!(h.app.ui.dialog.is_none(), "{:?}", h.app.ui.dialog);
    // undo points the photos back where they were
    h.app.session.undo_step().unwrap();
    assert_eq!(files(&h), [dir.path("trip/a.png"), dir.path("trip/b.png")]);
}

#[test]
fn a_folder_that_holds_only_some_of_the_photos_asks_first() {
    let dir = Scratch::new("some");
    let mut h = moved_folder(&dir, "archive/journey");
    std::fs::remove_file(dir.path("archive/journey/b.png")).unwrap();
    let before = files(&h);

    // Cancel: the library stays as it was
    row_menu(&mut h, &dir.path("trip"), "folderLocate");
    let asked = |h: &Headless| matches!(&h.app.ui.dialog, Some(Dialog::LocateFolder { found: 1, total: 2, .. }));
    assert!(h.step_until(T, asked), "{:?} · {:?}", h.app.ui.dialog, toast(&h));
    assert_eq!(files(&h), before, "nothing changed while it asks");
    shot(&mut h, "locate-some.png");
    h.app.ui.dialog = None;
    h.step();
    h.step();
    assert!(h.app.relocate_asked.is_none(), "what was found goes with the question");
    assert_eq!(files(&h), before);

    // Locate Anyway: both photos follow, one of them missing at the new place
    row_menu(&mut h, &dir.path("trip"), "folderLocate");
    assert!(h.step_until(T, asked), "{:?}", h.app.ui.dialog);
    press_ok(&mut h, |h| h.app.ui.dialog.is_none() && toast(h).contains("Folder located"));
    assert_eq!(files(&h), [dir.path("archive/journey/a.png"), dir.path("archive/journey/b.png")]);
    let missing = h.app.session.execute("library.missing", &json!({})).unwrap();
    assert_eq!(missing.as_array().map(Vec::len), Some(1), "{missing}");
}

#[test]
fn the_files_are_looked_for_under_a_row_whose_cross_stops_it() {
    let dir = Scratch::new("activity");
    let mut h = moved_folder(&dir, "archive/journey");
    let before = files(&h);
    let request = Request { path: dir.path("trip"), to: dir.path("archive/journey"), disk: false, incomplete: false, synchronize: false };

    // started between frames: nothing is collected until the next one
    let r = crate::relocate::start(&mut h.app, request.clone());
    assert!(r.is_ok(), "{r:?}");
    let rows = h.app.session.activity.list();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].kind, rows[0].label.as_str()), ("relocate", "Locating folder"));
    assert!(rows[0].detail.ends_with("trip"), "the folder by its name: {:?}", rows[0].detail);
    assert!(rows[0].cancellable);
    // one at a time
    assert!(crate::relocate::start(&mut h.app, request.clone()).is_err());

    // the row's ✕ (as `activity.cancel`)
    h.app.session.activity.cancel(rows[0].id).unwrap();
    assert!(h.step_until(T, |h| h.app.relocate.is_none()), "it ends");
    h.step();
    assert_eq!(files(&h), before, "stopped: nothing changed");
    assert!(h.app.session.activity.list().is_empty(), "the row goes with it");

    // an import running: its photos could be the folder's
    let r = crate::import::start_paths(&mut h.app, vec![dir.path("archive/journey/a.png")]);
    assert!(r.is_ok() && h.app.import.is_some(), "{r:?}");
    let r = crate::relocate::start(&mut h.app, request);
    assert!(r.is_err() && h.app.relocate.is_none(), "not while importing: {r:?}");
}

#[test]
fn a_refusal_that_needs_no_disk_is_said_at_once() {
    let dir = Scratch::new("refused");
    let mut h = moved_folder(&dir, "archive/journey");
    // a folder the library has no photo from
    let r = h.app.run("folder.locate", json!({"folder": dir.path("nothing"), "path": dir.path("archive/journey")}));
    assert!(r.as_ref().is_err_and(|e| e.contains("no photo in the library")), "{r:?}");
    assert!(h.app.relocate.is_none() && h.app.session.activity.list().is_empty());
    let r = h.app.run("folder.locate", json!({"path": dir.path("archive/journey")}));
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn the_question_waits_for_another_dialog_to_be_done_with() {
    let dir = Scratch::new("waits");
    let mut h = moved_folder(&dir, "archive/journey");
    std::fs::remove_file(dir.path("archive/journey/b.png")).unwrap();
    let request = Request { path: dir.path("trip"), to: dir.path("archive/journey"), disk: false, incomplete: false, synchronize: false };
    assert!(crate::relocate::start(&mut h.app, request).is_ok());
    // something else was opened while the files were looked for
    h.app.ui.dialog = Some(Dialog::About);
    assert!(h.step_until(T, |h| h.app.relocate.is_none()), "the looking ends");
    h.step();
    h.step();
    assert!(matches!(h.app.ui.dialog, Some(Dialog::About)), "it is not taken away: {:?}", h.app.ui.dialog);
    assert!(h.app.relocate_asked.is_some(), "the question is kept");
    h.app.ui.dialog = None;
    assert!(
        h.step_until(T, |h| matches!(&h.app.ui.dialog, Some(Dialog::LocateFolder { found: 1, total: 2, .. }))),
        "then it is asked: {:?}",
        h.app.ui.dialog
    );
}

#[test]
fn synchronize_is_not_opened_again_once_its_dialog_was_closed() {
    let dir = Scratch::new("closed");
    let mut h = moved_folder(&dir, "archive/journey");
    // the folder dialog's answer arrives after the Synchronize dialog was cancelled
    let r = h.app.run("folder.locate", json!({"folder": dir.path("trip"), "path": dir.path("archive/journey"), "synchronize": true}));
    assert!(r.is_ok(), "{r:?}");
    assert!(h.step_until(T, |h| h.app.relocate.is_none() && toast(h).contains("Folder located")), "{:?}", toast(&h));
    h.step();
    assert!(h.app.ui.dialog.is_none(), "{:?}", h.app.ui.dialog);
}

// ── Review of #1122 ──────────────────────────────────────────────────────────────────────────
//
// * A file at the new place that is not the size the library recorded is said in the question.

#[test]
fn a_file_that_is_not_the_size_the_library_recorded_is_said_in_the_question() {
    let dir = Scratch::new("other-size");
    let mut h = moved_folder(&dir, "archive/journey");
    // another picture under the same name
    let img = lightcraft_raster::Rgba8 {
        width: 40,
        height: 30,
        data: (0..1200usize).map(|i| [(i * 7) as u8, (i * 3) as u8, (i / 5) as u8, 255]).collect(),
    };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(dir.path("archive/journey/b.png"), bytes).unwrap();

    row_menu(&mut h, &dir.path("trip"), "folderLocate");
    let asked = |h: &Headless| matches!(&h.app.ui.dialog, Some(Dialog::LocateFolder { found: 1, total: 2, different: 1, .. }));
    assert!(h.step_until(T, asked), "{:?} · {:?}", h.app.ui.dialog, toast(&h));
    shot(&mut h, "locate-different.png");
}
