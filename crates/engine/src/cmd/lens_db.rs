//! Settings ▸ Lens profiles as commands: the lens database's status, its folder, downloading lensfun's database,
//! removing it, and looking photos' lenses up again after a change ([`crate::lens_download`]).

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::{Result, Session};

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(s.lens_db.status())
}

fn set_dir(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "lens.db.setDir";
    let dir = match p.get("dir") {
        None => return Err(bad(C, "missing dir (a folder of lensfun-format XML files, or null for none)")),
        Some(Value::Null) => None,
        Some(Value::String(d)) if d.trim().is_empty() => None,
        Some(Value::String(d)) => Some(PathBuf::from(d)),
        Some(_) => return Err(bad(C, "dir must be a folder path or null")),
    };
    if let Some(d) = &dir {
        if !d.is_dir() {
            return Err(bad(C, format!("{} is not a folder", d.display())));
        }
        if crate::lens_profiles::load(d).is_none() {
            return Err(bad(C, format!("{} holds no lens profiles: choose the folder with lensfun's .xml files (its data/db)", d.display())));
        }
    }
    s.lens_db.set_dir(dir).map_err(|e| bad(C, e))?;
    refresh_active(s);
    Ok(s.lens_db.status())
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "lens.db.download";
    let activity = s.activity.clone();
    s.lens_db.start_download(&activity).map_err(|e| bad(C, e))?;
    // a one-shot caller (the CLI) waits for the outcome instead of polling
    if p.get("wait").and_then(Value::as_bool) == Some(true) {
        while s.lens_db.running() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if let Some(crate::lens_download::State::Failed(e)) = s.lens_db.download_state() {
            return Err(bad(C, e));
        }
        refresh_active(s);
    }
    Ok(s.lens_db.status())
}

fn remove(s: &mut Session, _: &Value) -> Result<Value> {
    let deleted = s.lens_db.remove().map_err(|e| bad("lens.db.remove", e))?;
    refresh_active(s);
    let mut v = s.lens_db.status();
    v["deleted"] = json!(deleted);
    Ok(v)
}

/// The active photo picks the change up at once (the others when they are next opened, [`refresh`]).
fn refresh_active(s: &mut Session) {
    if let Some(id) = s.selection.active {
        let _ = s.refresh_lens_profiles(&[id]);
    }
}

fn refresh(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = s.targets(p);
    let changed = s.refresh_lens_profiles(&ids)?;
    Ok(json!({"changed": changed.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

/// What Develop ▸ Optics says about the photo's lens: `embedded` (its own correction), `noDatabase`, `notRaw`, `noLensName`,
/// `notFound` or `found` (with the lens name).
fn photo_status(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "lens.db.photo";
    let id = s.targets(p).first().copied().or(s.selection.active).ok_or_else(|| bad(C, "no photo"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(C, "no such photo"))?;
    Ok(match s.lens_match(ph) {
        None => json!({"state": "embedded"}),
        Some(crate::lens_profiles::Match::NoDatabase) => json!({"state": "noDatabase"}),
        Some(crate::lens_profiles::Match::NoLensName) => json!({"state": "noLensName"}),
        Some(crate::lens_profiles::Match::NotRaw) => json!({"state": "notRaw"}),
        Some(crate::lens_profiles::Match::NotFound(l)) => json!({"state": "notFound", "lens": l}),
        Some(crate::lens_profiles::Match::Found(l)) => json!({"state": "found", "lens": l}),
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "lens.db.status", "Lens Database Status", [], None, "{} → {dir, origin: env | folder | null, env, configured, downloaded, lenses, cameras, updated: YYYY-MM-DD, download: {state: running | done | failed | cancelled, bytes, total, lenses, error}, canDownload, attribution} — the lensfun-format database lens corrections fall back to (the environment variable `env` overrides the Settings folder)", always, status),
        cmd!(query "lens.db.setDir", "Set Lens Database Folder", [], None, "{dir: folder of lensfun-format .xml files | null} — used at once and remembered in the app settings → status", always, set_dir),
        cmd!(query "lens.db.download", "Download Lens Database", [], None, "{wait?} — fetch lensfun's database (CC-BY-SA 3.0) into the settings folder in the background (activity stack, cancellable) and use it; again to update → status (poll lens.db.status; `wait: true` blocks until it is in, for one-shot callers)", always, download),
        cmd!(query "lens.db.downloadCancel", "Cancel Lens Database Download", [], None, "{} → {cancelled}", always, |s, _| Ok(json!({"cancelled": s.lens_db.cancel_download()}))),
        cmd!(query "lens.db.remove", "Remove Lens Database", [], None, "{} — stop using the database (a downloaded one is deleted) → status + {deleted}", always, remove),
        cmd!(
            "lens.db.refresh",
            "Look Up Lens Profiles Again",
            [],
            None,
            "{ids?} — re-read the targets' lens data after the lens database changed → {changed: [ids]}",
            always,
            refresh
        ),
        cmd!(query "lens.db.photo", "Lens Profile for Photo", [], None, "{id?} → {state: embedded | noDatabase | notRaw | noLensName | notFound | found, lens}", always, photo_status),
    ]
}
