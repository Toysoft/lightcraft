//! Browsing and importing Nextcloud folders (`nextcloud.browse`, `library.importNextcloud`, against
//! craft's `FakeServer`): listings, the Memories join, metadata from Memories or the file's first
//! bytes, rescans, activity rows, awkward names, and never downloading an original.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;

use craft_nextcloud::testing::Fault;

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, Samples, encode_jpeg};
use serde_json::{Value, json};

use crate::Session;
use crate::media::{ProbeInfo, SourceLevel};
use crate::remote::{Event, ImportProgress};
use crate::tests_nextcloud_hooks::{cached_files, jpeg, raw_blob, remote, signed_in, wait_until};

/// A `w`×`h` JPEG without EXIF.
fn plain_jpeg(w: u32, h: u32) -> Vec<u8> {
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 90, 30]).collect();
    encode_jpeg(&EncodeImage::new(w, h, 3, Samples::U8(&px)), 90, ChromaSubsampling::S444, &EncodeMeta::default()).unwrap()
}

fn import(s: &mut Session, dirs: &[String]) -> Value {
    s.execute("library.importNextcloud", &json!({"dirs": dirs, "wait": true})).unwrap()
}

fn photo<'a>(s: &'a Session, path: &str) -> &'a Photo {
    let p = s.catalog.photos().find(|p| p.source == Source::File { path: path.into() });
    p.unwrap_or_else(|| panic!("no photo {path}"))
}

/// Import `task`'s entry in `nextcloud.preferences` → `imports`.
fn outcome(s: &mut Session, task: u64) -> Value {
    let all = s.execute("nextcloud.preferences", &json!({})).unwrap()["imports"].clone();
    all.as_array().unwrap().iter().find(|o| o["task"] == task).cloned().unwrap_or(Value::Null)
}

fn state(s: &Session, task: u64) -> ImportProgress {
    s.remote.imports().into_iter().find(|o| o.task == task).map(|o| o.state).unwrap()
}

