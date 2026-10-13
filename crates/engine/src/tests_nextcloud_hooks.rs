//! Nextcloud photos through the engine's file hooks (`remote::hooks`, against craft's `FakeServer`):
//! interactive loads download once, background and batch work never download, embedded previews come
//! from the server, probes read the header, and an unreachable server, a revoked app password or a
//! file deleted on the server turn into errors (and offline photos) without waiting on the network.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use craft_nextcloud::testing::{FakeServer, Fault};
use craft_nextcloud::{Account, RemotePath};
use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, Samples, encode_jpeg};
use serde_json::json;

use crate::Session;
use crate::media::QuickSource;
use crate::remote::{AccountState, CredentialStore, MemoryCredentials, NOT_DOWNLOADED, RemoteConfig};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-nextcloud-hooks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session signed in to `account` with `password`, its settings and originals cache in `dir`.
fn session_for(dir: &Path, account: &Account, password: &str) -> Session {
    std::fs::write(dir.join("nextcloud.json"), serde_json::to_vec(&json!({"accounts": [account]})).unwrap()).unwrap();
    let creds = Arc::new(MemoryCredentials::default());
    creds.set(account, password).unwrap();
    let s = Session::new().with_fs();
    s.remote.configure(config(dir, creds));
    s
}

fn config(dir: &Path, creds: Arc<MemoryCredentials>) -> RemoteConfig {
    RemoteConfig { settings_file: Some(dir.join("nextcloud.json")), cache_dir: Some(dir.join("cache")), credentials: Some(creds) }
}

/// A fake server and a session signed in to it.
pub(crate) fn signed_in(name: &str) -> (Session, FakeServer, PathBuf) {
    let fake = FakeServer::start();
    let d = temp(name);
    let s = session_for(&d, &fake.account(), FakeServer::PASSWORD);
    (s, fake, d)
}

/// `nextcloud://alice@127.0.0.1:<port>/<rel>`.
pub(crate) fn remote(fake: &FakeServer, rel: &str) -> String {
    rel.split('/').try_fold(fake.account().root(), |p, seg| p.join(seg)).unwrap().to_string()
}

/// A `w`×`h` JPEG with a camera and capture time in its EXIF (and `orientation`, when given).
pub(crate) fn jpeg(w: u32, h: u32, orientation: Option<u16>) -> Vec<u8> {
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 128, 200]).collect();
    let meta = lightcraft_meta::Metadata {
        make: Some("Canon".into()),
        model: Some("EOS R8".into()),
        capture_time: lightcraft_meta::DateTime::parse_exif("2024:09:10 12:34:56"),
        orientation: orientation.map(lightcraft_meta::Orientation::from_exif),
        ..Default::default()
    };
    let exif = lightcraft_meta::write_exif(&meta);
    encode_jpeg(&EncodeImage::new(w, h, 3, Samples::U8(&px)), 90, ChromaSubsampling::S444, &EncodeMeta { exif: Some(&exif), ..Default::default() })
        .unwrap()
}

/// Bytes standing in for a raw file (never decoded by these tests).
pub(crate) fn raw_blob(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 253) as u8).collect()
}

/// Files in the originals cache (not its index).
pub(crate) fn cached_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for acct in std::fs::read_dir(dir.join("cache")).into_iter().flatten().flatten() {
        if acct.path().is_dir() {
            out.extend(std::fs::read_dir(acct.path()).unwrap().flatten().map(|f| f.path()));
        }
    }
    out
}

/// Add `path` to the catalog as an unedited photo of `kind`.
fn add_photo(s: &mut Session, id: u64, path: &str, kind: MediaKind, size: u64) -> PhotoId {
    let name = path.rsplit('/').next().unwrap();
    let mut p = Photo::new(PhotoId(id), Source::File { path: path.into() }, name, "ARW", 6000, 4000, "2026-10-13");
    p.kind = kind;
    p.file_size = size;
    p.develop = Arc::new(p.camera_defaults());
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    PhotoId(id)
}

