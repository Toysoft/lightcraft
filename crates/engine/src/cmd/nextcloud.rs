//! Nextcloud commands (`nextcloud.*`): signing in, the accounts on this computer and the originals
//! cache budget (settings of this computer, not of the library: not journaled), browsing folders,
//! importing them (`library.importNextcloud`), and keeping photos' originals on this computer
//! (`photo.makeAvailableOffline`, `photo.removeOfflineCopy`; not journaled either).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_catalog::{Source, is_remote};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::activity::Cancel;
use crate::remote::{Fingerprint, RemotePath};
use crate::{EngineError, Result, Session};

/// Why the offline commands are off.
const NO_REMOTE: &str = "no Nextcloud photos selected";

fn preferences(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "nextcloud.preferences";
    if let Some(v) = p.get("cacheBudgetMb") {
        let mb = v.as_u64().filter(|mb| *mb >= 1000).ok_or_else(|| bad(C, "the cache budget must be at least 1000 MB"))?;
        s.remote.set_cache_budget_mb(mb).map_err(EngineError::Other)?;
    }
    let accounts: Vec<Value> = s
        .remote
        .accounts()
        .into_iter()
        .map(|(a, state)| json!({"account": a.key(), "server": a.server, "user": a.user, "root": a.root().to_string(), "state": state}))
        .collect();
    // (no cache folder configured: nothing cached)
    let usage = s.remote.cache().ok().map(|c| c.usage());
    let (cached, pinned) = usage.map_or((0, 0), |u| (u.total, u.pinned));
    Ok(json!({
        "accounts": accounts, "cacheBudgetMb": s.remote.cache_budget_mb(), "cachedBytes": cached, "pinnedBytes": pinned,
        "signIns": s.remote.sign_ins(), "imports": s.remote.imports(),
    }))
}

fn remove_account(s: &mut Session, p: &Value) -> Result<Value> {
    let key = str_param(p, "account").ok_or_else(|| bad("nextcloud.removeAccount", "missing `account` (user@host)"))?;
    s.remote.remove_account(key).map_err(EngineError::Other)?;
    Ok(json!({"removed": key}))
}

/// The optional `trust` fingerprint the user confirmed.
fn trust(c: &str, p: &Value) -> Result<Option<Fingerprint>> {
    str_param(p, "trust").map(|t| t.parse().map_err(|_| bad(c, "`trust` must be the certificate's SHA-256 fingerprint (AB:CD:…)"))).transpose()
}

fn add_account(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "nextcloud.addAccount";
    let server = str_param(p, "server").ok_or_else(|| bad(C, "missing `server` (the Nextcloud address)"))?;
    s.remote.sign_in(server, trust(C, p)?, bool_or(p, "wait", false)).map_err(EngineError::Other)
}

fn sign_in(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "nextcloud.signIn";
    let key = str_param(p, "account").ok_or_else(|| bad(C, "missing `account` (user@host)"))?;
    s.remote.sign_in_again(key, trust(C, p)?, bool_or(p, "wait", false)).map_err(EngineError::Other)
}

/// The `nextcloud://` folder in `v`.
fn folder(c: &str, v: Option<&Value>) -> Result<RemotePath> {
    let path = v.and_then(Value::as_str).ok_or_else(|| bad(c, "a folder must be a nextcloud://user@host/… path"))?;
    RemotePath::parse(path).map_err(|e| bad(c, format!("{path} isn't a Nextcloud folder: {e}")))
}