/// `w`×`h` RGB pixels of noise (they compress badly: big files).
fn noise(w: u32, h: u32) -> Vec<u8> {
    let mut x = 0x2545_f491_u32;
    (0..w * h * 3)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// A JPEG of noise with the EXIF of [`jpeg`].
fn noisy_jpeg(w: u32, h: u32) -> Vec<u8> {
    let px = noise(w, h);
    let meta = lightcraft_meta::Metadata {
        make: Some("Canon".into()),
        model: Some("EOS R8".into()),
        capture_time: lightcraft_meta::DateTime::parse_exif("2024:09:10 12:34:56"),
        ..Default::default()
    };
    let exif = lightcraft_meta::write_exif(&meta);
    encode_jpeg(&EncodeImage::new(w, h, 3, Samples::U8(&px)), 95, ChromaSubsampling::S444, &EncodeMeta { exif: Some(&exif), ..Default::default() })
        .unwrap()
}

fn rows(s: &Session) -> Vec<(&'static str, String)> {
    s.activity.list().into_iter().map(|t| (t.kind, t.label)).collect()
}

#[test]
fn browse_lists_one_level() {
    let (mut s, fake, _d) = signed_in("import-browse");
    fake.add_dir("/Photos/2024");
    fake.add_dir("/Photos/2023");
    fake.add_dir("/Photos/.thumbs");
    fake.add_file("/Photos/2024/deep.jpg", jpeg(8, 8, None));
    fake.add_file("/Photos/a.jpg", jpeg(8, 8, None));
    fake.add_file("/Photos/b.ARW", raw_blob(100));
    fake.add_file("/Photos/notes.txt", b"hi".to_vec());
    fake.add_file("/Photos/._a.jpg", b"AppleDouble".to_vec());
    // names craft refuses never reach us
    fake.add_raw_entry("/Photos", "..");
    fake.add_raw_entry("/Photos", "evil%2F..%2Fx.jpg");
    fake.add_raw_entry("/Photos", "bell%07.jpg");
    let r = s.execute("nextcloud.browse", &json!({"dir": remote(&fake, "Photos") + "/"})).unwrap();
    assert_eq!(r["dir"], remote(&fake, "Photos"));
    assert_eq!(r["folders"], json!([{"name": "2023", "path": remote(&fake, "Photos/2023")}, {"name": "2024", "path": remote(&fake, "Photos/2024")}]));
    assert_eq!(r["photos"], 2, "a.jpg and b.ARW: not the text file, not hidden files, not the folder's");
    let root = s.execute("nextcloud.browse", &json!({"dir": fake.account().root().to_string()})).unwrap();
    assert_eq!(root["folders"], json!([{"name": "Photos", "path": remote(&fake, "Photos")}]));
    assert!(s.execute("nextcloud.browse", &json!({"dir": "/home/me/Pictures"})).is_err());
    assert!(s.execute("nextcloud.browse", &json!({"dir": remote(&fake, "Nope")})).is_err());
    assert_eq!(s.catalog.len(), 0, "browsing adds nothing");
}

#[test]
fn import_lists_joins_memories_and_reads_metadata() {
    let (mut s, fake, d) = signed_in("import-memories");
    fake.enable_memories();
    let exif =
        json!({"Make": "Sony", "Model": "ILCE-7RM6", "LensModel": "FE 24-70mm F2.8 GM II", "ExposureTime": "1/250", "FNumber": 2.8, "ISO": 100});
    for (i, rel) in ["Photos/a.ARW", "Photos/b.ARW", "Photos/2024/c.ARW"].iter().enumerate() {
        fake.add_file(&format!("/{rel}"), raw_blob(3000));
        fake.set_photo(&format!("/{rel}"), 19675, 6000, 4000, 1_700_000_000 + i as i64, exif.clone());
    }
    let r = import(&mut s, &[remote(&fake, "Photos")]);
    assert_eq!(r, json!({"added": 3, "skipped": 0, "metadata": 3, "failedFolders": 0}));
    for (i, rel) in ["Photos/a.ARW", "Photos/b.ARW", "Photos/2024/c.ARW"].iter().enumerate() {
        let p = photo(&s, &remote(&fake, rel));
        assert_eq!((p.width, p.height, p.kind, p.format.as_str(), p.file_size), (6000, 4000, MediaKind::Raw, "ARW", 3000));
        assert_eq!(p.captured.as_deref(), Some(format!("2023-11-14T22:13:2{i}").as_str()), "Memories' epoch, read as wall time");
        assert_eq!((p.meta.camera.as_str(), p.meta.lens.as_str(), p.meta.shutter.as_str()), ("Sony ILCE-7RM6", "FE 24-70mm F2.8 GM II", "1/250"));
        assert_eq!((p.meta.aperture, p.meta.iso), (Some(2.8), Some(100)));
        assert_eq!(p.content_hash, None);
    }
    assert_eq!(fake.downloads(), 0, "no original was downloaded");
    assert!(cached_files(&d).is_empty());
    assert!(s.activity.list().is_empty(), "the rows are gone");
}

#[test]
fn import_without_memories_probes_headers() {
    let (mut s, fake, _d) = signed_in("import-headers");
    // small enough to be read whole: described from its bytes (oriented)
    fake.add_file("/Photos/x.jpg", jpeg(96, 64, Some(6)));
    // a raw past the 1 MiB window: its metadata and the size its header records (R29)
    let meta = lightcraft_meta::Metadata {
        make: Some("Sony".into()),
        model: Some("ILCE-7RM6".into()),
        lens_model: Some("FE 50mm F1.2 GM".into()),
        exposure_time: Some(1.0 / 250.0),
        f_number: Some(1.2),
        iso: Some(800),
        capture_time: lightcraft_meta::DateTime::parse_exif("2025:01:02 03:04:05"),
        ..Default::default()
    };
    let mut dng = crate::tests_xmp::synthetic_dng_with(None, meta);
    dng.resize(2 << 20, 0);
    fake.add_file("/Photos/big.dng", dng);
    // a JPEG bigger than the window: its metadata, never dimensions from part of it
    let big = noisy_jpeg(400, 300);
    assert!(big.len() > 64 << 10);
    fake.add_file("/Photos/big.jpg", big);
    // a motion photo (a video after the JPEG): its start describes it, but part of a file never
    // sets dimensions (R18)
    let mut motion = jpeg(96, 64, None);
    motion.resize(2 << 20, 0);
    assert!(crate::files::probe_bytes("m.jpg", &motion[..64 << 10]).is_ok_and(|i| i.width == 96), "the start describes it");
    fake.add_file("/Photos/motion.jpg", motion);
    let r = import(&mut s, &[remote(&fake, "Photos")]);
    assert_eq!(r, json!({"added": 4, "skipped": 0, "metadata": 4, "failedFolders": 0}));
    let x = photo(&s, &remote(&fake, "Photos/x.jpg"));
    assert_eq!((x.width, x.height, x.kind, x.format.as_str()), (64, 96, MediaKind::Image, "JPEG"));
    assert_eq!(x.captured.as_deref(), Some("2024-09-10T12:34:56"));
    assert_eq!((x.meta.camera.as_str(), x.content_hash.as_deref()), ("Canon EOS R8", None));
    let big = photo(&s, &remote(&fake, "Photos/big.dng"));
    assert_eq!((big.width, big.height, big.kind, big.format.as_str(), big.file_size), (32, 24, MediaKind::Raw, "DNG", 2 << 20));
    assert_eq!((big.meta.camera.as_str(), big.meta.lens.as_str(), big.meta.shutter.as_str()), ("Sony ILCE-7RM6", "FE 50mm F1.2 GM", "1/250"));
    assert_eq!((big.meta.aperture, big.meta.iso, big.captured.as_deref()), (Some(1.2), Some(800), Some("2025-01-02T03:04:05")));
    assert!(big.preview_only.is_none(), "nothing from part of a raw beyond its metadata");
    let jpg = photo(&s, &remote(&fake, "Photos/big.jpg"));
    assert_eq!((jpg.width, jpg.height, jpg.meta.camera.as_str()), (0, 0, "Canon EOS R8"), "no size in its EXIF: unknown");
    assert_eq!(jpg.captured.as_deref(), Some("2024-09-10T12:34:56"));
    let motion = photo(&s, &remote(&fake, "Photos/motion.jpg"));
    assert_eq!((motion.width, motion.height, motion.meta.camera.as_str()), (0, 0, "Canon EOS R8"));
    assert_eq!(fake.downloads(), 0);
}

/// R31/R29: the metadata phase gives a photo the capture time its file's header (or Memories'
/// `datetaken`) records over the one it had: a listing's is the Memories timeline's UTC instant, hours
/// off the camera's local time. A photo nothing sized gets the size its header records; one with a
/// size keeps it.
#[test]
fn metadata_gives_local_capture_times_and_missing_sizes() {
    let (mut s, fake, _d) = signed_in("import-local-times");
    fake.enable_memories();
    // on the timeline: Memories' `datetaken`
    fake.add_file("/Photos/a.ARW", raw_blob(3000));
    fake.set_photo("/Photos/a.ARW", 19675, 6000, 4000, 1_700_000_000, json!({"Make": "Sony", "Model": "ILCE-7RM6"}));
    // not on it, and past the first window: the header's EXIF
    let meta = lightcraft_meta::Metadata {
        make: Some("Sony".into()),
        model: Some("ILCE-7RM6".into()),
        capture_time: lightcraft_meta::DateTime::parse_exif("2025:01:02 03:04:05"),
        ..Default::default()
    };
    let mut dng = crate::tests_xmp::synthetic_dng_with(None, meta);
    dng.resize(2 << 20, 0);
    for f in ["/Photos/b.dng", "/Photos/c.dng"] {
        fake.add_file(f, dng.clone());
    }
    // a and b are in the library already, as a timeline listing left them: an hour or two early (the
    // fake serves one time for the timeline and `datetaken`), and sized
    for (id, name, captured) in [(1, "a.ARW", "2023-11-14T21:13:20"), (2, "b.dng", "2025-01-02T01:04:05")] {
        let mut p = Photo::new(PhotoId(id), Source::File { path: remote(&fake, &format!("Photos/{name}")) }, name, "", 4000, 6000, "2026-10-13");
        p.kind = MediaKind::Raw;
        p.captured = Some(captured.into());
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    import(&mut s, &[remote(&fake, "Photos")]);
    let a = photo(&s, &remote(&fake, "Photos/a.ARW"));
    assert_eq!((a.captured.as_deref(), a.meta.camera.as_str()), (Some("2023-11-14T22:13:20"), "Sony ILCE-7RM6"), "Memories' datetaken");
    let b = photo(&s, &remote(&fake, "Photos/b.dng"));
    assert_eq!((b.captured.as_deref(), b.width, b.height), (Some("2025-01-02T03:04:05"), 4000, 6000), "the header's time; its size kept");
    let c = photo(&s, &remote(&fake, "Photos/c.dng"));
    assert_eq!((c.captured.as_deref(), c.width, c.height), (Some("2025-01-02T03:04:05"), 32, 24), "the size its header records");
    assert_eq!(fake.downloads(), 0);
}

#[test]
fn rescan_adds_only_new_files_and_finishes_metadata() {
    let (mut s, fake, _d) = signed_in("import-rescan");
    fake.add_file("/Photos/a.jpg", jpeg(96, 64, None));
    fake.add_file("/Photos/b.jpg", plain_jpeg(40, 30));
    let dirs = [remote(&fake, "Photos")];
    assert_eq!(import(&mut s, &dirs), json!({"added": 2, "skipped": 0, "metadata": 2, "failedFolders": 0}));
    assert_eq!(photo(&s, &remote(&fake, "Photos/b.jpg")).meta.camera, "", "b has no camera yet");
    // b gets its EXIF on the server, c is new
    fake.add_file("/Photos/b.jpg", jpeg(40, 30, None));
    fake.add_file("/Photos/c.jpg", jpeg(20, 10, None));
    let before = fake.requests();
    assert_eq!(import(&mut s, &dirs), json!({"added": 1, "skipped": 2, "metadata": 2, "failedFolders": 0}));
    // the listing, Memories' "not installed", and the starts of b and c: a (with its camera) isn't read again
    assert_eq!(fake.requests() - before, 4);
    assert_eq!(s.catalog.len(), 3, "no duplicates");
    assert_eq!(photo(&s, &remote(&fake, "Photos/b.jpg")).meta.camera, "Canon EOS R8");
    assert_eq!(photo(&s, &remote(&fake, "Photos/c.jpg")).meta.camera, "Canon EOS R8");
    assert_eq!(s.undo.len(), 2, "one step per batch of photos added; metadata isn't an undo step");
}

#[test]
fn import_rows_show_in_activity() {
    let (mut s, fake, _d) = signed_in("import-rows");
    for name in ["a", "b", "c"] {
        fake.add_file(&format!("/Photos/{name}.jpg"), jpeg(32, 16, None));
    }
    let (step, steps) = mpsc::channel();
    *s.remote.import_steps.lock().unwrap() = Some(steps);
    let r = s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")]})).unwrap();
    assert_eq!(r["started"], true);
    let task = r["task"].as_u64().unwrap();
    assert_eq!(s.activity.list()[0].id, task, "the task is the row's id");
    assert_eq!(outcome(&mut s, task)["state"], "running");
    let importing = ("import", "Importing from Nextcloud".to_string());
    let scanning = ("scan", "Reading metadata".to_string());
    wait_until("the import row", || rows(&s) == [importing.clone()]);
    step.send(()).unwrap();
    wait_until("the metadata row", || rows(&s) == [scanning.clone()]);
    let scan = s.activity.list().into_iter().next().unwrap();
    assert!(scan.cancellable);
    assert_eq!((scan.done, scan.total), (0, 3));
    step.send(()).unwrap();
    wait_until("the worker's end", || state(&s, task) != ImportProgress::Running);
    assert!(s.activity.list().is_empty());
    assert_eq!(s.catalog.len(), 0, "nothing joins the catalog off the session");
    s.drain_remote();
    assert_eq!(s.catalog.len(), 3);
    assert!(s.catalog.photos().all(|p| p.meta.camera == "Canon EOS R8"));
    s.drain_remote();
    assert_eq!(s.catalog.len(), 3, "a drain with nothing queued changes nothing");
    assert_eq!(outcome(&mut s, task), json!({"task": task, "state": "done", "added": 3, "failedFolders": 0}));
}

#[test]
fn every_command_drains_first() {
    let (mut s, fake, _d) = signed_in("import-drain");
    fake.add_file("/Photos/a.jpg", jpeg(32, 16, None));
    let task = s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")]})).unwrap()["task"].as_u64().unwrap();
    wait_until("the worker's end", || state(&s, task) != ImportProgress::Running);
    s.execute("activity.list", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), 1, "the query applied what the import found");
    assert_eq!(s.catalog.photos().next().unwrap().meta.camera, "Canon EOS R8");
}

/// Review Focus 2: spaces, `#`, `%` and non-ASCII survive catalog → path → URL → load.
#[test]
fn awkward_names_import_and_load() {
    let (mut s, fake, _d) = signed_in("import-awkward");
    fake.add_file("/Datový Trezor/a b#1%.jpg", jpeg(96, 64, None));
    fake.set_preview("/Datový Trezor/a b#1%.jpg", jpeg(64, 32, None));
    let r = import(&mut s, &[remote(&fake, "Datový Trezor")]);
    assert_eq!(r["added"], 1);
    let path = fake.account().root().join("Datový Trezor").unwrap().join("a b#1%.jpg").unwrap().to_string();
    let p = photo(&s, &path);
    assert_eq!((p.file_name.as_str(), p.width, p.height), ("a b#1%.jpg", 96, 64));
    // the thumbnail comes from the server's preview
    let load = s.media.file_loader.clone().unwrap();
    let (thumb, _) = crate::memory::in_background(|| load(&path, SourceLevel::Thumb.max_edge())).unwrap();
    assert_eq!((thumb.width, thumb.height), (64, 32));
    assert_eq!(fake.downloads(), 0);
    // and an interactive load the original
    let (img, _) = load(&path, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64));
    assert_eq!(fake.downloads(), 1);
}

