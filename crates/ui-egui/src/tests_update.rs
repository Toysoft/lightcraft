//! Help ▸ Check for Updates… in the app (see `lcc_engine::update`).
//!
//! Scenarios, in the words of someone updating:
//!
//! * Help ▸ Check for Updates… opens the dialog, which shows this build and asks the release feed.
//! * A newer release is listed with its file, size, install action and notes, and it can be
//!   downloaded, installed or skipped.
//! * Skipping remembers the version, so a start-up check does not ask about it again.
//! * The two settings are in the dialog and are saved.
//! * An install that only lays the file out never quits the app, and says what to do instead.
//!
//! The fixtures (a scratch folder, a fake downloaded file) are set up the short way this crate's
//! other UI tests use: a failed `unwrap` there fails the test, which is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-ui-update-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ask(h: &mut Headless, method: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request(method, params, T);
    assert_eq!(r["ok"], true, "{method}: {r}");
    r
}

/// The PNG encoder and file writer a real host provides (`ui.screenshot` needs them).
fn services() -> Services {
    Services {
        png: Some(Box::new(|img: &lcc_raster::Rgba8| {
            lcc_codecs::encode_png(&lcc_codecs::EncodeImage::rgba8(img), &lcc_codecs::EncodeMeta::default()).unwrap_or_default()
        })),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    }
}

/// The app with an in-memory session (no library, no network) and the start-up check already done.
fn app() -> Headless {
    let mut session = lcc_engine::Session::new();
    // a fresh updater: the settings are the defaults, not whatever this machine's are
    session.updater = lcc_engine::update::Updater::new(None);
    let app = LccApp::new(session, services());
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    // no start-up check: these tests drive the dialog themselves
    h.app.ui.update_checked_at_startup = true;
    h.step();
    h
}

/// Offer `version` as a newer release, as a check would, with its file already downloaded.
fn offer(h: &mut Headless, dir: &Scratch, version: &str, kind: lcc_engine::update::Kind) -> PathBuf {
    use lcc_engine::update::{Kind, Progress, Staged};
    let file = dir.0.join(format!("app-{version}-{}.bin", kind.text()));
    std::fs::write(&file, b"an installer").unwrap();
    let engine_kind = match kind {
        Kind::Msi => lcc_updater::Kind::Msi,
        Kind::Dmg => lcc_updater::Kind::Dmg,
        Kind::AppImage => lcc_updater::Kind::AppImage,
        Kind::Archive => lcc_updater::Kind::Archive,
        Kind::Checksums => lcc_updater::Kind::Checksums,
    };
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let update = lcc_updater::Update {
        current: lcc_updater::Version::current(),
        version: lcc_updater::Version::parse(version).unwrap(),
        title: format!("App {version}"),
        tag: format!("v{version}"),
        asset: lcc_updater::Asset { name: name.clone(), url: "http://127.0.0.1:1/app.bin".into(), size: 12, sha256: None, kind: engine_kind },
        sha256: Some("ab".repeat(32)),
        published: Some("2026-10-12".into()),
        notes: "### Fixed\n- a thing".into(),
        action: "run the installer".into(),
    };
    h.app.session.updater.apply(lcc_updater::Checked::Found(Box::new(update)));
    h.app.session.updater.set_progress(Progress::Ready(Box::new(Staged {
        version: version.into(),
        path: file.clone(),
        bytes: 12,
        sha256: "ab".repeat(32),
        file: name,
        kind,
        notes: "### Fixed\n- a thing".into(),
    })));
    file
}

#[test]
fn the_help_menu_opens_the_dialog_and_it_lists_what_is_available() {
    let dir = Scratch::new("listing");
    let mut h = app();
    let _ = offer(&mut h, &dir, "9.9.9", lcc_engine::update::Kind::Archive);
    ask(&mut h, "ui.menu.invoke", json!({"id": "app.checkForUpdates"}));
    h.step();
    assert_eq!(h.app.ui.dialog, Some(Dialog::Update), "the dialog is up");
    // Esc closes the dialog (the window's own Close is registered as the frame's)
    assert!(h.app.widgets.iter().any(|(w, _)| w == "dialog:window"), "the window is laid out");
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:updateCheck"));
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:updateInstall"), "a downloaded file can be installed");
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:updateSkip"));
    assert!(h.app.widgets.iter().any(|(w, _)| w == "check:updateAtStartup"));
    assert!(h.app.widgets.iter().any(|(w, _)| w == "check:updatePrerelease"));
    let text = h.painted_text().join(" ");
    assert!(text.contains("9.9.9"), "the version is shown: {text}");
    assert!(text.contains("is available"), "{text}");
    // Esc ends it, like the other dialogs that only show something
    h.request("ui.key", json!({"key": "Escape"}), T);
    h.step();
    assert_eq!(h.app.ui.dialog, None, "Esc closes the dialog");
}