fn browse(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = folder("nextcloud.browse", p.get("dir"))?;
    crate::remote::import::browse(&s.remote, &dir).map_err(EngineError::Other)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.importNextcloud";
    let given = p.get("dirs").and_then(Value::as_array).filter(|d| !d.is_empty()).ok_or_else(|| bad(C, "missing `dirs` (nextcloud:// folders)"))?;
    let mut dirs: Vec<RemotePath> = Vec::new();
    for d in given {
        let d = folder(C, Some(d))?;
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    let account = dirs.first().map(RemotePath::account_key).unwrap_or_default();
    if dirs.iter().any(|d| d.account_key() != account) {
        return Err(bad(C, "the folders must be on one Nextcloud account"));
    }
    crate::remote::import::start(s, account, dirs, bool_or(p, "wait", false))
}

/// The originals of the Nextcloud photos among the targets (`ids`/`id`, else the selection), each
/// once (virtual copies share their master's).
fn remote_targets(s: &Session, p: &Value) -> BTreeSet<String> {
    let path = |id| match s.catalog.photo(id).map(|ph| &ph.source) {
        Some(Source::File { path }) if is_remote(path) => Some(path.clone()),
        _ => None,
    };
    s.targets(p).into_iter().filter_map(path).collect()
}

/// Enabled while a selected photo is on Nextcloud.
fn has_remote_target(s: &Session) -> std::result::Result<(), String> {
    if remote_targets(s, &Value::Null).is_empty() { Err(NO_REMOTE.into()) } else { Ok(()) }
}

/// [`remote_targets`] of a command `c`, which is off without any (photos named in the call too).
fn offline_targets(s: &Session, p: &Value, c: &str) -> Result<Vec<String>> {
    let paths = remote_targets(s, p);
    if paths.is_empty() {
        return Err(EngineError::Disabled(c.into(), NO_REMOTE.into()));
    }
    Ok(paths.into_iter().collect())
}

fn make_offline(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.makeAvailableOffline";
    let paths = offline_targets(s, p, C)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let row = s.activity.start("download", "Making photos available offline", Cancel::Flag(cancel.clone()));
    row.progress(0, paths.len() as u64);
    let remote = s.remote.clone();
    if bool_or(p, "wait", false) {
        let (photos, failed) = remote.make_available_offline(&paths, &cancel, &row);
        drop(row);
        // (the downloads complete their photos now)
        s.drain_remote();
        return Ok(json!({"photos": photos, "failed": failed}));
    }
    let (task, photos) = (row.id(), paths.len());
    std::thread::Builder::new()
        .name("nextcloud-offline".into())
        .spawn(move || {
            // last-resort guard: the row goes with the thread, however it ends
            if let Err(e) = crate::guard::catch("Make Available Offline", || remote.make_available_offline(&paths, &cancel, &row)) {
                log::error!("{C}: {e}");
            }
        })
        .map_err(|e| bad(C, format!("could not start: {e}")))?;
    Ok(json!({"started": true, "task": task, "photos": photos}))
}

fn remove_offline(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.removeOfflineCopy";
    let paths = offline_targets(s, p, C)?;
    let cache = s.remote.cache().map_err(EngineError::Other)?;
    // ponytail: craft writes its index once per unpin; a batch unpin there if big selections are slow
    for path in &paths {
        let rp = RemotePath::parse(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
        cache.unpin(&rp).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(json!({"photos": paths.len()}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "nextcloud.browse", "Browse Nextcloud Folder", [], None, "{dir: nextcloud://user@host/folder/} → {dir, folders: [{name, path}], photos} — one level of a Nextcloud folder: its subfolders (sorted by name) and how many photos it holds itself (hidden files left out)", always, browse),
        cmd!(
            "library.importNextcloud",
            "Import from Nextcloud",
            [],
            None,
            "{dirs: [nextcloud://user@host/folder], wait?: false} → {started: true, task} | with wait: {added, skipped, metadata, failedFolders} — add the photos in Nextcloud folders and their subfolders to the library where they are, without downloading them. A worker lists the folders (an `import` activity row; the photos join the library as they are found, with the dimensions and capture time the server's Memories app knows), then reads the camera, lens, exposure and capture time of photos that have none yet from Memories or the start of each file (a cancellable `scan` row). Photos already in the library are skipped, so running it again adds only new files and finishes the metadata; the photos an import adds are one undo step. All folders on one account. How it went (by `task`, the row's id) shows in nextcloud.preferences → imports: subfolders that couldn't be listed (failedFolders), or the error that stopped it (a folder asked for missing, the account signed out or unreachable). Opening another library cancels it. `wait: true` returns once both are done: photos added, skipped, whose metadata was filled, and subfolders that couldn't be listed",
            always,
            import
        ),
        cmd!(query "nextcloud.addAccount", "Add Nextcloud Account…", [], None, "{server, trust?: fingerprint, wait?: false} → {state: needsTrust, fingerprint} | {state: waiting, loginUrl, task} | {state: signedIn, account} — sign in through the browser: open `loginUrl` and approve; the app password goes to the OS keyring. An untrusted certificate answers needsTrust: check the fingerprint with the server's admin and run again with `trust`. Waiting shows a cancellable activity row (id `task`); how it ends shows in nextcloud.preferences → signIns; `wait: true` blocks until approved", always, add_account),
        cmd!(query "nextcloud.signIn", "Sign In to Nextcloud", [], None, "{account: user@host, trust?: fingerprint, wait?: false} → like nextcloud.addAccount — sign the account in again (after its app password was revoked or its certificate changed), with its server and pinned certificate. A certificate other than the pinned one answers {state: needsTrust, fingerprint, changed: true}: check it with the server's admin and run again with `trust` to pin it", always, sign_in),
        cmd!(query "nextcloud.preferences", "Nextcloud Preferences", [], None, "{cacheBudgetMb?} → {accounts: [{account, server, user, root, state: signedIn | signedOut | unreachable | signingIn}], cacheBudgetMb, cachedBytes, pinnedBytes, signIns: [{task, state: waiting | signedIn | failed | cancelled | timedOut, account?, error?}], imports: [{task, state: running | done | failed | cancelled, added, failedFolders, error?}]} — the Nextcloud accounts on this computer, the last 8 sign-ins and the last 8 imports (library.importNextcloud); `cacheBudgetMb` (at least 1000) sets the disk budget for downloaded originals", always, preferences),
        cmd!(query "nextcloud.removeAccount", "Remove Nextcloud Account", [], None, "{account: user@host} → {removed} — forget the account on this computer (its app password and settings); its photos stay in the library, offline", always, remove_account),
        CommandSpec {
            explicit_targets: true,
            ..cmd!(query "photo.makeAvailableOffline", "Make Available Offline", ["Photo"], None, "{ids?, wait?: false} → {started: true, task, photos} | with wait: {photos, failed} — keep the originals of the target Nextcloud photos (`ids`, else the selection; other photos are left out) on this computer: pinned, so the originals cache never evicts them, and downloaded when they aren't here yet, one at a time, behind a cancellable `download` row (photos done of all, id `task`). A photo that fails doesn't stop the rest (`failed`). nextcloud.preferences → pinnedBytes is the space they take", has_remote_target, make_offline)
        },
        CommandSpec {
            explicit_targets: true,
            ..cmd!(query "photo.removeOfflineCopy", "Remove Offline Copy", ["Photo"], None, "{ids?} → {photos} — the originals photo.makeAvailableOffline kept for the target Nextcloud photos are no longer pinned: the originals cache evicts them like any other download when it needs the space", has_remote_target, remove_offline)
        },
    ]
}