#[test]
fn import_never_downloads_originals() {
    let (mut s, fake, d) = signed_in("import-never");
    fake.enable_memories();
    fake.add_file("/Photos/indexed.ARW", raw_blob(5000));
    fake.set_photo("/Photos/indexed.ARW", 1, 6000, 4000, 1_700_000_000, json!({"Make": "Sony"}));
    // not on the Memories timeline yet: its start is read instead
    fake.add_file("/Photos/new.jpg", jpeg(96, 64, None));
    fake.add_file("/Photos/raw.CR3", raw_blob(3 << 20));
    let before = fake.requests();
    let r = import(&mut s, &[remote(&fake, "Photos")]);
    assert_eq!(r["added"], 3);
    assert!(fake.requests() > before);
    assert_eq!(fake.downloads(), 0, "listings, Memories and Range reads only");
    assert!(cached_files(&d).is_empty());
    assert_eq!(photo(&s, &remote(&fake, "Photos/new.jpg")).meta.camera, "Canon EOS R8");
}

#[test]
fn bad_dirs_are_refused() {
    let (mut s, fake, _d) = signed_in("import-bad");
    fake.add_dir("/Photos");
    let run = |s: &mut Session, dirs: Value| s.execute("library.importNextcloud", &json!({"dirs": dirs, "wait": true}));
    assert!(run(&mut s, json!([])).is_err(), "nothing to import");
    assert!(run(&mut s, json!(["/home/me/Pictures"])).is_err(), "not a Nextcloud folder");
    assert!(run(&mut s, json!(["nextcloud://bob@elsewhere.example/Photos"])).unwrap_err().to_string().contains("No Nextcloud account"));
    assert!(run(&mut s, json!([remote(&fake, "Missing")])).is_err(), "a folder the server doesn't have");
    assert!(s.activity.list().is_empty());
}

