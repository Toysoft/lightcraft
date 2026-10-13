//! Originals of Nextcloud photos in Develop (against craft's `FakeServer`): the open photo's copy is
//! held from eviction, the next photo is prefetched (once, and not after moving on), the first
//! download of an original completes the photo's record (and builds its smart preview), and
//! copies made available offline are pinned.

use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use craft_nextcloud::RemotePath;
use craft_nextcloud::testing::Fault;
use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::cmd::previews::has_smart_preview;
use crate::media::SourceLevel;
use crate::remote::{AccountState, Event};
use crate::tests_nextcloud_hooks::{cached_files, jpeg, raw_blob, remote, signed_in, wait_until};
use crate::{EngineError, Selection, Session, find_command};

fn rows(s: &Session) -> Vec<(&'static str, String)> {
    s.activity.list().into_iter().map(|t| (t.kind, t.label)).collect()
}

/// Whether the original of `path` is on this computer (its etag is known by then: no request).
fn is_cached(s: &Session, path: &str) -> bool {
    let rp = RemotePath::parse(path).unwrap();
    let (etag, _) = s.remote.etag_and_size(&rp).unwrap();
    s.remote.cache().unwrap().cached(&rp, &etag).is_some()
}

/// Add `path` to the catalog as an imported photo of `kind` the listing described (no camera, no
/// dimensions, no size).
fn add(s: &mut Session, id: u64, path: &str, kind: MediaKind) -> PhotoId {
    let name = path.rsplit('/').next().unwrap();
    let mut p = Photo::new(PhotoId(id), Source::File { path: path.into() }, name, "", 0, 0, "2026-10-13");
    p.kind = kind;
    p.develop = Arc::new(p.camera_defaults());
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    PhotoId(id)
}

/// Downloads wait for a message on the returned sender once their row is up.
fn step_downloads(s: &Session) -> mpsc::Sender<()> {
    let (step, steps) = mpsc::channel();
    *s.remote.download_steps.lock().unwrap() = Some(steps);
    step
}

/// Give the open originals cache a budget below the settings' 1000 MB minimum (it keeps it: the
/// settings don't change).
fn small_budget(s: &Session, bytes: u64) {
    s.remote.cache().unwrap().set_budget(bytes);
}

/// Photos `names` (`.ARW` blobs of `len` bytes) on the server, their etags known (so no `stat`
/// comes before their downloads).
fn listed<const N: usize>(s: &Session, fake: &craft_nextcloud::testing::FakeServer, names: [&str; N], len: usize) -> [String; N] {
    names.map(|n| {
        fake.add_file(&format!("/Photos/{n}.ARW"), raw_blob(len));
        let p = remote(fake, &format!("Photos/{n}.ARW"));
        s.remote.etag_and_size(&RemotePath::parse(&p).unwrap()).unwrap();
        p
    })
}

fn row(label: &str) -> Vec<(&'static str, String)> {
    vec![("download", label.to_string())]
}

#[test]
fn the_open_photo_is_held() {
    let (s, fake, _d) = signed_in("held");
    let [a, b, c] = ["a", "b", "c"].map(|n| {
        fake.add_file(&format!("/Photos/{n}.ARW"), raw_blob(600_000));
        remote(&fake, &format!("Photos/{n}.ARW"))
    });
    // room for one copy, not two
    small_budget(&s, 1_000_000);
    s.remote.local_original(&a).unwrap();
    s.remote.hold(Some(&a));
    s.remote.local_original(&b).unwrap();
    assert!(is_cached(&s, &a), "the open photo's copy stays");
    assert!(is_cached(&s, &b));
    // moving on releases it
    s.remote.hold(Some(&c));
    s.remote.local_original(&c).unwrap();
    assert!(!is_cached(&s, &a) && !is_cached(&s, &b), "evicted like any other copy");
    s.remote.hold(None);
    s.remote.local_original(&a).unwrap();
    assert!(!is_cached(&s, &c), "none held");
    // (a local path holds nothing)
    s.remote.hold(Some("/tmp/x.jpg"));
}

