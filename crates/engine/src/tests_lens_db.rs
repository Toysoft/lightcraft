//! Settings ▸ Lens profiles: the folder setting (saved, applied at once), downloading lensfun's database from a local
//! server (good, 404, truncated, not an archive), removing it, and what Develop ▸ Optics is told about a photo.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::PoisonError;

use serde_json::{Value, json};

use crate::Session;
use crate::lens_download::tests::{DB, archive, gzip};
use crate::lens_profiles::TEST_LOCK;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lens-db-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A local server answering every request with `status` and `body`.
fn serve(status: &'static str, body: Vec<u8>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/lensfun.tar.gz", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let _ = s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
            let _ = s.write_all(&body);
        }
    });
    url
}

/// A session whose settings live in `dir`, downloading from `url`.
fn session(dir: &std::path::Path, url: &str) -> Session {
    let mut s = Session::new();
    s.lens_db = crate::lens_download::LensSettings::load(Some(dir.to_path_buf()));
    s.lens_db.url = url.to_string();
    s
}

fn skip_for_env() -> bool {
    std::env::var_os(crate::lens_profiles::ENV).is_some_and(|v| !v.is_empty())
}

#[test]
fn folder_setting_is_saved_used_at_once_and_reloaded() {
    let _g = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if skip_for_env() {
        return;
    }
    let before = crate::lens_profiles::configured_dir();
    let settings = scratch("settings");
    let db = scratch("folder");
    std::fs::write(db.join("lenses.xml"), DB).unwrap();
    crate::lens_profiles::set_dir(None);
    let mut s = session(&settings, "http://127.0.0.1:9/none");
    assert_eq!(s.execute("lens.db.status", &json!({})).unwrap()["lenses"], 0);
    // not a folder, a folder without profiles: errors that say what to choose
    assert!(s.execute("lens.db.setDir", &json!({"dir": settings.join("nope").to_string_lossy()})).is_err());
    let empty = scratch("empty");
    let e = s.execute("lens.db.setDir", &json!({"dir": empty.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("no lens profiles"), "{e}");
    assert!(s.execute("lens.db.setDir", &json!({})).is_err());
    let st = s.execute("lens.db.setDir", &json!({"dir": db.to_string_lossy()})).unwrap();
    assert_eq!(st["lenses"], 1);
    assert_eq!(st["origin"], "folder");
    assert_eq!(st["dir"], db.to_string_lossy().as_ref());
    assert!(st["updated"].as_str().is_some_and(|d| d.len() == 10), "{st}");
    // remembered: a new session (a restart) uses it
    crate::lens_profiles::set_dir(None);
    let s2 = session(&settings, "");
    assert_eq!(crate::lens_profiles::configured_dir(), Some(db.clone()));
    assert_eq!(s2.lens_db.status()["lenses"], 1);
    // cleared with null, and remembered so
    s.execute("lens.db.setDir", &json!({"dir": null})).unwrap();
    assert_eq!(s.execute("lens.db.status", &json!({})).unwrap()["lenses"], 0);
    let _ = session(&settings, "");
    assert_eq!(crate::lens_profiles::configured_dir(), None);
    crate::lens_profiles::set_dir(before);
}

#[test]
fn download_installs_lensfun_and_failures_are_messages() {
    let _g = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if skip_for_env() {
        return;
    }
    let before = crate::lens_profiles::configured_dir();
    crate::lens_profiles::set_dir(None);
    let settings = scratch("download");
    let fail = |status: &'static str, body: Vec<u8>| -> String {
        let mut s = session(&settings, &serve(status, body));
        s.execute("lens.db.download", &json!({})).unwrap();
        match s.lens_db.wait() {
            Some(crate::lens_download::State::Failed(e)) => e,
            other => panic!("{status}: {other:?}"),
        }
    };
    let e = fail("404 Not Found", b"missing".to_vec());
    assert!(e.contains("could not download the lens database"), "{e}");
    let e = fail("200 OK", b"<html>a captive portal</html>".to_vec());
    assert!(e.contains("not a gzip archive"), "{e}");
    let good = archive();
    let e = fail("200 OK", good[..good.len() / 2].to_vec());
    assert!(e.contains("damaged") || e.contains("incomplete"), "{e}");
    let e = fail("200 OK", gzip(&crate::lens_download::tests::entry("x/data/db/a.xml", b"<lensdatabase/>", b'0')));
    assert!(e.contains("no lens profiles"), "{e}");
    // nothing was put in place by a failure
    assert_eq!(crate::lens_profiles::configured_dir(), None);
    assert!(!settings.join("lensfun").join("db").exists());

    let mut s = session(&settings, &serve("200 OK", good));
    let st = s.execute("lens.db.download", &json!({})).unwrap();
    assert_eq!(st["download"]["state"], "running");
    assert!(s.execute("lens.db.download", &json!({})).is_err() || s.lens_db.wait().is_some());
    assert_eq!(s.lens_db.wait(), Some(crate::lens_download::State::Done { lenses: 1 }));
    let st = s.execute("lens.db.status", &json!({})).unwrap();
    assert_eq!(st["lenses"], 1, "{st}");
    assert_eq!(st["downloaded"], true);
    assert_eq!(st["download"]["state"], "done");
    let target = settings.join("lensfun").join("db");
    assert!(target.join("slr-testcam.xml").is_file());
    // nothing outside the folder, nothing but the database
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    assert!(!settings.join("evil.xml").exists() && !settings.join("lensfun").join("evil.xml").exists());
    // remembered for the next start
    crate::lens_profiles::set_dir(None);
    let mut s = session(&settings, "");
    assert_eq!(s.lens_db.status()["lenses"], 1);
    // Remove deletes a downloaded database
    let st = s.execute("lens.db.remove", &json!({})).unwrap();
    assert_eq!(st["deleted"], true);
    assert_eq!(st["lenses"], 0);
    assert!(!settings.join("lensfun").exists());
    crate::lens_profiles::set_dir(before);
}

#[test]
fn a_download_without_a_settings_folder_is_refused() {
    let mut s = Session::new();
    let e = s.execute("lens.db.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains("settings folder"), "{e}");
}

#[test]
fn optics_is_told_why_there_is_no_correction() {
    let _g = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if skip_for_env() {
        return;
    }
    use crate::lens_profiles::Match;
    let before = crate::lens_profiles::configured_dir();
    crate::lens_profiles::set_dir(None);
    let mut s = Session::new();
    let raw = |lens: &str| {
        let mut p = lightcraft_catalog::Photo::new(
            lightcraft_catalog::PhotoId(1),
            lightcraft_catalog::Source::Demo { scene: 0 },
            "a.nef",
            "NEF",
            10,
            10,
            "now",
        );
        p.kind = lightcraft_catalog::MediaKind::Raw;
        p.meta.lens = lens.into();
        p
    };
    assert_eq!(s.lens_match(&raw("18-55mm F3.5-5.6")), Some(Match::NoDatabase));
    let mut jpeg = raw("x");
    jpeg.kind = lightcraft_catalog::MediaKind::Image;
    let db = scratch("optics");
    std::fs::write(db.join("lenses.xml"), DB).unwrap();
    s.lens_db.set_dir(Some(db)).unwrap();
    assert_eq!(s.lens_match(&jpeg), Some(Match::NotRaw));
    assert_eq!(s.lens_match(&raw("")), Some(Match::NoLensName));
    assert_eq!(s.lens_match(&raw("70-200mm F4")), Some(Match::NotFound("70-200mm F4".into())));
    assert!(matches!(s.lens_match(&raw("18-55mm F3.5-5.6")), Some(Match::Found(_))));
    let mut own = raw("70-200mm F4");
    own.embedded_lens = Some(Default::default());
    assert_eq!(s.lens_match(&own), None);
    // the command says the same about a catalogued photo
    let id = s.catalog.alloc_photo_id();
    let mut p = raw("70-200mm F4");
    p.id = id;
    s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let v: Value = s.execute("lens.db.photo", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(v, json!({"state": "notFound", "lens": "70-200mm F4"}));
    // a change makes every photo's lens data stale until it is looked up again
    assert!(s.lens_profiles_stale(id));
    s.execute("lens.db.refresh", &json!({"ids": [id.0]})).unwrap();
    assert!(!s.lens_profiles_stale(id));
    crate::lens_profiles::reload();
    assert!(s.lens_profiles_stale(id));
    crate::lens_profiles::set_dir(before);
}