/// A hostile server's folder tree: never more than 64 levels deep.
#[test]
fn deep_trees_stop_at_the_path_limit() {
    let (mut s, fake, _d) = signed_in("import-deep");
    let deep: String = (0..70).map(|i| format!("/d{i}")).collect();
    fake.add_file(&format!("{deep}/x.jpg"), jpeg(8, 8, None));
    fake.add_file("/d0/near.jpg", jpeg(8, 8, None));
    let r = import(&mut s, &[remote(&fake, "d0")]);
    assert_eq!(r["added"], 1);
    assert_eq!(fake.downloads(), 0);
}

/// R18: the start of a file is read in two steps: 64 KiB, and 1 MiB only when those had no camera.
#[test]
fn metadata_reads_64_kib_then_1_mib() {
    let (mut s, fake, _d) = signed_in("import-windows");
    let meta = lightcraft_meta::Metadata { make: Some("Sony".into()), model: Some("ILCE-7RM6".into()), ..Default::default() };
    let mut dng = crate::tests_xmp::synthetic_dng_with(None, meta);
    dng.resize(2 << 20, 0);
    fake.add_file("/A/x.dng", dng);
    fake.add_file("/B/y.CR3", raw_blob(3 << 20));
    let before = fake.requests();
    import(&mut s, &[remote(&fake, "A")]);
    assert_eq!(fake.requests() - before, 3, "Memories' 404, the listing, and 64 KiB that had the camera");
    let before = fake.requests();
    import(&mut s, &[remote(&fake, "B")]);
    assert_eq!(fake.requests() - before, 4, "no camera in 64 KiB: 1 MiB too");
    assert_eq!(photo(&s, &remote(&fake, "A/x.dng")).meta.camera, "Sony ILCE-7RM6");
}