#[test]
fn prefetch_downloads_the_next_photo_once() {
    let (s, fake, d) = signed_in("prefetch");
    fake.add_file("/Photos/b.ARW", raw_blob(4000));
    let b = remote(&fake, "Photos/b.ARW");
    let step = step_downloads(&s);
    s.remote.prefetch(Some(&b));
    wait_until("the prefetch row", || !s.activity.list().is_empty());
    let row = s.activity.list().remove(0);
    assert_eq!((row.kind, row.label.as_str(), row.cancellable), ("download", "Prefetching b.ARW", true));
    step.send(()).unwrap();
    wait_until("the prefetch ends", || s.activity.list().is_empty());
    assert!(is_cached(&s, &b));
    assert_eq!(fake.downloads(), 1);
    assert!(matches!(s.remote.take_events().as_slice(), [Event::Downloaded { path, .. }] if *path == b), "it completes the record");
    // asked again (after another photo): it's here, nothing is asked
    s.remote.prefetch(None);
    let before = fake.requests();
    s.remote.prefetch(Some(&b));
    // asked every frame, it isn't looked for again (a copy gone meanwhile goes unnoticed)
    for f in cached_files(&d) {
        std::fs::remove_file(f).unwrap();
    }
    s.remote.prefetch(Some(&b));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(fake.requests(), before);
    assert!(s.activity.list().is_empty());
    // local photos aren't prefetched
    s.remote.prefetch(Some("/tmp/x.jpg"));
    assert_eq!(fake.requests(), before);
}

#[test]
fn prefetch_is_cancelled_by_moving_on() {
    let (s, fake, _d) = signed_in("prefetch-cancel");
    // (long enough that b's request is still stalled when the test moves on)
    s.remote.set_stall_limit(Duration::from_secs(5));
    let [b, c] = listed(&s, &fake, ["b", "c"], 4000);
    // (the etags are known: the stall hits the `stat` b's fetch starts with, once its row is up)
    fake.fail_next(Fault::Stall);
    let before = fake.requests();
    s.remote.prefetch(Some(&b));
    wait_until("b's row", || rows(&s) == row("Prefetching b.ARW"));
    wait_until("b's fetch stalls in its stat", || fake.requests() > before);
    s.remote.prefetch(Some(&c));
    wait_until("c is prefetched instead", || is_cached(&s, &c) && s.activity.list().is_empty());
    assert!(!is_cached(&s, &b), "b never downloads");
    assert_eq!(fake.downloads(), 1, "c only");
    assert_eq!(s.remote.accounts()[0].1, AccountState::SignedIn, "a cancelled download says nothing about the server");
    // moving on to no photo cancels too
    s.remote.prefetch(None);
}

#[test]
fn prefetches_wait_for_the_download_running() {
    let (s, fake, _d) = signed_in("prefetch-queue");
    let [a, b, c] = ["a", "b", "c"].map(|n| {
        fake.add_file(&format!("/Photos/{n}.ARW"), raw_blob(4000));
        let p = remote(&fake, &format!("Photos/{n}.ARW"));
        s.remote.etag_and_size(&RemotePath::parse(&p).unwrap()).unwrap();
        p
    });
    let step = step_downloads(&s);
    let remote = s.remote.clone();
    let loading = std::thread::spawn(move || remote.local_original(&a));
    wait_until("the loupe's download", || rows(&s) == [("download", "Downloading a.ARW (1 MB)".to_string())]);
    // b waits its turn, then c is wanted instead: b is never asked for
    s.remote.prefetch(Some(&b));
    s.remote.prefetch(Some(&c));
    step.send(()).unwrap();
    loading.join().unwrap().unwrap();
    wait_until("c's turn", || rows(&s) == [("download", "Prefetching c.ARW".to_string())]);
    step.send(()).unwrap();
    wait_until("c is here", || is_cached(&s, &c) && s.activity.list().is_empty());
    assert!(!is_cached(&s, &b));
    assert_eq!(fake.downloads(), 2);
}

