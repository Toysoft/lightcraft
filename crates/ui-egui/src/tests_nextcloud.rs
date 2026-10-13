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

/// A `w`×`h` grey JPEG.
fn jpeg(w: usize, h: usize) -> Vec<u8> {
    let px = vec![90; w * h * 3];
    let img = lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&px));
    lightcraft_codecs::encode_jpeg(&img, 90, lightcraft_codecs::ChromaSubsampling::S444, &Default::default()).unwrap()
}

/// Three photos `a`, `b`, `c` with extension `ext` (`bytes` each) imported from a fake server that has
/// previews for them and Memories sizes (4000×6000, portrait); browsing them, then developing the
/// first: Fit and Fill (where a portrait photo wants window renders), the Compare view, then an
/// edit tool, then the grid.
fn browse_then_develop(ext: &str, bytes: Vec<u8>) {
    let fake = FakeServer::start();
    fake.enable_memories();
    let preview = jpeg(1000, 1500);
    for n in ["a", "b", "c"] {
        let f = format!("/Photos/{n}.{ext}");
        fake.add_file(&f, bytes.clone());
        fake.set_preview(&f, preview.clone());
        fake.set_photo(&f, 19675, 4000, 6000, 1_700_000_000, json!({}));
    }
    let d = temp(&format!("develop-{ext}"));
    let mut s = session(&d, &[fake.account()]);
    s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")], "wait": true})).unwrap();
    assert_eq!(s.catalog.len(), 3);
    assert!(s.catalog.photos().all(|p| (p.width, p.height) == (4000, 6000)), "Memories' sizes");
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    let ids = h.app.session.visible_cloned();
    let first = ids[0];
    h.app.session.execute("library.select", &json!({"ids": [first.0]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Info;
    let mut sizes = vec![];
    for zoom in [Zoom::Fit, Zoom::Fill] {
        h.app.ui.zoom = zoom;
        h.settle(T);
        for _ in 0..30 {
            h.step();
        }
        assert_eq!(fake.downloads(), 0, "{ext} at {zoom:?} shows the server's preview");
        sizes.push(h.app.renderer.textures.get(&crate::render::Slot::Preview).map_or(0, |t| t.size[1]));
    }
    assert!(sizes[1] > sizes[0], "Fill asks for a sharper server preview than Fit: {sizes:?}");
    assert_eq!(cached_files(&d), 0);
    assert!(h.app.renderer.textures.get(&crate::render::Slot::Preview).is_some_and(|t| t.photo == first), "the quick view shows");
    // comparing photos only looks at them too
    h.app.session.execute("library.select", &json!({"ids": [first.0, ids[1].0]})).unwrap();
    h.app.run("view.compare", json!({})).unwrap();
    assert_eq!(h.app.ui.view, ViewMode::Compare);
    for zoom in [Zoom::Fit, Zoom::Fill] {
        h.app.ui.zoom = zoom;
        h.settle(T);
        assert_eq!(fake.downloads(), 0, "{ext}: Compare at {zoom:?} shows the server's previews");
    }

    // an edit tool: the photo's original, then the next one's
    h.app.session.execute("library.select", &json!({"ids": [first.0]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    assert!(h.step_until(T, |h| cached_files(&d) == 2 && h.app.session.activity.list().is_empty()), "cached: {}", cached_files(&d));
    h.settle(T);
    assert_eq!(fake.downloads(), 2, "{ext}: the open photo once, the next one prefetched");

    // back in the grid: nothing is held or prefetched any more
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.session.execute("library.select", &json!({"ids": [ids[2].0]})).unwrap();
    h.settle(T);
    assert_eq!(fake.downloads(), 2, "the grid downloads nothing");
}

#[test]
fn browsing_unedited_remote_photos_downloads_nothing() {
    browse_then_develop("ARW", (0..40_000).map(|i| (i % 253) as u8).collect());
}

/// Phone photos (JPEG, HEIC…) are most of a Nextcloud archive: browsing them downloads nothing either.
#[test]
fn browsing_unedited_remote_jpegs_downloads_nothing() {
    browse_then_develop("jpg", jpeg(400, 600));
}

/// A server without a preview of a photo (no preview provider for its format): the loupe renders
/// its original at full size (one download), not the 512 px thumbnail the quick view fell back to.
#[test]
fn photos_without_a_server_preview_render_their_original() {
    let fake = FakeServer::start();
    fake.enable_memories();
    fake.add_file("/Photos/a.jpg", jpeg(1200, 1800));
    fake.set_photo("/Photos/a.jpg", 19675, 1200, 1800, 1_700_000_000, json!({}));
    let d = temp("no-preview");
    let mut s = session(&d, &[fake.account()]);
    s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")], "wait": true})).unwrap();
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    let first = h.app.session.visible_cloned()[0];
    h.app.session.execute("library.select", &json!({"ids": [first.0]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Info;
    h.app.ui.zoom = Zoom::Fit;
    let main = |h: &Headless| h.app.renderer.textures.get(&crate::render::Slot::Main).filter(|t| t.photo == first).map(|t| t.size);
    assert!(h.step_until(T, |h| main(h).is_some_and(|s| s[1] > 512)), "Main: {:?}", main(&h));
    h.settle(T);
    assert_eq!(fake.downloads(), 1);
}

/// `names` (`.ARW` blobs) on a fake server with Memories sizes, imported (their etags known), the app
/// on them in Detail with the Edit panel open on the first in view (its name returned); with the
/// settings folder.
fn developing(names: &[&str]) -> (Headless, FakeServer, PathBuf, String) {
    let fake = FakeServer::start();
    fake.enable_memories();
    for n in names {
        let f = format!("/Photos/{n}.ARW");
        fake.add_file(&f, (0..40_000).map(|i| (i % 253) as u8).collect());
        fake.set_photo(&f, 19675, 4000, 6000, 1_700_000_000, json!({}));
    }
    let d = temp(&format!("developing-{}", names.join("")));
    let mut s = session(&d, &[fake.account()]);
    s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")], "wait": true})).unwrap();
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    let first = h.app.session.visible_cloned()[0];
    h.app.session.execute("library.select", &json!({"ids": [first.0]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    let open = h.app.session.catalog.photo(first).unwrap().file_name.clone();
    (h, fake, d, open)
}

/// R35: the next photo is prefetched only once the open one's original is here, so the two never
/// share the connection: an open photo that can't be downloaded (gone from the server) leaves the
/// next one alone.
#[test]
fn the_next_photo_waits_for_the_open_one() {
    let (mut h, fake, d, open) = developing(&["a", "b"]);
    fake.remove(&format!("/Photos/{open}"));
    let mut prefetched = false;
    let t = std::time::Instant::now();
    while t.elapsed() < Duration::from_secs(1) {
        h.step();
        prefetched |= h.app.session.activity.list().iter().any(|r| r.label.starts_with("Prefetching"));
        std::thread::sleep(Duration::from_millis(5));
    }
    h.settle(T);
    assert!(!prefetched, "no prefetch while the open photo isn't here");
    assert_eq!((cached_files(&d), fake.downloads()), (0, 0));
}

/// R34: the app's UI thread never downloads: Auto (Shift+A) on a photo whose original isn't here
/// says to open it in Develop, at once, and downloads nothing.
#[test]
fn commands_on_the_ui_thread_never_download() {
    let (mut h, fake, d, _) = developing(&["a"]);
    h.app.ui.view = ViewMode::PhotoGrid;
    h.settle(T);
    let t = std::time::Instant::now();
    let r = h.request("engine.execute", json!({"command": "develop.auto"}), T);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(r["error"].as_str().is_some_and(|e| e.contains("open it in Develop to download it")), "{r}");
    h.settle(T);
    assert_eq!((cached_files(&d), fake.downloads()), (0, 0));
}

/// Loads that, run with a cancel flag (`memory::cancellable`), wait until it is set: a download
/// that stops when told. Without one they fail at once (thumbnails).
#[derive(Clone, Default)]
struct Loads {
    calls: Arc<Mutex<Vec<String>>>,
    flags: Arc<Mutex<Vec<(String, Arc<std::sync::atomic::AtomicBool>)>>>,
}

impl Loads {
    /// A session with photos at `paths` (600×400) that load through these loads.
    fn session(&self, paths: &[&str]) -> Session {
        let mut s = Session::new();
        for (i, path) in paths.iter().enumerate() {
            let id = lightcraft_catalog::PhotoId(i as u64 + 1);
            let p = lightcraft_catalog::Photo::new(
                id,
                lightcraft_catalog::Source::File { path: path.to_string() },
                "x.jpg",
                "JPEG",
                600,
                400,
                "2026-01-01T00:00:00",
            );
            s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        let me = self.clone();
        s.media.file_loader = Some(Arc::new(move |path: &str, _| {
            let Some(flag) = lightcraft_engine::memory::cancel_flag() else { return Err("no flag".into()) };
            let already = flag.load(std::sync::atomic::Ordering::Relaxed);
            me.calls.lock().unwrap().push(format!("{} {path}", if already { "start-cancelled" } else { "start" }));
            me.flags.lock().unwrap().push((path.to_string(), flag.clone()));
            let t = std::time::Instant::now();
            while !flag.load(std::sync::atomic::Ordering::Relaxed) && t.elapsed() < T {
                std::thread::sleep(Duration::from_millis(5));
            }
            me.calls.lock().unwrap().push(format!("stop {path}"));
            Err("cancelled".into())
        }));
        s
    }

    fn count(&self, what: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| *c == what).count()
    }

    /// Set every flag handed out (so no load waits on after the test).
    fn stop_all(&self) {
        for (_, f) in self.flags.lock().unwrap().iter() {
            f.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Moving on to another photo stops the download the loupe started for the one before
/// (`memory::cancellable` around the render; `Want::Load` takes its flag).
#[test]
fn moving_on_stops_the_last_photos_load() {
    let loads = Loads::default();
    let (urls, revealed) = Default::default();
    let mut h = app(loads.session(&["/nas/a.jpg", "/nas/b.jpg"]), &urls, &revealed);
    h.app.session.execute("library.select", &json!({"ids": [1]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    assert!(h.step_until(T, |_| loads.count("start /nas/a.jpg") == 1), "{:?}", loads.calls.lock().unwrap());
    h.app.session.execute("library.select", &json!({"ids": [2]})).unwrap();
    let t = std::time::Instant::now();
    assert!(h.step_until(T, |_| loads.count("stop /nas/a.jpg") == 1), "{:?}", loads.calls.lock().unwrap());
    assert!(t.elapsed() < Duration::from_secs(5), "a's load stopped when b opened, not at its timeout");
    assert!(h.step_until(T, |_| loads.count("start /nas/b.jpg") == 1), "{:?}", loads.calls.lock().unwrap());
    // and a slider drag's renders of the same photo don't stop its load
    h.app.session.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    for _ in 0..30 {
        h.step();
    }
    assert_eq!(loads.count("stop /nas/b.jpg"), 0, "{:?}", loads.calls.lock().unwrap());
    loads.stop_all();
}

/// The download row's ✕ stops the photo's load (it sets the render's flag); the photo's next render
/// (an edit) starts it again instead of being cancelled at once.
#[test]
fn a_cancelled_load_starts_again_on_the_next_render() {
    let loads = Loads::default();
    let (urls, revealed) = Default::default();
    let mut h = app(loads.session(&["/nas/a.jpg"]), &urls, &revealed);
    h.app.session.execute("library.select", &json!({"ids": [1]})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    assert!(h.step_until(T, |_| loads.count("start /nas/a.jpg") == 1), "{:?}", loads.calls.lock().unwrap());
    loads.stop_all();
    assert!(h.step_until(T, |_| loads.count("stop /nas/a.jpg") == 1), "{:?}", loads.calls.lock().unwrap());
    h.app.session.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    assert!(h.step_until(T, |_| loads.count("start /nas/a.jpg") == 2), "{:?}", loads.calls.lock().unwrap());
    assert_eq!(loads.count("start-cancelled /nas/a.jpg"), 0);
    loads.stop_all();
}

/// Renders a view no longer needs stop their downloads: a released window, the loupe of a Nextcloud
/// photo once the Detail view is left (a local photo's render goes on), and every render when
/// another library opens. Thumbnails never get a flag.
#[test]
fn views_that_go_stop_their_downloads() {
    let loads = Loads::default();
    let (urls, revealed) = Default::default();
    let mut h = app(loads.session(&["nextcloud://alice@example.com/a.jpg", "/nas/b.jpg"]), &urls, &revealed);
    let (a, b) = ("nextcloud://alice@example.com/a.jpg", "/nas/b.jpg");
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    // (the Nextcloud photo's load stops in the grid; the local photo's render goes on)
    for (id, path, stops) in [(1, a, 1), (2, b, 0)] {
        h.app.session.execute("library.select", &json!({"ids": [id]})).unwrap();
        assert!(h.step_until(T, |_| loads.count(&format!("start {path}")) == 1), "{:?}", loads.calls.lock().unwrap());
        h.app.ui.view = ViewMode::PhotoGrid;
        for _ in 0..30 {
            h.step();
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(loads.count(&format!("stop {path}")), stops, "{path}: {:?}", loads.calls.lock().unwrap());
        h.app.ui.view = ViewMode::Detail;
    }
    assert!(h.app.renderer.photo_in(crate::render::Slot::Thumb(lightcraft_catalog::PhotoId(2))).is_none(), "no flag for thumbnails");
    // another library: every render stops (now, not at the load's own timeout)
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.renderer.forget_all();
    assert!(h.step_until(Duration::from_secs(5), |_| loads.count(&format!("stop {b}")) == 1), "{:?}", loads.calls.lock().unwrap());
    // a window the view released
    let job = h.app.session.render_job(lightcraft_catalog::PhotoId(2), 64, 64, false, true).unwrap();
    h.app.renderer.request(crate::render::Slot::Region, job, 99);
    assert!(h.step_until(T, |_| loads.count(&format!("start {b}")) == 2), "{:?}", loads.calls.lock().unwrap());
    h.app.renderer.release(crate::render::Slot::Region);
    assert!(h.step_until(Duration::from_secs(5), |_| loads.count(&format!("stop {b}")) == 2), "{:?}", loads.calls.lock().unwrap());
    loads.stop_all();
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

    // a settings file that can't be written: the change is refused, and says so
    std::fs::remove_file(d.join("nextcloud.json")).unwrap();
    std::fs::create_dir(d.join("nextcloud.json")).unwrap();
    click(&mut h, "button:settingsNextcloudCache-1");
    assert_eq!(h.app.session.remote.cache_budget_mb(), 50_000);
    assert!(h.app.ui.toast.as_ref().is_some_and(|t| t.0.contains("nextcloud.json")), "toast: {:?}", h.app.ui.toast);
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

/// A Nextcloud folder's menu in the sidebar offers Show in Finder switched off: there is no folder
/// on this computer to show.
#[test]
fn nextcloud_folders_are_not_shown_in_finder() {
    let d = temp("folder-finder");
    let mut s = session(&d, &[]);
    let p = lightcraft_catalog::Photo::new(
        lightcraft_catalog::PhotoId(1),
        lightcraft_catalog::Source::File { path: "nextcloud://alice@example.com/Photos/a.jpg".into() },
        "a.jpg",
        "JPEG",
        60,
        40,
        "2026-01-01T00:00:00",
    );
    s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let (urls, revealed) = Default::default();
    let mut h = app(s, &urls, &revealed);
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.ui.left_panel = true;
    h.settle(T);
    let rows: Vec<String> = h.app.widgets.iter().map(|(w, _)| w.clone()).filter(|w| w.starts_with("source:libfolder:nextcloud://")).collect();
    let row = rows.last().unwrap_or_else(|| panic!("no Nextcloud folder row: {:?}", h.app.widgets.iter().map(|(w, _)| w).collect::<Vec<_>>()));
    let at = h.app.widgets.iter().find(|(w, _)| w == row).unwrap().1.center();
    let r = h.request("ui.click", json!({"x": at.x, "y": at.y, "button": "right"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.widgets.iter().any(|(w, _)| w == "button:folderReveal"), "the menu has Show in Finder");
    click(&mut h, "button:folderReveal");
    assert!(revealed.lock().unwrap().is_empty(), "{:?}", revealed.lock().unwrap());
}

/// One import is one account: folders chosen on one are dropped when the picker goes to another.
#[test]
fn switching_accounts_drops_the_folders_chosen() {
    let (one, two) = (FakeServer::start(), FakeServer::start());
    for fake in [&one, &two] {
        fake.add_file("/Photos/a.jpg", vec![0; 10]);
    }
    let d = temp("two-accounts");
    let (urls, revealed) = Default::default();
    let mut h = app(session(&d, &[one.account(), two.account()]), &urls, &revealed);
    h.app.run("file.importNextcloud", json!({})).unwrap();
    // (the picker lists the accounts in the settings' order)
    click(&mut h, "button:nextcloudAccount-0");
    assert!(h.step_until(T, |h| listed(h) == ["Photos"]), "{:?}", h.app.ui.dialog);
    click(&mut h, "check:nextcloudFolder-0");
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::NextcloudImport { chosen, .. }) if *chosen == [remote(&one, "Photos")]));
    click(&mut h, "button:nextcloudUp");
    click(&mut h, "button:nextcloudAccount-1");
    assert!(h.step_until(T, |h| listed(h) == ["Photos"]), "{:?}", h.app.ui.dialog);
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::NextcloudImport { chosen, .. }) if chosen.is_empty()), "{:?}", h.app.ui.dialog);
}
