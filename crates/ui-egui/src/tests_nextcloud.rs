//! Nextcloud in the app (against craft's `FakeServer`): the File and Photo menu items, the sign-in and
//! import dialogs, the Settings tab, Show in Finder, and Develop's download policy (browsing unedited
//! photos downloads nothing; an edit tool downloads the open photo and prefetches the next).

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_nextcloud::Account;
use craft_nextcloud::testing::FakeServer;
use lightcraft_engine::Session;
use lightcraft_engine::remote::{AccountState, CredentialStore, MemoryCredentials, RemoteConfig};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::menubar::MenuNode;
use crate::state::{Dialog, RightPanel, ViewMode, Zoom};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-ui-nextcloud-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session with files on this computer and Nextcloud settings in `dir`, signed in to `accounts`.
fn session(dir: &Path, accounts: &[Account]) -> Session {
    std::fs::write(dir.join("nextcloud.json"), serde_json::to_vec(&json!({"accounts": accounts})).unwrap()).unwrap();
    let creds = Arc::new(MemoryCredentials::default());
    for a in accounts {
        creds.set(a, FakeServer::PASSWORD).unwrap();
    }
    let s = Session::new().with_fs();
    s.remote.configure(RemoteConfig {
        settings_file: Some(dir.join("nextcloud.json")),
        cache_dir: Some(dir.join("cache")),
        credentials: Some(creds),
    });
    s
}

/// The app on `s`; the URLs it opens in the browser and the files it shows in Finder land in the lists.
fn app(s: Session, urls: &Arc<Mutex<Vec<String>>>, revealed: &Arc<Mutex<Vec<String>>>) -> Headless {
    let (u, r) = (urls.clone(), revealed.clone());
    let services = Services {
        png: None,
        open_url: Some(Box::new(move |url: &str| {
            u.lock().unwrap().push(url.to_string());
            Ok(())
        })),
        reveal: Some(Box::new(move |path: &str| {
            r.lock().unwrap().push(path.to_string());
            Ok(())
        })),
        ..Default::default()
    };
    let mut h = Headless::new(LightcraftApp::new(s, services), [1400.0, 900.0], 1.0);
    h.step();
    h
}

/// Click widget `id` once what's on screen has settled (a dialog's first frames lay it out).
fn click(h: &mut Headless, id: &str) {
    h.settle(T);
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

fn shows(h: &Headless, text: &str) -> bool {
    h.painted_text().iter().any(|t| t.contains(text))
}

/// `nextcloud://alice@127.0.0.1:<port>/<rel>`.
fn remote(fake: &FakeServer, rel: &str) -> String {
    rel.split('/').try_fold(fake.account().root(), |p, seg| p.join(seg)).unwrap().to_string()
}

/// Files in the originals cache (not its index).
fn cached_files(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("cache"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|a| a.path().is_dir())
        .map(|a| std::fs::read_dir(a.path()).unwrap().count())
        .sum()
}

fn file_menu_labels(h: &Headless, menu: &str) -> Vec<(String, String)> {
    let bar = crate::menubar::menu_bar(&h.app);
    let (_, items) = bar.into_iter().find(|(t, _)| t == menu).unwrap();
    items.into_iter().filter_map(|n| if let MenuNode::Item { id, label, .. } = n { Some((id, label)) } else { None }).collect()
}

fn sign_in_dialog(h: &Headless) -> (Option<String>, Option<String>, Option<String>) {
    match &h.app.ui.dialog {
        Some(Dialog::NextcloudSignIn { fingerprint, login_url, error, .. }) => (fingerprint.clone(), login_url.clone(), error.clone()),
        other => panic!("not the sign-in dialog: {other:?}"),
    }
}

#[test]
fn menus_list_the_nextcloud_items() {
    let d = temp("menus");
    let (urls, revealed) = Default::default();
    let mut h = app(session(&d, &[]), &urls, &revealed);
    let file = file_menu_labels(&h, "File");
    let at = file.iter().position(|(id, _)| id == "file.importLightroom").expect("Import Lightroom Catalog… in File");
    assert_eq!(file[at + 1], ("file.addNextcloudAccount".to_string(), "Add Nextcloud Account…".to_string()));
    assert_eq!(file[at + 2], ("file.importNextcloud".to_string(), "Import from Nextcloud…".to_string()));
    let photo = file_menu_labels(&h, "Photo");
    for id in ["photo.makeAvailableOffline", "photo.removeOfflineCopy"] {
        assert!(photo.iter().any(|(i, _)| i == id), "{id} in Photo");
    }

    h.app.run("file.addNextcloudAccount", json!({})).unwrap();
    h.settle(T);
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::NextcloudSignIn { server, .. }) if server.is_empty()));
    assert!(shows(&h, "Add Nextcloud Account"));
    // no account yet: the import dialog offers to add one
    h.app.ui.dialog = None;
    h.app.run("file.importNextcloud", json!({})).unwrap();
    h.settle(T);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::NextcloudImport { .. })));
    assert!(shows(&h, "Add Nextcloud Account…"), "{:?}", h.painted_text());
    click(&mut h, "button:dialogOk");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::NextcloudSignIn { .. })), "{:?}", h.app.ui.dialog);
}