#[test]
fn loads_go_before_prefetches() {
    let (s, fake, _d) = signed_in("loads-first");
    let [x, a, b] = listed(&s, &fake, ["x", "a", "b"], 4000);
    let step = step_downloads(&s);
    let r = s.remote.clone();
    let first = std::thread::spawn(move || r.local_original(&x));
    wait_until("x downloads", || rows(&s) == row("Downloading x.ARW (1 MB)"));
    // a prefetch and the loupe's load both wait for x: the load goes first
    s.remote.prefetch(Some(&b));
    let r = s.remote.clone();
    let loading = std::thread::spawn(move || r.local_original(&a));
    wait_until("the load waits", || s.remote.loads_waiting.load(Ordering::SeqCst) == 1);
    step.send(()).unwrap();
    first.join().unwrap().unwrap();
    wait_until("the next download", || !s.activity.list().is_empty());
    assert_eq!(rows(&s), row("Downloading a.ARW (1 MB)"));
    step.send(()).unwrap();
    loading.join().unwrap().unwrap();
    wait_until("then the prefetch", || rows(&s) == row("Prefetching b.ARW"));
    step.send(()).unwrap();
    wait_until("b is here", || is_cached(&s, &b) && s.activity.list().is_empty());
    assert_eq!(fake.downloads(), 3);
}

#[test]
fn opening_the_prefetched_photo_keeps_its_download() {
    let (s, fake, _d) = signed_in("prefetch-open");
    let [b, c] = listed(&s, &fake, ["b", "c"], 4000);
    let step = step_downloads(&s);
    s.remote.prefetch(Some(&b));
    wait_until("b's prefetch", || rows(&s) == row("Prefetching b.ARW"));
    // Develop opens b and prefetches the next photo: b's prefetch goes on, and its load waits for it
    s.remote.hold(Some(&b));
    s.remote.prefetch(Some(&c));
    let (r, open) = (s.remote.clone(), b.clone());
    let loading = std::thread::spawn(move || r.local_original(&open));
    wait_until("the load waits", || s.remote.loads_waiting.load(Ordering::SeqCst) == 1);
    step.send(()).unwrap();
    wait_until("the load finds b's copy", || {
        assert!(!rows(&s).iter().any(|(_, l)| l.starts_with("Downloading b")), "no second download row");
        loading.is_finished()
    });
    loading.join().unwrap().unwrap();
    wait_until("c's turn", || rows(&s) == row("Prefetching c.ARW"));
    step.send(()).unwrap();
    wait_until("c is here", || is_cached(&s, &c) && s.activity.list().is_empty());
    assert_eq!(fake.downloads(), 2, "b once, c once");
    let paths: Vec<String> =
        s.remote.take_events().into_iter().filter_map(|e| if let Event::Downloaded { path, .. } = e { Some(path) } else { None }).collect();
    assert_eq!(paths, [b, c]);
}