/// R16: a subfolder that can't be listed is counted, and the rest is imported.
#[test]
fn failed_subfolders_are_counted() {
    let (mut s, fake, _d) = signed_in("import-subfolder");
    fake.add_file("/Photos/a.jpg", jpeg(32, 16, None));
    fake.add_file("/Photos/locked/b.jpg", jpeg(32, 16, None));
    // Memories' "not installed" and the folder asked for answer; the subfolder is forbidden
    // (`IgnoreRange` changes nothing for these requests)
    fake.fail_next(Fault::IgnoreRange);
    fake.fail_next(Fault::IgnoreRange);
    fake.fail_next(Fault::Status(403));
    let r = import(&mut s, &[remote(&fake, "Photos")]);
    assert_eq!(r, json!({"added": 1, "skipped": 0, "metadata": 1, "failedFolders": 1}));
    let task = s.remote.imports().last().unwrap().task;
    assert_eq!(outcome(&mut s, task), json!({"task": task, "state": "done", "added": 1, "failedFolders": 1}));
}

/// R16: when the server stops answering in the middle of the walk, the import fails and says why,
/// instead of counting every folder left as failed.
#[test]
fn losing_the_server_mid_walk_fails_the_import() {
    let (mut s, fake, _d) = signed_in("import-lost");
    s.remote.set_stall_limit(Duration::from_secs(1));
    fake.add_file("/Photos/a.jpg", jpeg(32, 16, None));
    for i in 0..5 {
        fake.add_file(&format!("/Photos/sub/{i}/b.jpg"), jpeg(32, 16, None));
    }
    fake.fail_next(Fault::IgnoreRange);
    fake.fail_next(Fault::IgnoreRange);
    fake.fail_next(Fault::Stall);
    let e = s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")], "wait": true})).unwrap_err().to_string();
    let last = s.remote.imports().pop().unwrap();
    assert_eq!((last.state, last.failed_folders), (ImportProgress::Failed, 0));
    assert!(last.error.as_deref().is_some_and(|m| e.contains(m)), "{e} / {last:?}");
    assert_eq!(s.catalog.len(), 1, "what was found before stays");
}