#[test]
fn sign_in_dialog_walks_trust_and_waiting() {
    let fake = FakeServer::start_tls(lightcraft_engine::remote::provider());
    let fp = fake.fingerprint().unwrap().to_string();
    let d = temp("sign-in");
    let (urls, revealed) = Default::default();
    let mut h = app(session(&d, &[]), &urls, &revealed);
    h.app.run("file.addNextcloudAccount", json!({})).unwrap();
    h.step();
    click(&mut h, "field:nextcloudServer");
    let r = h.request("ui.text", json!({"text": fake.url()}), T);
    assert_eq!(r["ok"], true, "{r}");
    click(&mut h, "button:dialogOk");
    assert!(h.step_until(T, |h| sign_in_dialog(h).0.is_some()), "no fingerprint shown: {:?}", h.app.ui.dialog);
    assert_eq!(sign_in_dialog(&h).0.as_deref(), Some(fp.as_str()));
    h.settle(T);
    assert!(shows(&h, &fp) && shows(&h, "Trust This Certificate"), "{:?}", h.painted_text());
    assert!(h.app.session.remote.accounts().is_empty());

    click(&mut h, "button:dialogOk");
    assert!(h.step_until(T, |h| sign_in_dialog(h).1.is_some()), "never waiting: {:?}", h.app.ui.dialog);
    let login = sign_in_dialog(&h).1.unwrap();
    assert_eq!(*urls.lock().unwrap(), [login], "the login page opens in the browser");
    h.step();
    assert!(shows(&h, "Approve LightCraft in your browser…"), "{:?}", h.painted_text());
    assert!(h.app.session.activity.list().iter().any(|t| t.kind == "nextcloud"), "the sign-in waits in the activity stack");

    fake.approve_login();
    assert!(h.step_until(T, |h| h.app.ui.dialog.is_none()), "the dialog stayed: {:?}", h.app.ui.dialog);
    let accounts = h.app.session.remote.accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!((accounts[0].0.trust.map(|f| f.to_string()), accounts[0].1), (Some(fp), AccountState::SignedIn));
}

#[test]
fn cancelling_the_sign_in_dialog_cancels_its_row() {
    let fake = FakeServer::start();
    let d = temp("sign-in-cancel");
    let (urls, revealed) = Default::default();
    let mut h = app(session(&d, &[]), &urls, &revealed);
    h.app.run("file.addNextcloudAccount", json!({})).unwrap();
    if let Some(Dialog::NextcloudSignIn { server, .. }) = &mut h.app.ui.dialog {
        *server = fake.url();
    }
    h.step();
    click(&mut h, "button:dialogOk");
    assert!(h.step_until(T, |h| sign_in_dialog(h).1.is_some()), "never waiting: {:?}", h.app.ui.dialog);
    click(&mut h, "button:dialogCancel");
    assert!(h.app.ui.dialog.is_none());
    assert!(h.step_until(T, |h| h.app.session.activity.list().is_empty()), "the sign-in row stayed");
    let prefs = h.app.session.execute("nextcloud.preferences", &json!({})).unwrap();
    assert_eq!(prefs["signIns"][0]["state"], "cancelled", "{prefs}");
    assert!(h.app.session.remote.accounts().is_empty());
}