/// Poll until `done` holds (fail after 10 s).
pub(crate) fn wait_until(what: &str, done: impl Fn() -> bool) {
    let t = Instant::now();
    while !done() {
        assert!(t.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn fs_paths_are_untouched() {
    let (s, fake, d) = signed_in("fs");
    let file = d.join("local.jpg");
    std::fs::write(&file, jpeg(96, 64, None)).unwrap();
    let path = file.to_string_lossy().to_string();
    let (img, _) = (s.media.file_loader.clone().unwrap())(&path, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64));
    let info = (s.media.file_probe.clone().unwrap())(&path).unwrap();
    assert_eq!((info.width, info.height, info.file_size), (96, 64, std::fs::metadata(&file).unwrap().len()));
    assert!(info.content_hash.is_some(), "local files keep their content hash");
    assert_eq!((s.media.file_bytes.clone().unwrap())(&path).unwrap(), std::fs::read(&file).unwrap());
    assert_eq!(s.media.availability.exists(&path), Some(true));
    assert_eq!(s.media.availability.exists(&d.join("nope.jpg").to_string_lossy()), Some(false));
    assert_eq!(fake.requests(), 0, "no request for a local file");
    assert!(cached_files(&d).is_empty());
}

#[test]
fn background_loads_never_download() {
    let (s, fake, d) = signed_in("background");
    fake.add_file("/Photos/x.ARW", raw_blob(4000));
    fake.add_file("/Photos/y.jpg", jpeg(320, 160, None));
    fake.set_preview("/Photos/y.jpg", jpeg(64, 32, None));
    let load = s.media.file_loader.clone().unwrap();
    let raw = remote(&fake, "Photos/x.ARW");
    assert_eq!(crate::memory::in_background(|| load(&raw, 512)).unwrap_err(), NOT_DOWNLOADED);
    assert_eq!(crate::memory::without_downloads(|| load(&raw, 512)).unwrap_err(), NOT_DOWNLOADED);
    // a non-raw image shows its server preview instead
    let (img, _) = crate::memory::in_background(|| load(&remote(&fake, "Photos/y.jpg"), 512)).unwrap();
    assert_eq!((img.width, img.height), (64, 32));
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
}

/// R13: a background load bigger than a thumbnail (a loupe prefetch, whose source is kept for Develop)
/// gets no server preview in place of the original; work that only looks (the face scan) does.
#[test]
fn background_stand_ins_are_thumbnails_only() {
    let (s, fake, d) = signed_in("stand-ins");
    fake.add_file("/Photos/y.jpg", jpeg(320, 160, None));
    fake.set_preview("/Photos/y.jpg", jpeg(64, 32, None));
    let load = s.media.file_loader.clone().unwrap();
    let y = remote(&fake, "Photos/y.jpg");
    let preview_level = crate::media::SourceLevel::Preview.max_edge();
    assert_eq!(crate::memory::in_background(|| load(&y, preview_level)).unwrap_err(), NOT_DOWNLOADED);
    let thumb_level = crate::media::SourceLevel::Thumb.max_edge();
    let (img, _) = crate::memory::in_background(|| load(&y, thumb_level)).unwrap();
    assert_eq!((img.width, img.height), (64, 32), "a thumbnail from the server's preview");
    let (img, _) = crate::memory::without_downloads(|| load(&y, preview_level)).unwrap();
    assert_eq!((img.width, img.height), (64, 32));
    // R34: the app's UI thread gets thumbnails only (a command's source, e.g. Auto Straighten's, is kept)
    assert_eq!(crate::memory::on_ui_thread(|| load(&y, preview_level)).unwrap_err(), NOT_DOWNLOADED);
    assert!(crate::memory::on_ui_thread(|| load(&y, thumb_level)).is_ok());
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
}

/// Find Similar and Assisted Culling look at many photos: none is downloaded for them (JPEGs are
/// looked at in their server previews, raws not at all).
#[test]
fn looking_at_many_photos_never_downloads() {
    let (mut s, fake, d) = signed_in("similar");
    for (i, name) in ["a.jpg", "b.jpg"].into_iter().enumerate() {
        fake.add_file(&format!("/Photos/{name}"), jpeg(96, 64, None));
        fake.set_preview(&format!("/Photos/{name}"), jpeg(64, 32, None));
        add_photo(&mut s, i as u64 + 1, &remote(&fake, &format!("Photos/{name}")), MediaKind::Image, 1000);
    }
    fake.add_file("/Photos/c.ARW", raw_blob(4000));
    add_photo(&mut s, 3, &remote(&fake, "Photos/c.ARW"), MediaKind::Raw, 4000);
    let r = s.execute("library.findSimilar", &json!({"id": 1, "similarity": 0.0})).unwrap();
    assert_eq!(r["photos"].as_array().unwrap().len(), 2, "{r}");
    let r = s.execute("photo.analyze", &json!({"ids": [1, 2, 3]})).unwrap();
    assert_eq!(r["failed"].as_array().unwrap().len(), 1, "the raw: {r}");
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
}

/// R14: a listing's size is the server's word; a huge one overflows nothing when the copy on disk is probed.
#[test]
fn a_huge_reported_size_is_harmless() {
    let (s, fake, _d) = signed_in("huge");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let y = remote(&fake, "Photos/y.jpg");
    (s.media.file_loader.clone().unwrap())(&y, 512).unwrap();
    let rp = RemotePath::parse(&y).unwrap();
    let etag = s.remote.client(&rp.account_key()).unwrap().stat(&rp).unwrap().etag;
    let listed = craft_nextcloud::Entry {
        name: "y.jpg".into(),
        is_dir: false,
        size: u64::MAX,
        etag,
        fileid: None,
        mtime: None,
        content_type: None,
        has_preview: true,
    };
    s.remote.note_listing(&rp.parent().unwrap(), &[listed]);
    // (something else holds memory at the gate, as the app's other work does)
    let _other = crate::memory::work_gate().acquire_urgent(1);
    let info = (s.media.file_probe.clone().unwrap())(&y).unwrap();
    assert_eq!((info.width, info.height, info.file_size), (96, 64, u64::MAX));
}

#[test]
fn an_interactive_miss_downloads_once() {
    let (mut s, fake, d) = signed_in("interactive");
    let bytes = jpeg(96, 64, None);
    fake.add_file("/Photos/y.jpg", bytes.clone());
    let load = s.media.file_loader.clone().unwrap();
    let y = remote(&fake, "Photos/y.jpg");
    let (img, _) = load(&y, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64));
    assert_eq!(fake.downloads(), 1);
    // the second load is a cache hit: its etag is known, so no request at all
    let before = fake.requests();
    let (img, _) = load(&y, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64));
    assert_eq!((fake.requests() - before, fake.downloads()), (0, 1));
    // a background load now finds it on disk too, and so do the byte reader and the pair loader
    assert!(crate::memory::in_background(|| load(&y, 512)).is_ok());
    assert_eq!((s.media.file_bytes.clone().unwrap())(&y).unwrap(), bytes);
    assert_eq!(cached_files(&d).len(), 1);
    let r = s.execute("nextcloud.preferences", &json!({})).unwrap();
    assert_eq!((r["cachedBytes"].as_u64(), r["pinnedBytes"].as_u64()), (Some(bytes.len() as u64), Some(0)));
    assert!(s.activity.list().is_empty(), "the download row went");
}