#[test]
fn cancelled_imports_say_so() {
    let (mut s, fake, _d) = signed_in("import-cancel");
    fake.add_file("/Photos/a.jpg", jpeg(32, 16, None));
    let (step, steps) = mpsc::channel();
    *s.remote.import_steps.lock().unwrap() = Some(steps);
    let task = s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")]})).unwrap()["task"].as_u64().unwrap();
    s.activity.cancel(task).unwrap();
    step.send(()).unwrap();
    wait_until("the worker's end", || state(&s, task) != ImportProgress::Running);
    assert_eq!(outcome(&mut s, task)["state"], "cancelled");
    assert_eq!(s.catalog.len(), 0);
}

/// R18: a password revoked while metadata is read stops the reads (the client the import holds
/// would go on asking).
#[test]
fn revoked_password_stops_the_metadata_phase() {
    let (mut s, fake, _d) = signed_in("import-revoked");
    fake.enable_memories();
    for i in 0..200 {
        let path = format!("/Photos/{i}.ARW");
        fake.add_file(&path, raw_blob(100));
        fake.set_photo(&path, 1, 6000, 4000, 1_700_000_000, json!({"Make": "Sony"}));
    }
    let (step, steps) = mpsc::channel();
    *s.remote.import_steps.lock().unwrap() = Some(steps);
    let task = s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")]})).unwrap()["task"].as_u64().unwrap();
    step.send(()).unwrap();
    wait_until("the metadata row", || rows(&s).iter().any(|(kind, _)| *kind == "scan"));
    let before = fake.requests();
    fake.fail_next(Fault::Status(401));
    step.send(()).unwrap();
    wait_until("the worker's end", || state(&s, task) != ImportProgress::Running);
    let asked = fake.requests() - before;
    assert!(asked < 60, "{asked} requests after the password was refused");
    let o = outcome(&mut s, task);
    assert_eq!(o["state"], "failed");
    assert!(o["error"].as_str().unwrap().contains("Sign in to Nextcloud"), "{o}");
    s.drain_remote();
    assert_eq!(s.catalog.len(), 200, "the photos found are kept");
}