/// A load run by work that moved on to another photo (a render whose view shows another photo now:
/// its thread's `memory::cancellable` flag) stops: while it waits for the download running, it
/// starts no download; once its own runs, it stops there. The next photo's load goes ahead.
#[test]
fn a_load_stops_when_its_work_moves_on() {
    let (s, fake, _d) = signed_in("load-moves-on");
    let [a, b, c] = listed(&s, &fake, ["a", "b", "c"], 4000);
    let step = step_downloads(&s);
    let load = |path: &String, flag: Option<Arc<std::sync::atomic::AtomicBool>>| {
        let (r, path) = (s.remote.clone(), path.clone());
        std::thread::spawn(move || match flag {
            Some(flag) => crate::memory::cancellable(flag, || r.local_original(&path)),
            None => r.local_original(&path),
        })
    };
    let downloading = |name: &str| rows(&s).iter().any(|(_, l)| l.starts_with(&format!("Downloading {name}")));
    // a waits for c's download, then its view moves on: no row, no request
    let loading_c = load(&c, None);
    wait_until("c's download", || downloading("c.ARW"));
    let moved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let loading_a = load(&a, Some(moved.clone()));
    wait_until("a waits", || s.remote.loads_waiting.load(Ordering::SeqCst) == 1);
    moved.store(true, Ordering::Relaxed);
    step.send(()).unwrap();
    assert!(loading_c.join().unwrap().is_ok());
    wait_until("a gives up", || {
        assert!(!downloading("a.ARW"), "no row for a");
        loading_a.is_finished()
    });
    assert_eq!(loading_a.join().unwrap().unwrap_err(), craft_nextcloud::Error::Cancelled.to_string());
    assert!(!is_cached(&s, &a));
    // a's own download, stopped once its view moves on
    let moved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let loading_a = load(&a, Some(moved.clone()));
    wait_until("a's download", || downloading("a.ARW"));
    moved.store(true, Ordering::Relaxed);
    step.send(()).unwrap();
    assert_eq!(loading_a.join().unwrap().unwrap_err(), craft_nextcloud::Error::Cancelled.to_string());
    wait_until("a's row goes", || !downloading("a.ARW"));
    assert!(!is_cached(&s, &a));
    // the next photo's load goes ahead
    let loading_b = load(&b, Some(Arc::new(std::sync::atomic::AtomicBool::new(false))));
    wait_until("b's download", || downloading("b.ARW"));
    step.send(()).unwrap();
    assert!(loading_b.join().unwrap().is_ok());
    assert!(is_cached(&s, &b));
    assert_eq!(fake.downloads(), 2, "c and b; never a");
    assert_eq!(s.remote.accounts()[0].1, AccountState::SignedIn, "a stopped download isn't the server's fault");
}

#[test]
fn moving_on_cancels_the_open_photos_prefetch() {
    let (s, fake, _d) = signed_in("prefetch-skim");
    let [b, c, e] = listed(&s, &fake, ["b", "c", "e"], 4000);
    let step = step_downloads(&s);
    s.remote.prefetch(Some(&b));
    wait_until("b's prefetch", || rows(&s) == row("Prefetching b.ARW"));
    // skimming in Develop: b opens and c is prefetched, then c opens
    s.remote.hold(Some(&b));
    s.remote.prefetch(Some(&c));
    s.remote.hold(Some(&c));
    step.send(()).unwrap();
    wait_until("c's turn", || rows(&s) == row("Prefetching c.ARW"));
    step.send(()).unwrap();
    wait_until("c is here", || is_cached(&s, &c) && s.activity.list().is_empty());
    assert!(!is_cached(&s, &b), "b's prefetch stopped");
    assert_eq!(fake.downloads(), 1, "c only");
    // leaving Develop stops the open photo's prefetch too
    s.remote.prefetch(Some(&e));
    wait_until("e's prefetch", || rows(&s) == row("Prefetching e.ARW"));
    s.remote.hold(Some(&e));
    s.remote.hold(None);
    step.send(()).unwrap();
    wait_until("e's row goes", || s.activity.list().is_empty());
    assert!(!is_cached(&s, &e));
    assert_eq!(fake.downloads(), 1);
}

#[test]
fn batch_downloads_skip_an_unreachable_server() {
    let (mut s, fake, _d) = signed_in("prefetch-offline");
    fake.add_file("/Photos/b.ARW", raw_blob(4000));
    let b = remote(&fake, "Photos/b.ARW");
    add(&mut s, 1, &b, MediaKind::Raw);
    s.remote.note_error(&fake.account().key(), &craft_nextcloud::Error::Unreachable("timed out".into()));
    let before = fake.requests();
    s.remote.prefetch(Some(&b));
    std::thread::sleep(Duration::from_millis(100));
    let r = s.execute("photo.makeAvailableOffline", &json!({"ids": [1], "wait": true})).unwrap();
    assert_eq!(r, json!({"photos": 0, "failed": 1}));
    assert_eq!(fake.requests(), before, "no request while it doesn't answer");
    assert!(s.activity.list().is_empty());
}