/// R26: another process (another window, lightcraft-cli) holding the originals cache keeps this one
/// out of it: downloads say so, nextcloud.preferences reports no sizes, and once it lets go the cache
/// is this process's for good. (A second handle on the lock file stands in for the other process.)
#[test]
fn another_process_using_the_cache_keeps_downloads_out() {
    let (mut s, fake, d) = signed_in("cache-lock");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    fake.set_preview("/Photos/y.jpg", jpeg(64, 32, None));
    let y = remote(&fake, "Photos/y.jpg");
    let load = s.media.file_loader.clone().unwrap();
    let lock_file = || std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(d.join("cache/.lock")).unwrap();
    std::fs::create_dir_all(d.join("cache")).unwrap();
    let other = lock_file();
    other.try_lock().unwrap();
    let e = s.remote.local_original(&y).unwrap_err();
    assert!(e.contains("Another LightCraft window or lightcraft-cli is using the Nextcloud download cache"), "{e}");
    assert_eq!(fake.downloads(), 0);
    let r = s.execute("nextcloud.preferences", &json!({})).unwrap();
    assert_eq!((r["cachedBytes"].as_u64(), r["pinnedBytes"].as_u64()), (Some(0), Some(0)), "{r}");
    // browsing goes on: a thumbnail still comes from the server's preview
    let thumb_level = crate::media::SourceLevel::Thumb.max_edge();
    let (img, _) = crate::memory::in_background(|| load(&y, thumb_level)).unwrap();
    assert_eq!((img.width, img.height), (64, 32));
    // the other process quits: the next use opens the cache and keeps it locked
    drop(other);
    s.remote.local_original(&y).unwrap();
    assert_eq!(fake.downloads(), 1);
    assert!(matches!(lock_file().try_lock(), Err(std::fs::TryLockError::WouldBlock)), "held while this process runs");
}