/// The folders the import dialog lists (names).
fn listed(h: &Headless) -> Vec<String> {
    match &h.app.ui.dialog {
        Some(Dialog::NextcloudImport { folders, loading: false, .. }) => folders.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![],
    }
}

#[test]
fn import_dialog_browses_and_imports() {
    let fake = FakeServer::start();
    for f in ["/Photos/2024/a.jpg", "/Photos/2024/b.jpg", "/Photos/2025/c.jpg", "/Documents/x.jpg"] {
        fake.add_file(f, vec![0; 10]);
    }
    let d = temp("import");
    let (urls, revealed) = Default::default();
    let mut h = app(session(&d, &[fake.account()]), &urls, &revealed);

    // the control channel gets the command's own reply, though it runs off the UI thread
    let r = h.request("engine.execute", json!({"command": "nextcloud.browse", "params": {"dir": remote(&fake, "Photos")}}), T);
    assert_eq!(r["result"]["folders"][0]["name"], "2024", "{r}");

    h.app.run("file.importNextcloud", json!({})).unwrap();
    // one account: its files at once
    assert!(h.step_until(T, |h| listed(h) == ["Documents", "Photos"]), "{:?}", h.app.ui.dialog);
    click(&mut h, "button:nextcloudOpen-1");
    assert!(h.step_until(T, |h| listed(h) == ["2024", "2025"]), "{:?}", h.app.ui.dialog);
    h.settle(T);
    assert!(shows(&h, "2024") && shows(&h, "2025"));
    click(&mut h, "check:nextcloudFolder-0");
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::NextcloudImport { chosen, .. }) if *chosen == [remote(&fake, "Photos/2024")]));
    click(&mut h, "button:dialogOk");
    assert!(h.app.ui.dialog.is_none(), "the import runs in the activity stack");
    assert!(h.step_until(T, |h| h.app.session.catalog.len() == 2), "photos: {}", h.app.session.catalog.len());

    // a folder gone from the server by the time it's imported: the import's error shows
    h.app.run("file.importNextcloud", json!({})).unwrap();
    assert!(h.step_until(T, |h| listed(h) == ["Documents", "Photos"]), "{:?}", h.app.ui.dialog);
    click(&mut h, "button:nextcloudOpen-1");
    assert!(h.step_until(T, |h| listed(h) == ["2024", "2025"]), "{:?}", h.app.ui.dialog);
    click(&mut h, "check:nextcloudFolder-1");
    fake.remove("/Photos/2025");
    click(&mut h, "button:dialogOk");
    let error = |h: &Headless| h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(h.step_until(T, |h| error(h).contains("no longer on the Nextcloud server")), "toast: {:?}", h.app.ui.toast);
    assert_eq!(h.app.session.catalog.len(), 2);

    // an import started elsewhere (an agent): its photos land as frames go by
    h.app.session.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Documents")]})).unwrap();
    assert!(h.step_until(T, |h| h.app.session.catalog.len() == 3), "photos: {}", h.app.session.catalog.len());
}