#[test]
fn first_download_completes_the_record() {
    let (mut s, fake, d) = signed_in("complete");
    s.media.smart_dir = Some(d.join("smart"));
    let bytes = jpeg(96, 64, None);
    fake.add_file("/Photos/x.jpg", bytes.clone());
    fake.set_preview("/Photos/x.jpg", jpeg(64, 32, None));
    let meta = lightcraft_meta::Metadata { make: Some("Sony".into()), model: Some("ILCE-7RM6".into()), ..Default::default() };
    fake.add_file("/Photos/y.dng", crate::tests_xmp::synthetic_dng_with(None, meta));
    let (xp, yp) = (remote(&fake, "Photos/x.jpg"), remote(&fake, "Photos/y.dng"));
    let x = add(&mut s, 1, &xp, MediaKind::Image);
    let y = add(&mut s, 2, &yp, MediaKind::Raw);
    // R31: listed from the Memories timeline (its UTC instant), two hours early
    s.catalog.apply(Op::SetCaptured { id: x, captured: Some("2024-09-10T10:34:56".into()) }).unwrap();
    // the grid's thumbnail stands in the server's preview
    let thumb = crate::memory::in_background(|| s.source_now(x, SourceLevel::Thumb)).unwrap();
    assert_eq!((thumb.width, thumb.height), (64, 32));
    // Develop opens them: the originals download
    let load = s.media.file_loader.clone().unwrap();
    load(&xp, 512).unwrap();
    load(&yp, 512).unwrap();
    // (Develop's render of the original, accepted before the drain)
    s.source_now(x, SourceLevel::Preview).unwrap();
    assert!(s.catalog.photo(x).unwrap().meta.camera.is_empty(), "nothing changes off the session");
    s.drain_remote();
    let px = s.catalog.photo(x).unwrap();
    assert_eq!((px.width, px.height, px.file_size, px.content_hash.as_deref()), (96, 64, bytes.len() as u64, None));
    assert_eq!((px.meta.camera.as_str(), px.captured.as_deref()), ("Canon EOS R8", Some("2024-09-10T12:34:56")));
    let py = s.catalog.photo(y).unwrap();
    assert_eq!((py.width, py.height, py.meta.camera.as_str(), py.content_hash.as_deref()), (32, 24, "Sony ILCE-7RM6", None));
    assert!(s.undo.is_empty(), "not an undo step");
    // R15: the stand-in is gone; pixel statistics (Auto Tone, the WB picker) read the original now
    assert!(!s.media.has_source(x, SourceLevel::Thumb));
    assert!(s.media.has_source(x, SourceLevel::Preview), "a source decoded from the original stays");
    let thumb = s.source_now(x, SourceLevel::Thumb).unwrap();
    assert_eq!((thumb.width, thumb.height), (96, 64));
    // their smart previews are built in the background, from the downloaded copies
    wait_until("smart previews", || has_smart_preview(&s, x) && has_smart_preview(&s, y));
    wait_until("the build ends", || s.activity.list().is_empty());
    assert_eq!(fake.downloads(), 2);
    // a load from the cache isn't a download: nothing more to complete
    load(&xp, 512).unwrap();
    assert!(s.remote.take_events().is_empty());
}