#[test]
fn downloads_show_and_cancel_in_activity() {
    let (s, fake, d) = signed_in("activity");
    s.remote.set_stall_limit(Duration::from_secs(2));
    fake.add_file("/Photos/x.ARW", raw_blob(2_600_000));
    let raw = remote(&fake, "Photos/x.ARW");
    let load = s.media.file_loader.clone().unwrap();
    // (a background load stats the file: its etag and size are known from then on)
    assert!(crate::memory::in_background(|| load(&raw, 512)).is_err());
    fake.fail_next(Fault::Stall);
    let worker = std::thread::spawn(move || load(&raw, 512));
    wait_until("the download row", || !s.activity.list().is_empty());
    let row = s.activity.list().remove(0);
    assert_eq!((row.kind, row.label.as_str(), row.cancellable), ("download", "Downloading x.ARW (3 MB)", true));
    s.activity.cancel(row.id).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(s.activity.list().is_empty(), "the row went with the download");
    assert!(cached_files(&d).is_empty(), "nothing half-downloaded is kept");
}

#[test]
fn previews_come_from_the_server_upright() {
    let (s, fake, _d) = signed_in("previews");
    fake.add_file("/Photos/x.ARW", raw_blob(4000));
    // the server's preview is upright already: an orientation tag in it is not applied again
    fake.set_preview("/Photos/x.ARW", jpeg(64, 32, Some(6)));
    let preview = s.media.preview_loader.clone().unwrap();
    let img = preview(&remote(&fake, "Photos/x.ARW"), 512).unwrap();
    assert_eq!((img.width, img.height), (64, 32));
    // fitted into the size asked for
    let img = preview(&remote(&fake, "Photos/x.ARW"), 16).unwrap();
    assert_eq!((img.width, img.height), (16, 8));
    // none on the server: none here
    fake.add_file("/Photos/z.ARW", raw_blob(4000));
    assert!(preview(&remote(&fake, "Photos/z.ARW"), 512).is_none());
    assert_eq!(fake.downloads(), 0);
}

#[test]
fn remote_probe_reads_the_header_only() {
    let (s, fake, d) = signed_in("probe");
    let bytes = jpeg(96, 64, Some(6));
    fake.add_file("/Photos/y.jpg", bytes.clone());
    let probe = s.media.file_probe.clone().unwrap();
    let y = remote(&fake, "Photos/y.jpg");
    let info = probe(&y).unwrap();
    assert_eq!((info.width, info.height), (64, 96), "oriented");
    assert_eq!(info.captured.as_deref(), Some("2024-09-10T12:34:56"));
    assert_eq!(info.meta.camera, "Canon EOS R8");
    assert_eq!((info.format.as_str(), info.file_size, info.content_hash.as_ref()), ("JPEG", bytes.len() as u64, None));
    assert_eq!(fake.downloads(), 0, "Range reads only");
    // once downloaded, the local copy is probed (no request), still without a content hash
    (s.media.file_loader.clone().unwrap())(&y, 512).unwrap();
    let before = fake.requests();
    let again = probe(&y).unwrap();
    assert_eq!((again.width, again.height, again.content_hash), (64, 96, None));
    assert_eq!(fake.requests(), before);
    assert_eq!(cached_files(&d).len(), 1);
}

/// A CR3 holding only a `w`×`h` preview (`PRVW`), padded to `len` bytes: a raw this build describes
/// from its embedded preview.
fn cr3_with_preview(w: u32, h: u32, len: usize) -> Vec<u8> {
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 128, 200]).collect();
    let jpeg = encode_jpeg(&EncodeImage::new(w, h, 3, Samples::U8(&px)), 90, ChromaSubsampling::S444, &EncodeMeta::default()).unwrap();
    let mut b = b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom".to_vec();
    b.extend_from_slice(&((24 + jpeg.len()) as u32).to_be_bytes());
    b.extend_from_slice(b"PRVW\0\0\0\0\0\x01");
    b.extend_from_slice(&(w as u16).to_be_bytes());
    b.extend_from_slice(&(h as u16).to_be_bytes());
    b.extend_from_slice(b"\0\x01");
    b.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
    b.extend_from_slice(&jpeg);
    b.resize(len.max(b.len()), 0);
    b
}