/// R19: an import belongs to the library it started in.
#[test]
fn opening_another_library_drops_its_imports() {
    let (mut s, fake, d) = signed_in("import-library");
    fake.add_file("/Photos/a.jpg", jpeg(32, 16, None));
    // one import is done, its photos still queued; another is under way
    let done = s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")]})).unwrap()["task"].as_u64().unwrap();
    wait_until("the first import's end", || state(&s, done) != ImportProgress::Running);
    let (step, steps) = mpsc::channel();
    *s.remote.import_steps.lock().unwrap() = Some(steps);
    // (started directly: `execute` would apply the first import's photos first)
    let dirs = vec![craft_nextcloud::RemotePath::parse(&remote(&fake, "Photos")).unwrap()];
    let running = crate::remote::import::start(&mut s, fake.account().key(), dirs, false).unwrap()["task"].as_u64().unwrap();
    assert_eq!(s.catalog.len(), 0, "the first import's photos are still queued");
    s.open_library(d.join("other.lclib"), false).unwrap();
    step.send(()).unwrap();
    wait_until("the second import's end", || state(&s, running) != ImportProgress::Running);
    assert_eq!(state(&s, running), ImportProgress::Cancelled);
    s.drain_remote();
    assert_eq!(s.catalog.len(), 0, "nothing lands in the library opened since");
}

/// R17: the photos of one import are one undo step, however many drains they come in, unless
/// something else was done in between.
#[test]
fn an_import_is_one_undo_step() {
    let mut s = Session::new();
    let task = 77;
    s.remote.begin_import(task, Arc::new(AtomicBool::new(false)));
    let generation = s.library_generation();
    let send = |s: &Session, name: &str| {
        let info = ProbeInfo { format: "JPEG".into(), file_size: 10, ..ProbeInfo::default() };
        let photos = vec![(format!("nextcloud://alice@cloud.example/P/{name}"), info)];
        s.remote.push_event(Event::Found { task, generation, photos });
    };
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        send(&s, name);
        s.drain_remote();
    }
    assert_eq!((s.catalog.len(), s.undo.len()), (3, 1));
    assert_eq!(s.undo[0].label, "Add 3 Photos");
    s.undo_step().unwrap();
    assert_eq!(s.catalog.len(), 0, "one undo takes all three back");
    s.redo_step().unwrap();
    assert_eq!(s.catalog.len(), 3);
    // something else in between: the next photos are a step of their own
    let id = s.catalog.photos().next().unwrap().id;
    s.commit("Rating", Op::SetRating { id, rating: 3 }).unwrap();
    send(&s, "d.jpg");
    s.drain_remote();
    assert_eq!(s.undo.len(), 3);
    assert_eq!(s.remote.imports()[0].added, 4);
    // another library's photos never land
    s.remote.push_event(Event::Found {
        task,
        generation: generation + 1,
        photos: vec![("nextcloud://alice@cloud.example/P/e.jpg".into(), ProbeInfo::default())],
    });
    s.drain_remote();
    assert_eq!(s.catalog.len(), 4);
}

/// R19: a command another command runs doesn't apply results in the middle of it.
#[test]
fn nested_commands_dont_drain() {
    let mut s = Session::new();
    let generation = s.library_generation();
    let info = ProbeInfo { format: "JPEG".into(), ..ProbeInfo::default() };
    s.remote.push_event(Event::Found { task: 1, generation, photos: vec![("nextcloud://alice@cloud.example/a.jpg".into(), info)] });
    s.depth = 1;
    s.execute("activity.list", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), 0);
    s.depth = 0;
    s.execute("activity.list", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), 1);
}

/// Sizes and exposure values Memories can't mean are unknown, not facts.
#[test]
fn odd_memories_values_are_unknown() {
    let (mut s, fake, _d) = signed_in("import-odd");
    fake.enable_memories();
    fake.add_file("/Photos/a.ARW", raw_blob(100));
    fake.set_photo("/Photos/a.ARW", 1, 0, 4000, 1_700_000_000, json!({"Make": "Sony", "ExposureTime": 0, "FNumber": -2.8, "FocalLength": "0 mm"}));
    import(&mut s, &[remote(&fake, "Photos")]);
    let p = photo(&s, &remote(&fake, "Photos/a.ARW"));
    assert_eq!((p.width, p.height), (0, 0));
    assert_eq!((p.meta.camera.as_str(), p.meta.shutter.as_str(), p.meta.aperture, p.meta.focal_mm), ("Sony", "", None, None));
}