#[test]
fn a_startup_check_that_found_a_release_waits_until_the_dialog_is_opened() {
    let dir = Scratch::new("startup");
    let mut h = app();
    // as the start-up path does: the worker's answer is pending, and no window opened by itself
    h.app.ui.update_pending = Some("9.9.9".into());
    let _ = offer(&mut h, &dir, "9.9.9", lcc_engine::update::Kind::Archive);
    h.step();
    h.step();
    assert_eq!(h.app.ui.dialog, None, "a start-up check never opens a window");
    assert!(h.app.ui.update_pending.is_none(), "the announcement is made once");
    assert!(h.app.ui.toast.is_some(), "it says so instead");
}

#[test]
fn a_manual_install_never_quits_the_app() {
    let dir = Scratch::new("manual");
    let mut h = app();
    let file = offer(&mut h, &dir, "9.9.9", lcc_engine::update::Kind::Archive);
    let desktop = std::sync::Arc::new(lcc_engine::desktop::Recorder::default());
    h.app.session.desktop = desktop.clone();
    ask(&mut h, "ui.menu.invoke", json!({"id": "app.checkForUpdates"}));
    h.step();
    // the archive path is a manual install: it shows the file and stays open
    h.request("ui.clickWidget", json!({"id": "button:updateInstall"}), T);
    h.step();
    assert!(!h.quit_requested(), "nothing to quit for");
    assert_eq!(h.app.ui.dialog, Some(Dialog::Update));
    assert!(file.exists(), "the downloaded file is still there for the user to unpack");
    let installed = h.app.session.updater.installed().expect("an outcome");
    assert!(!installed.exit);
    assert!(installed.message.contains("Quit the app"), "{}", installed.message);
    // the file is shown through the session's desktop, never by starting the real file manager
    assert_eq!(desktop.requests(), vec![lcc_engine::desktop::Request::Reveal(file.clone())]);
}

#[test]
fn skipping_remembers_the_version() {
    let dir = Scratch::new("skip");
    let mut h = app();
    let _ = offer(&mut h, &dir, "9.9.9", lcc_engine::update::Kind::Archive);
    ask(&mut h, "ui.menu.invoke", json!({"id": "app.checkForUpdates"}));
    h.step();
    ask(&mut h, "ui.clickWidget", json!({"id": "button:updateSkip"}));
    h.step();
    assert!(h.app.session.updater.already_seen("9.9.9"), "the version is remembered");
    // the dialog stays open until it is closed, and the update is no longer announced as new
    assert_eq!(h.app.ui.dialog, Some(Dialog::Update));
    h.request("ui.key", json!({"key": "Escape"}), T);
    h.step();
    assert_eq!(h.app.ui.dialog, None, "Esc ends the dialog");
}

#[test]
fn the_settings_in_the_dialog_are_saved() {
    let mut h = app();
    ask(&mut h, "ui.menu.invoke", json!({"id": "app.checkForUpdates"}));
    h.step();
    assert!(h.app.session.updater.prefs().check_at_startup, "on by default");
    ask(&mut h, "ui.clickWidget", json!({"id": "check:updateAtStartup"}));
    h.step();
    assert!(!h.app.session.updater.prefs().check_at_startup, "turning it off is saved");
    ask(&mut h, "ui.clickWidget", json!({"id": "check:updatePrerelease"}));
    h.step();
    assert!(h.app.session.updater.prefs().prerelease);
    // the dialog is still usable after a settings change
    assert_eq!(h.app.ui.dialog, Some(Dialog::Update));
    h.request("ui.key", json!({"key": "Escape"}), T);
    h.step();
    assert_eq!(h.app.ui.dialog, None);
}

#[test]
fn the_download_button_says_it_cannot_start_when_there_is_nothing_to_fetch() {
    let mut h = app();
    ask(&mut h, "ui.menu.invoke", json!({"id": "app.checkForUpdates"}));
    h.step();
    // nothing on offer: no Download button is drawn at all
    assert!(h.app.widgets.iter().all(|(w, _)| w != "button:updateDownload"), "no update, no Download");
    // and the command says why rather than panicking
    let r = h.app.session.execute("update.download", &json!({}));
    assert!(r.is_err());
}

#[test]
fn a_checksum_list_is_never_offered_for_install() {
    let dir = Scratch::new("sums");
    let mut h = app();
    let _ = offer(&mut h, &dir, "9.9.9", lcc_engine::update::Kind::Checksums);
    ask(&mut h, "ui.menu.invoke", json!({"id": "app.checkForUpdates"}));
    h.step();
    let err = h.app.session.execute("update.install", &json!({"confirm": true})).expect_err("refused");
    assert!(err.to_string().contains("no file for this system"), "{err}");
}