/// The start of a big raw describes it from its embedded preview (measured on real ARW and CR3 files:
/// the raw data lies past any useful window), which would mark it preview-only at the preview's size.
/// That description only counts when the whole file was read.
#[test]
fn truncated_raws_are_not_described_from_their_preview() {
    let (s, fake, _d) = signed_in("truncated");
    fake.add_file("/Photos/big.CR3", cr3_with_preview(96, 64, 5 << 20));
    fake.add_file("/Photos/small.CR3", cr3_with_preview(96, 64, 0));
    let probe = s.media.file_probe.clone().unwrap();
    let before = fake.requests();
    let e = probe(&remote(&fake, "Photos/big.CR3")).unwrap_err();
    assert!(e.contains("doesn't describe it"), "{e}");
    assert_eq!(fake.requests() - before, 2, "a stat and the header window: no second read of a big raw");
    let small = probe(&remote(&fake, "Photos/small.CR3")).unwrap();
    assert!(small.preview_only.is_some(), "a whole file is described as it is");
    // a file up to 4 MiB is read whole when its start didn't describe it
    fake.add_file("/Photos/mid.CR3", cr3_with_preview(96, 64, 1 << 20));
    assert!(probe(&remote(&fake, "Photos/mid.CR3")).unwrap().preview_only.is_some());
    // a raw the start does describe, and not from its preview (a small DNG, padded), still doesn't count
    // from part of the file
    let mut dng = crate::tests_xmp::synthetic_dng_with(None, lightcraft_meta::Metadata::default());
    let start = crate::files::probe_bytes("big.dng", &dng);
    assert!(start.is_ok_and(|i| i.kind == MediaKind::Raw && i.preview_only.is_none()), "the start describes it");
    dng.resize(5 << 20, 0);
    fake.add_file("/Photos/big.dng", dng);
    let e = probe(&remote(&fake, "Photos/big.dng")).unwrap_err();
    assert!(e.contains("doesn't describe it"), "{e}");
    // a format no download would open keeps its own error
    fake.add_file("/Photos/big.xyz", vec![7; 5 << 20]);
    assert_eq!(probe(&remote(&fake, "Photos/big.xyz")).unwrap_err(), "unrecognized file format");
    assert_eq!(fake.downloads(), 0);
}

#[test]
fn remote_thumbnails_land_in_the_disk_cache() {
    let (mut s, fake, _d) = signed_in("thumbs");
    fake.add_file("/Photos/x.ARW", raw_blob(4000));
    fake.set_preview("/Photos/x.ARW", jpeg(64, 32, None));
    let id = add_photo(&mut s, 1, &remote(&fake, "Photos/x.ARW"), MediaKind::Raw, 4000);
    let job = s.thumb_job(id, 128).unwrap();
    let q = s.quick_thumb_job(&job).unwrap().run();
    assert_eq!((q.quick, q.key), (Some(QuickSource::Embedded), job.key));
    let before = fake.requests();
    assert_eq!(s.quick_thumb_job(&job).unwrap().run().quick, Some(QuickSource::Cached));
    assert_eq!(fake.requests(), before, "the second time comes from the cache");
}

#[test]
fn stale_copy_is_used_when_the_server_is_down() {
    let (s, fake, d) = signed_in("stale");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let y = remote(&fake, "Photos/y.jpg");
    let load = s.media.file_loader.clone().unwrap();
    load(&y, 512).unwrap();
    // a new session on this computer: nothing known about the file but what is on disk
    let creds = Arc::new(MemoryCredentials::default());
    creds.set(&fake.account(), FakeServer::PASSWORD).unwrap();
    s.remote.configure(config(&d, creds));
    drop(fake);
    let (img, _) = load(&y, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64), "the copy on disk, though it may be out of date");
    // (and the server isn't asked again for 30 s: background work uses the copy at once too)
    assert!(crate::memory::in_background(|| load(&y, 512)).is_ok());
}

#[test]
fn deleted_on_server_shows_missing() {
    let (s, fake, _d) = signed_in("deleted");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let y = remote(&fake, "Photos/y.jpg");
    assert!(s.remote.exists(&y));
    let before = fake.requests();
    assert!(s.remote.exists(&y));
    assert_eq!(fake.requests(), before, "the server answered a moment ago: no new request");
    fake.remove("/Photos/y.jpg");
    let e = (s.media.file_loader.clone().unwrap())(&y, 512).unwrap_err();
    assert_eq!(e, "The file or folder is no longer on the Nextcloud server");
    assert!(!s.remote.exists(&y));
    assert_eq!(s.media.availability.exists(&y), Some(false), "the photo shows as missing");
    // a listing that has it again brings it back
    s.remote.note_listing(
        &RemotePath::parse(&remote(&fake, "Photos")).unwrap(),
        &[craft_nextcloud::Entry {
            name: "y.jpg".into(),
            is_dir: false,
            size: 1,
            etag: "e".into(),
            fileid: None,
            mtime: None,
            content_type: None,
            has_preview: true,
        }],
    );
    assert!(s.remote.exists(&y));
    // and one without it (or without its folder's other files) says it isn't there
    assert!(!s.remote.exists(&remote(&fake, "Photos/other.jpg")));
}

