//! Browsing and importing Nextcloud folders (`nextcloud.browse`, `library.importNextcloud`, against
//! craft's `FakeServer`): listings, the Memories join, metadata from Memories or the file's first
//! bytes, rescans, activity rows, awkward names, and never downloading an original.

use std::sync::mpsc;

use lightcraft_catalog::{MediaKind, Photo, Source};
use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, Samples, encode_jpeg};
use serde_json::{Value, json};

use crate::Session;
use crate::media::SourceLevel;
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
    assert_eq!(r, json!({"added": 3, "skipped": 0, "metadata": 3}));
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
    // a raw past the 1 MiB window: only its metadata counts, its size stays unknown
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
    let r = import(&mut s, &[remote(&fake, "Photos")]);
    assert_eq!(r, json!({"added": 2, "skipped": 0, "metadata": 2}));
    let x = photo(&s, &remote(&fake, "Photos/x.jpg"));
    assert_eq!((x.width, x.height, x.kind, x.format.as_str()), (64, 96, MediaKind::Image, "JPEG"));
    assert_eq!(x.captured.as_deref(), Some("2024-09-10T12:34:56"));
    assert_eq!((x.meta.camera.as_str(), x.content_hash.as_deref()), ("Canon EOS R8", None));
    let big = photo(&s, &remote(&fake, "Photos/big.dng"));
    assert_eq!((big.width, big.height, big.kind, big.format.as_str(), big.file_size), (0, 0, MediaKind::Raw, "DNG", 2 << 20));
    assert_eq!((big.meta.camera.as_str(), big.meta.lens.as_str(), big.meta.shutter.as_str()), ("Sony ILCE-7RM6", "FE 50mm F1.2 GM", "1/250"));
    assert_eq!((big.meta.aperture, big.meta.iso, big.captured.as_deref()), (Some(1.2), Some(800), Some("2025-01-02T03:04:05")));
    assert!(big.preview_only.is_none(), "nothing from part of a raw beyond its metadata");
    assert_eq!(fake.downloads(), 0);
}

#[test]
fn rescan_adds_only_new_files_and_finishes_metadata() {
    let (mut s, fake, _d) = signed_in("import-rescan");
    fake.add_file("/Photos/a.jpg", jpeg(96, 64, None));
    fake.add_file("/Photos/b.jpg", plain_jpeg(40, 30));
    let dirs = [remote(&fake, "Photos")];
    assert_eq!(import(&mut s, &dirs), json!({"added": 2, "skipped": 0, "metadata": 2}));
    assert_eq!(photo(&s, &remote(&fake, "Photos/b.jpg")).meta.camera, "", "b has no camera yet");
    // b gets its EXIF on the server, c is new
    fake.add_file("/Photos/b.jpg", jpeg(40, 30, None));
    fake.add_file("/Photos/c.jpg", jpeg(20, 10, None));
    let before = fake.requests();
    assert_eq!(import(&mut s, &dirs), json!({"added": 1, "skipped": 2, "metadata": 2}));
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
    assert_eq!(r, json!({"started": true}));
    let importing = ("import", "Importing from Nextcloud".to_string());
    let scanning = ("scan", "Reading metadata".to_string());
    wait_until("the import row", || rows(&s) == [importing.clone()]);
    step.send(()).unwrap();
    wait_until("the metadata row", || rows(&s) == [scanning.clone()]);
    let task = s.activity.list().into_iter().next().unwrap();
    assert!(task.cancellable);
    assert_eq!((task.done, task.total), (0, 3));
    step.send(()).unwrap();
    wait_until("the worker's end", || s.activity.list().is_empty());
    assert_eq!(s.catalog.len(), 0, "nothing joins the catalog off the session");
    s.drain_remote();
    assert_eq!(s.catalog.len(), 3);
    assert!(s.catalog.photos().all(|p| p.meta.camera == "Canon EOS R8"));
    s.drain_remote();
    assert_eq!(s.catalog.len(), 3, "a drain with nothing queued changes nothing");
}

#[test]
fn every_command_drains_first() {
    let (mut s, fake, _d) = signed_in("import-drain");
    fake.add_file("/Photos/a.jpg", jpeg(32, 16, None));
    s.execute("library.importNextcloud", &json!({"dirs": [remote(&fake, "Photos")]})).unwrap();
    wait_until("the worker's end", || s.activity.list().is_empty());
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