/// R38: the first download sets the file's capture time only on a photo the metadata phase never
/// read (no camera: its time is the listing's estimate). One it read keeps its time, which may be the
/// user's (Edit Capture Time before the photo was first opened).
#[test]
fn a_download_keeps_an_edited_capture_time() {
    let (mut s, fake, _d) = signed_in("edited-time");
    let [ap, bp] = ["a", "b"].map(|n| {
        fake.add_file(&format!("/Photos/{n}.jpg"), jpeg(96, 64, None));
        remote(&fake, &format!("Photos/{n}.jpg"))
    });
    let (a, b) = (add(&mut s, 1, &ap, MediaKind::Image), add(&mut s, 2, &bp, MediaKind::Image));
    let mut read = s.catalog.photo(a).unwrap().meta.clone();
    read.camera = "Canon EOS R8".into();
    s.catalog.apply(Op::SetMeta { id: a, meta: Box::new(read) }).unwrap();
    s.execute("library.select", &json!({"ids": [a.0, b.0]})).unwrap();
    s.execute("photo.setCaptureTime", &json!({"time": "2020-01-01T00:00:00", "each": true})).unwrap();
    let load = s.media.file_loader.clone().unwrap();
    load(&ap, 512).unwrap();
    load(&bp, 512).unwrap();
    s.drain_remote();
    assert_eq!(s.catalog.photo(a).unwrap().captured.as_deref(), Some("2020-01-01T00:00:00"), "the user's time");
    assert_eq!(s.catalog.photo(b).unwrap().captured.as_deref(), Some("2024-09-10T12:34:56"), "the file's local time");
}

#[test]
fn the_downloading_thread_describes_the_photo() {
    let (mut s, fake, d) = signed_in("described");
    fake.add_file("/Photos/x.jpg", jpeg(96, 64, None));
    let xp = remote(&fake, "Photos/x.jpg");
    let x = add(&mut s, 1, &xp, MediaKind::Image);
    s.remote.local_original(&xp).unwrap();
    // the drain reads no file (the copy may even be gone by then)
    for f in cached_files(&d) {
        std::fs::remove_file(f).unwrap();
    }
    s.drain_remote();
    let px = s.catalog.photo(x).unwrap();
    assert_eq!((px.width, px.height, px.meta.camera.as_str(), px.content_hash.as_deref()), (96, 64, "Canon EOS R8", None));
}

#[test]
fn copies_on_disk_are_not_downloads() {
    let (s, fake, _d) = signed_in("on-disk");
    fake.add_file("/Photos/x.jpg", jpeg(96, 64, None));
    let x = remote(&fake, "Photos/x.jpg");
    // the folder's listing remembers x's etag; then x changes on the server
    s.remote.list(&RemotePath::parse(&remote(&fake, "Photos")).unwrap()).unwrap();
    fake.add_file("/Photos/x.jpg", jpeg(128, 64, None));
    s.remote.local_original(&x).unwrap();
    let events = s.remote.take_events();
    let [Event::Downloaded { info: Some(info), .. }] = events.as_slice() else { panic!("one download, described") };
    assert_eq!((info.width, info.content_hash.as_deref()), (128, None), "the new version, no hash");
    // (the etag remembered is still the old one: the download finds the new copy on disk)
    s.remote.local_original(&x).unwrap();
    assert!(s.remote.take_events().is_empty(), "a copy on disk isn't a download");
    assert_eq!(fake.downloads(), 1);
}

#[test]
fn offline_pins_survive_eviction() {
    let (mut s, fake, _d) = signed_in("offline");
    let [a, b, c, e] = ["a", "b", "c", "e"].map(|n| {
        fake.add_file(&format!("/Photos/{n}.ARW"), raw_blob(400_000));
        remote(&fake, &format!("Photos/{n}.ARW"))
    });
    add(&mut s, 1, &a, MediaKind::Raw);
    add(&mut s, 2, &b, MediaKind::Raw);
    small_budget(&s, 1_000_000);
    let r = s.execute("photo.makeAvailableOffline", &json!({"ids": [1, 2], "wait": true})).unwrap();
    assert_eq!(r, json!({"photos": 2, "failed": 0}));
    assert!(s.activity.list().is_empty());
    let pinned = |s: &mut Session| s.execute("nextcloud.preferences", &json!({})).unwrap()["pinnedBytes"].as_u64().unwrap();
    assert_eq!(pinned(&mut s), 800_000);
    // more downloads than the budget holds: the pinned copies stay, the others go
    s.remote.local_original(&c).unwrap();
    s.remote.local_original(&e).unwrap();
    assert!(is_cached(&s, &a) && is_cached(&s, &b));
    assert!(!is_cached(&s, &c));
    assert_eq!(s.execute("photo.removeOfflineCopy", &json!({"ids": [1, 2]})).unwrap(), json!({"photos": 2}));
    assert_eq!(pinned(&mut s), 0);
}