/// R2 / Review Focus 4: a path of an account this computer doesn't have is offline.
#[test]
fn unknown_accounts_photos_are_offline() {
    let (s, _fake, _d) = signed_in("unknown-account");
    let path = "nextcloud://bob@nowhere.example/Photos/x.ARW";
    assert!(!s.remote.exists(path));
    assert_eq!(s.media.availability.exists(path), Some(false));
    let e = (s.media.file_loader.clone().unwrap())(path, 512).unwrap_err();
    assert!(e.contains("No Nextcloud account bob@nowhere.example"), "{e}");
    assert!(!s.remote.exists("nextcloud://not a path"));
}

/// Review Focus 1: a server that stops answering costs one wait, not one per request.
#[test]
fn unreachable_server_fails_fast() {
    // accepts connections, counts them and never answers
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let account = Account { server: format!("http://{}", silent.local_addr().unwrap()), user: "alice".into(), trust: None };
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = connections.clone();
    std::thread::spawn(move || {
        let mut open = Vec::new();
        for stream in silent.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            open.push(stream);
        }
    });
    let d = temp("unreachable");
    let s = session_for(&d, &account, "pw");
    s.remote.set_stall_limit(Duration::from_secs(1));
    let load = s.media.file_loader.clone().unwrap();
    let paths: Vec<String> = (0..50).map(|i| account.root().join("Photos").unwrap().join(&format!("{i}.ARW")).unwrap().to_string()).collect();
    let t = Instant::now();
    for p in &paths {
        let e = crate::memory::in_background(|| load(p, 512)).unwrap_err();
        assert!(e.contains("Can't reach the Nextcloud server"), "{e}");
    }
    for p in &paths {
        assert!(!s.remote.exists(p));
    }
    // one request waited out the stall limit; the other 99 didn't try (100 s if each had)
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
    assert_eq!(s.remote.accounts()[0].1, AccountState::Unreachable);
}

/// Review Focus 3: a revoked app password signs the account out; nothing is asked again until it signs in.
#[test]
fn revoked_password_marks_account_signed_out() {
    let fake = FakeServer::start();
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let d = temp("revoked");
    let s = session_for(&d, &fake.account(), "revoked-password");
    let load = s.media.file_loader.clone().unwrap();
    let y = remote(&fake, "Photos/y.jpg");
    let e = load(&y, 512).unwrap_err();
    assert!(e.contains("sign in again"), "{e}");
    assert_eq!(s.remote.accounts()[0].1, AccountState::SignedOut);
    let before = fake.requests();
    let e = load(&y, 512).unwrap_err();
    assert!(e.contains("again"), "{e}");
    assert!(!s.remote.exists(&y));
    assert!((s.media.file_probe.clone().unwrap())(&y).is_err());
    assert_eq!(fake.requests(), before, "no request while signed out");
}

/// Review Focus 5: Build Previews and smart previews never download an original.
#[test]
fn batch_builders_never_download() {
    let (mut s, fake, d) = signed_in("batch");
    let mut ids = Vec::new();
    for i in 0..3u64 {
        fake.add_file(&format!("/Photos/{i}.ARW"), raw_blob(4000));
        ids.push(add_photo(&mut s, i + 1, &remote(&fake, &format!("Photos/{i}.ARW")), MediaKind::Raw, 4000).0);
    }
    // a JPEG: its server preview makes a thumbnail, but never a preview-size source
    fake.add_file("/Photos/y.jpg", jpeg(320, 160, None));
    fake.set_preview("/Photos/y.jpg", jpeg(64, 32, None));
    ids.push(add_photo(&mut s, 4, &remote(&fake, "Photos/y.jpg"), MediaKind::Image, 1000).0);
    let r = s.execute("library.buildPreviews", &json!({"ids": ids, "wait": true})).unwrap();
    assert_eq!((r["failed"].as_u64(), r["skipped"].as_u64()), (Some(0), Some(4)), "left out, not failed: {r}");
    std::fs::create_dir_all(d.join("smart")).unwrap();
    s.media.smart_dir = Some(d.join("smart"));
    let r = s.execute("library.smartPreviews", &json!({"ids": ids})).unwrap();
    assert_eq!(r["built"], 0, "{r}");
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
}