#[test]
fn browsing_unedited_remote_photos_downloads_nothing() {
    let fake = FakeServer::start();
    let preview = lightcraft_codecs::encode_jpeg(
        &lightcraft_codecs::EncodeImage::new(64, 48, 3, lightcraft_codecs::Samples::U8(&[90; 64 * 48 * 3])),
        90,
        lightcraft_codecs::ChromaSubsampling::S444,
        &Default::default(),
    )
    .unwrap();
    for f in ["/Photos/a.ARW", "/Photos/b.ARW", "/Photos/c.ARW"] {
        fake.add_file(f, (0..40_000).map(|i| (i % 253) as u8).collect());
        fake.set_preview(f, preview.clone());
    }
    let d = temp("develop");
    let mut s = session(&d, &[fake.account()]);
    s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")], "wait": true})).unwrap();
    assert_eq!(s.catalog.len(), 3);
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    let ids = h.app.session.visible_cloned();
    let first = ids[0];
    h.app.session.execute("library.select", &json!({"ids": [first.0]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Info;
    h.app.ui.zoom = Zoom::Fit;
    h.settle(T);
    for _ in 0..30 {
        h.step();
    }
    assert_eq!(fake.downloads(), 0, "browsing at Fit shows the server's preview");
    assert_eq!(cached_files(&d), 0);
    assert!(h.app.renderer.textures.get(&crate::render::Slot::Preview).is_some_and(|t| t.photo == first), "the quick view shows");

    // an edit tool: the photo's original, then the next one's
    h.app.ui.right = RightPanel::Edit;
    assert!(h.step_until(T, |h| cached_files(&d) == 2 && h.app.session.activity.list().is_empty()), "cached: {}", cached_files(&d));
    h.settle(T);
    assert_eq!(fake.downloads(), 2, "the open photo once, the next one prefetched");

    // back in the grid: nothing is held or prefetched any more
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.session.execute("library.select", &json!({"ids": [ids[2].0]})).unwrap();
    h.settle(T);
    assert_eq!(fake.downloads(), 2, "the grid downloads nothing");
}

#[test]
fn settings_tab_shows_accounts_and_sizes() {
    let fake = FakeServer::start();
    let d = temp("settings");
    let (urls, revealed) = Default::default();
    let mut h = app(session(&d, &[fake.account()]), &urls, &revealed);
    let key = fake.account().key();
    h.app.run("app.settings", json!({"tab": "nextcloud"})).unwrap();
    h.settle(T);
    assert!(shows(&h, &key), "{:?}", h.painted_text());
    assert!(shows(&h, "Signed In"), "{:?}", h.painted_text());
    assert!(shows(&h, "Cached: 0.0 GB · Offline copies: 0.0 GB"), "{:?}", h.painted_text());

    click(&mut h, "button:settingsNextcloudCache-3");
    assert_eq!(h.app.session.remote.cache_budget_mb(), 50_000);

    click(&mut h, "button:nextcloudSignInAgain-0");
    assert!(h.step_until(T, |_| !urls.lock().unwrap().is_empty()), "Sign In Again opens the login page");
    assert!(urls.lock().unwrap()[0].starts_with(&fake.url()), "{:?}", urls.lock().unwrap());
    fake.approve_login();
    assert!(
        h.step_until(T, |h| h.app.session.activity.list().is_empty() && h.app.session.remote.accounts()[0].1 == AccountState::SignedIn),
        "{:?}",
        h.app.session.remote.accounts()
    );

    click(&mut h, "button:nextcloudRemove-0");
    assert!(h.app.session.remote.accounts().is_empty());
    h.settle(T);
    assert!(!shows(&h, &key), "{:?}", h.painted_text());
}

#[test]
fn show_in_finder_is_disabled_for_remote_photos() {
    let d = temp("finder");
    let mut s = session(&d, &[]);
    let local = d.join("local.jpg");
    std::fs::write(&local, b"x").unwrap();
    for (i, path) in [local.to_string_lossy().to_string(), "nextcloud://alice@example.com/Photos/a.ARW".to_string()].into_iter().enumerate() {
        let p = lightcraft_catalog::Photo::new(
            lightcraft_catalog::PhotoId(i as u64 + 1),
            lightcraft_catalog::Source::File { path },
            "x",
            "JPEG",
            60,
            40,
            "2026-01-01T00:00:00",
        );
        s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    h.app.session.execute("library.select", &json!({"ids": [1]})).unwrap();
    assert!(crate::menus::ui_enabled(&h.app, "app.showInFinder"), "a local photo can be shown");
    h.app.session.execute("library.select", &json!({"ids": [2]})).unwrap();
    assert!(!crate::menus::ui_enabled(&h.app, "app.showInFinder"));
    let e = h.app.run("app.showInFinder", Value::Null).unwrap_err();
    assert_eq!(e, format!("{} isn't available for Nextcloud photos", crate::menus::reveal_label()));
    assert!(revealed.lock().unwrap().is_empty());
}

#[test]
fn photos_on_the_server_are_not_missing() {
    let fake = FakeServer::start();
    fake.add_file("/Photos/a.jpg", vec![0; 10]);
    let d = temp("missing");
    let mut s = session(&d, &[fake.account()]);
    s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")], "wait": true})).unwrap();
    assert_eq!(s.catalog.len(), 1);
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.ui.left_panel = true;
    // (the count is made on a worker thread: give it time, then look)
    for _ in 0..50 {
        h.settle(T);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(shows(&h, "Recently Deleted"), "the sidebar's Library rows: {:?}", h.painted_text());
    assert!(!shows(&h, "Missing Photos"), "{:?}", h.painted_text());
}