#[test]
fn offline_copies_show_in_activity() {
    let (mut s, fake, _d) = signed_in("offline-rows");
    for (id, n) in [(1, "a"), (2, "b")] {
        fake.add_file(&format!("/Photos/{n}.ARW"), raw_blob(2_600_000));
        add(&mut s, id, &remote(&fake, &format!("Photos/{n}.ARW")), MediaKind::Raw);
    }
    // one is missing on the server: the other is still made available
    fake.remove("/Photos/a.ARW");
    let step = step_downloads(&s);
    let r = s.execute("photo.makeAvailableOffline", &json!({"ids": [1, 2]})).unwrap();
    assert_eq!((r["started"].clone(), r["photos"].clone()), (json!(true), json!(2)));
    let task = r["task"].as_u64().unwrap();
    wait_until("b downloads", || s.activity.list().len() == 2);
    let list = s.activity.list();
    let offline = list.iter().find(|t| t.id == task).unwrap();
    assert_eq!((offline.kind, offline.label.as_str(), offline.cancellable), ("download", "Making photos available offline", true));
    assert_eq!((offline.done, offline.total), (1, 2), "a failed");
    assert!(list.iter().any(|t| t.label == "Downloading b.ARW (3 MB)"));
    step.send(()).unwrap();
    wait_until("the rows go", || s.activity.list().is_empty());
    assert_eq!(fake.downloads(), 1);
    assert_eq!(s.execute("nextcloud.preferences", &json!({})).unwrap()["pinnedBytes"], json!(2_600_000));
}

#[test]
fn offline_commands_ignore_local_photos() {
    let (mut s, fake, d) = signed_in("offline-local");
    fake.add_file("/Photos/a.ARW", raw_blob(4000));
    let file = d.join("local.jpg");
    std::fs::write(&file, jpeg(96, 64, None)).unwrap();
    let local = add(&mut s, 1, &file.to_string_lossy(), MediaKind::Image);
    let a = add(&mut s, 2, &remote(&fake, "Photos/a.ARW"), MediaKind::Raw);
    let r = s.execute("photo.makeAvailableOffline", &json!({"ids": [1, 2], "wait": true})).unwrap();
    assert_eq!(r, json!({"photos": 1, "failed": 0}));
    assert_eq!(fake.downloads(), 1);
    // only a local photo selected (or named): both are off
    s.selection = Selection { ids: vec![local], active: Some(local) };
    for id in ["photo.makeAvailableOffline", "photo.removeOfflineCopy"] {
        let spec = find_command(id).unwrap();
        assert_eq!(spec.menu, ["Photo"]);
        assert!(!spec.info(&s).enabled, "{id}");
        assert!(matches!(s.execute(id, &json!({})), Err(EngineError::Disabled(..))), "{id}");
        assert!(matches!(s.execute(id, &json!({"ids": [1]})), Err(EngineError::Disabled(..))), "{id} named");
    }
    s.selection = Selection { ids: vec![local, a], active: Some(a) };
    assert!(find_command("photo.removeOfflineCopy").unwrap().info(&s).enabled);
    assert_eq!(s.execute("photo.removeOfflineCopy", &json!({})).unwrap(), json!({"photos": 1}));
}