/// A never-downloaded raw edited in the Library (Paste Settings, a preset) gets no thumbnail: an edit is
/// rendered from the original, which a thumbnail never downloads, and the server's preview can't show
/// the edit (docs/nextcloud.md, Limits).
#[test]
fn an_edited_raw_not_downloaded_has_no_thumbnail() {
    let (mut s, fake, _d) = signed_in("edited-thumb");
    fake.add_file("/Photos/x.ARW", raw_blob(4000));
    fake.set_preview("/Photos/x.ARW", jpeg(64, 32, None));
    let id = add_photo(&mut s, 1, &remote(&fake, "Photos/x.ARW"), MediaKind::Raw, 4000);
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    let job = s.thumb_job(id, 256).unwrap();
    assert!(s.quick_thumb_job(&job).is_none(), "an edit isn't the server's preview");
    assert_eq!(crate::memory::in_background(|| job.run()).rendered.err().as_deref(), Some(NOT_DOWNLOADED));
    assert_eq!(fake.downloads(), 0);
}

/// R5: a hostile settings file can't make the budget 0 or overflow it: u64::MAX MB saturates to
/// u64::MAX bytes, and downloading (and evicting) under it, then under the clamped 0, keeps the copy.
#[test]
fn hostile_budgets_are_clamped() {
    let fake = FakeServer::start();
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let y = remote(&fake, "Photos/y.jpg");
    let d = temp("budget");
    let creds = Arc::new(MemoryCredentials::default());
    creds.set(&fake.account(), FakeServer::PASSWORD).unwrap();
    for (written, kept) in [(u64::MAX, u64::MAX), (0, 1000)] {
        let settings = json!({"accounts": [fake.account()], "cacheBudgetMb": written});
        std::fs::write(d.join("nextcloud.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        let s = Session::new().with_fs();
        s.remote.configure(config(&d, creds.clone()));
        assert_eq!(s.remote.cache_budget_mb(), kept);
        (s.media.file_loader.clone().unwrap())(&y, 512).unwrap();
        assert_eq!(cached_files(&d).len(), 1, "budget {written} MB");
    }
    assert_eq!(crate::remote::budget_bytes(u64::MAX), u64::MAX);
    assert_eq!(crate::remote::budget_bytes(0), 1_000_000_000);
}

/// R36: a loupe's quick view never downloads. A photo the server has no preview of falls back to a
/// thumbnail that isn't there either; the view's own render (cancellable) downloads the original.
#[test]
fn quick_views_never_download() {
    let (mut s, fake, d) = signed_in("quick");
    fake.add_file("/Photos/x.ARW", raw_blob(4000));
    let id = add_photo(&mut s, 1, &remote(&fake, "Photos/x.ARW"), MediaKind::Raw, 4000);
    let q = s.quick_view_job(id, 1600, true).unwrap().run();
    assert!(q.rendered.is_err());
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
}

/// R34: All Metadata of a photo whose original isn't here reads the start of the file (its tags),
/// not the whole file; once the original is here, it reads that.
#[test]
fn all_metadata_reads_the_header_of_a_photo_not_here() {
    let (mut s, fake, d) = signed_in("all-metadata");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let y = remote(&fake, "Photos/y.jpg");
    let id = add_photo(&mut s, 1, &y, MediaKind::Image, 1000);
    let tags = |s: &mut Session| s.execute("photo.allMetadata", &json!({"id": id.0})).unwrap()["exif"].to_string();
    assert!(tags(&mut s).contains("Canon"), "{}", tags(&mut s));
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
    (s.media.file_loader.clone().unwrap())(&y, 512).unwrap();
    let before = fake.requests();
    assert!(tags(&mut s).contains("Canon"));
    assert_eq!(fake.requests(), before, "the copy on this computer");
}

/// R30/R33: while the server can't be reached nothing waits for it, after a restart too: a photo
/// downloaded before opens from the cache's newest copy (no etag known, no request) and counts as
/// there (edits render from the original, not the smart preview); one never downloaded fails at once.
#[test]
fn offline_copies_open_at_once_after_a_restart() {
    let (s, fake, d) = signed_in("offline-restart");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    fake.add_file("/Photos/z.jpg", jpeg(96, 64, None));
    let (y, z) = (remote(&fake, "Photos/y.jpg"), remote(&fake, "Photos/z.jpg"));
    (s.media.file_loader.clone().unwrap())(&y, 512).unwrap();
    drop(s);
    // a restart, and the server stops answering
    let offline = || {
        let s = session_for(&d, &fake.account(), FakeServer::PASSWORD);
        s.remote.note_error(&fake.account().key(), &craft_nextcloud::Error::Unreachable("timed out".into()));
        s
    };
    let s = offline();
    let (load, before, t) = (s.media.file_loader.clone().unwrap(), fake.requests(), Instant::now());
    let (img, _) = load(&y, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64), "the copy on disk");
    assert!(s.remote.exists(&y), "a photo with its original here is there");
    let e = load(&z, 512).unwrap_err();
    assert!(e.contains("Can't reach the Nextcloud server"), "{e}");
    assert!(!s.remote.exists(&z));
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    assert_eq!(fake.requests(), before, "nothing asked the server");
}

/// While the server can't be reached, a photo whose newer version a listing showed this session, and
/// whose older copy is all that is here, opens from that copy (not the smart preview), with no request.
#[test]
fn an_unreachable_server_opens_an_older_copy() {
    let (s, fake, _d) = signed_in("offline-older");
    fake.add_file("/Photos/y.jpg", jpeg(96, 64, None));
    let y = remote(&fake, "Photos/y.jpg");
    let load = s.media.file_loader.clone().unwrap();
    load(&y, 512).unwrap();
    // a newer version on the server, its etag seen by a listing; then the server stops answering
    fake.add_file("/Photos/y.jpg", jpeg(48, 32, None));
    s.remote.list(&RemotePath::parse(&remote(&fake, "Photos")).unwrap()).unwrap();
    s.remote.note_error(&fake.account().key(), &craft_nextcloud::Error::Unreachable("timed out".into()));
    let before = fake.requests();
    let (img, _) = load(&y, 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64), "the older copy");
    assert!(s.remote.exists(&y), "a photo with an original here is there");
    assert_eq!(fake.requests(), before, "nothing asked the server");
}

