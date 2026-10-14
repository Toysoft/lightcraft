//! Nextcloud photos (`nextcloud://user@host/dir/x.ARW`) live on a server: every action that
//! reads or writes beside the original through the file system refuses them with a clear
//! message, and existence checks ask the availability probe instead of `Path::exists`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;

const DIR: &str = "nextcloud://alice@127.0.0.1/Photos";

fn remote(name: &str) -> String {
    format!("{DIR}/{name}")
}

/// A session holding remote photos 1..=n (`a.ARW`, `b.ARW`…), and a probe that counts its calls.
fn scene(n: u64) -> (Session, Arc<AtomicUsize>) {
    let mut s = Session::new();
    let ops = (1..=n)
        .map(|i| {
            let name = format!("{}.ARW", (b'a' + (i - 1) as u8) as char);
            let p = Photo::new(PhotoId(i), Source::File { path: remote(&name) }, &name, "ARW", 60, 40, "2026-01-01T00:00:00");
            Op::AddPhoto { photo: Box::new(p) }
        })
        .collect();
    s.commit("Add", Op::Batch { ops }).unwrap();
    if n > 0 {
        s.selection = crate::Selection { active: Some(PhotoId(1)), ids: (1..=n).map(PhotoId).collect() };
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    s.media.availability.set_probe(Arc::new(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        true
    }));
    (s, calls)
}

fn refused(r: crate::Result<serde_json::Value>, action: &str) {
    let e = r.expect_err(action).to_string();
    assert!(e.contains(&format!("{action} isn't available for Nextcloud photos")), "{e}");
}

#[test]
fn sidecars_skip_remote_photos() {
    let (mut s, _) = scene(1);
    assert!(crate::sidecar::find_sidecar(&remote("a.ARW"), crate::sidecar::SidecarNaming::Stem).is_none());
    assert!(crate::sidecar::read_packet(&remote("a.ARW"), lightcraft_catalog::MediaKind::Raw, crate::sidecar::SidecarNaming::Stem).is_none());
    s.xmp.auto_write = true;
    s.execute("photo.rate", &json!({"rating": 3, "ids": [1]})).unwrap();
    assert!(!std::path::Path::new("nextcloud:").exists(), "nothing was created relative to the working directory");
    let r = s.execute("photo.saveMetadataToFile", &json!({"ids": [1]})).unwrap();
    let err = r["failed"][0]["error"].as_str().unwrap_or_default();
    assert!(err.contains("Saving XMP isn't available for Nextcloud photos"), "{r}");
    assert!(s.save_sidecar(PhotoId(1)).is_err());
}

#[test]
fn library_import_rejects_remote_paths() {
    let (mut s, _) = scene(0);
    let e = s.execute("library.import", &json!({"paths": [remote("a.ARW")]})).unwrap_err().to_string();
    assert!(e.contains("use library.importNextcloud for Nextcloud folders"), "{e}");
}

#[test]
fn missing_uses_the_availability_probe() {
    let (mut s, calls) = scene(2);
    assert_eq!(s.execute("library.missing", &json!({})).unwrap(), json!([]));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    s.media.availability.set_probe(Arc::new(|p| !p.ends_with("b.ARW")));
    let r = s.execute("library.missing", &json!({})).unwrap();
    assert_eq!(r, json!([{"id": 2, "path": remote("b.ARW")}]));
    refused(s.execute("photo.relink", &json!({"id": 2, "path": remote("c.ARW")})), "Relinking");
}

#[test]
fn find_missing_skips_remote_photos() {
    use crate::cmd::missing::{find_candidates, plan_find_missing};
    let (s, calls) = scene(2);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sn = seen.clone();
    let probe: crate::availability::ExistsProbe = Arc::new(move |p| {
        sn.lock().unwrap().push(p.to_string());
        false
    });
    let dir = std::env::temp_dir().join(format!("lc-remote-find-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plan = plan_find_missing(&find_candidates(&s.catalog), &dir.to_string_lossy(), &probe).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((plan.found.len(), plan.missing), (0, 0), "{plan:?}");
    assert!(seen.lock().unwrap().is_empty(), "remote candidates are not looked for");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(plan_find_missing(&[], DIR, &probe).unwrap_err().contains("Nextcloud"));
}

#[test]
fn folder_and_file_operations_refuse_remote_paths() {
    let (mut s, _) = scene(2);
    refused(s.execute("library.browse", &json!({"path": DIR})), "Browsing");
    refused(s.execute("folder.rename", &json!({"path": DIR, "name": "Other"})), "Renaming folders");
    refused(s.execute("folder.move", &json!({"path": DIR, "into": std::env::temp_dir().to_string_lossy()})), "Renaming folders");
    refused(s.execute("folder.create", &json!({"parent": DIR, "name": "New"})), "Creating folders");
    refused(s.execute("photo.moveToFolder", &json!({"folder": std::env::temp_dir().to_string_lossy(), "ids": [1]})), "Moving files");
    refused(s.execute("photo.moveToFolder", &json!({"folder": DIR, "ids": [1]})), "Moving files");
    refused(s.execute("photo.rename", &json!({"template": "{name}-x", "ids": [1]})), "Renaming files");
    refused(s.execute("photo.renamePreview", &json!({"template": "{name}-x", "ids": [1]})), "Renaming files");
    let plans = vec![crate::rename::RenamePlan { id: 1, from: "a.ARW".into(), to: "z.ARW".into(), from_path: Some(remote("a.ARW")), to_path: None }];
    assert!(s.apply_rename(&plans).unwrap_err().to_string().contains("Renaming files isn't available"));
}

#[test]
fn synchronize_refuses_remote_folders() {
    let (mut s, _) = scene(1);
    let e = crate::sync::SyncInput::new(&mut s, DIR, false).err().expect("refused").to_string();
    assert!(e.contains("Synchronize Folder isn't available for Nextcloud photos"), "{e}");
    refused(s.execute("folder.synchronize", &json!({"path": DIR})), "Synchronize Folder");
}

#[test]
fn writes_beside_the_original_refuse_remote_photos() {
    let (mut s, _) = scene(2);
    refused(s.execute("photo.convertToDng", &json!({"ids": [1]})), "Convert to DNG");
    refused(s.execute("photo.duplicate", &json!({"ids": [1]})), "Duplicate");
    refused(s.execute("photo.editExternal", &json!({"ids": [1]})), "Edit In");
}

#[test]
fn ai_masks_do_not_call_a_remote_photo_missing() {
    // (segment_job needs the `sam` feature and a model; its check is the same probe call)
    let (mut s, calls) = scene(1);
    let r = s.execute("photo.smartPreview", &json!({"id": 1})).unwrap();
    assert_eq!(r["originalOnline"], true, "{r}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    s.media.availability.set_probe(Arc::new(|_| false));
    assert_eq!(s.execute("photo.smartPreview", &json!({"id": 1})).unwrap()["originalOnline"], false);
}