/// R32: a raw the server has no preview of (no preview provider for its format: all CR3s on a live
/// server) shows the embedded preview in its first MiB, from a Range read, in the grid and the quick
/// view alike. A preview past that window isn't read: none, as before.
#[test]
fn raws_without_a_server_preview_show_their_own() {
    let (s, fake, d) = signed_in("raw-previews");
    fake.add_file("/Photos/x.CR3", cr3_with_preview(96, 64, 5 << 20));
    let small = cr3_with_preview(96, 64, 0);
    let mut far = small[..24].to_vec();
    far.resize(24 + (1 << 20), 0);
    far.extend_from_slice(&small[24..]);
    fake.add_file("/Photos/far.CR3", far);
    let preview = s.media.preview_loader.clone().unwrap();
    let img = preview(&remote(&fake, "Photos/x.CR3"), 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64));
    assert!(preview(&remote(&fake, "Photos/far.CR3"), 512).is_none());
    assert_eq!(fake.downloads(), 0);
    assert!(cached_files(&d).is_empty());
}

/// R38: a TIFF-based raw (an ARW, NEF…) the server has no preview of: its JPEG preview shows when the
/// first MiB holds all of it, and not at all when the window cuts it off (no half-grey thumbnail).
#[test]
fn a_preview_the_window_cuts_off_isnt_shown() {
    let (s, fake, _d) = signed_in("cut-preview");
    let preview = jpeg(96, 64, None);
    // a little-endian TIFF whose one IFD points at the JPEG (JPEGInterchangeFormat and its length)
    let tiff_at = |at: usize| {
        let mut b = b"II*\0\x08\0\0\0\x02\0".to_vec();
        for (tag, value) in [(0x0201u16, at as u32), (0x0202, preview.len() as u32)] {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&4u16.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&value.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b.resize(at, 0);
        b.extend_from_slice(&preview);
        b.resize(at + preview.len() + (1 << 20), 0);
        b
    };
    fake.add_file("/Photos/whole.ARW", tiff_at(4096));
    fake.add_file("/Photos/cut.ARW", tiff_at((1 << 20) - preview.len() / 2));
    let load = s.media.preview_loader.clone().unwrap();
    let img = load(&remote(&fake, "Photos/whole.ARW"), 512).unwrap();
    assert_eq!((img.width, img.height), (96, 64));
    assert!(load(&remote(&fake, "Photos/cut.ARW"), 512).is_none(), "half a JPEG isn't a preview");
    assert_eq!(fake.downloads(), 0);
}
