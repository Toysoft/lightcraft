//! Nextcloud commands (`nextcloud.*`): signing in, the accounts on this computer and the originals
//! cache budget (settings of this computer, not of the library: not journaled), browsing folders,
//! and importing them (`library.importNextcloud`).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::remote::{Fingerprint, RemotePath};
use crate::{EngineError, Result, Session};

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
    Ok(
        json!({"accounts": accounts, "cacheBudgetMb": s.remote.cache_budget_mb(), "cachedBytes": cached, "pinnedBytes": pinned, "signIns": s.remote.sign_ins()}),
    )
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

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "nextcloud.browse", "Browse Nextcloud Folder", [], None, "{dir: nextcloud://user@host/folder/} → {dir, folders: [{name, path}], photos} — one level of a Nextcloud folder: its subfolders (sorted by name) and how many photos it holds itself (hidden files left out)", always, browse),
        cmd!(
            "library.importNextcloud",
            "Import from Nextcloud",
            [],
            None,
            "{dirs: [nextcloud://user@host/folder], wait?: false} → {started: true} | with wait: {added, skipped, metadata} — add the photos in Nextcloud folders and their subfolders to the library where they are, without downloading them. A worker lists the folders (an `import` activity row; the photos join the library as they are found, with the dimensions and capture time the server's Memories app knows), then reads the camera, lens, exposure and capture time of photos that have none yet from Memories or the first MiB of each file (a cancellable `scan` row). Photos already in the library are skipped, so running it again adds only new files and finishes the metadata. All folders on one account. `wait: true` returns once both are done: photos added, skipped, and whose metadata was filled",
            always,
            import
        ),
        cmd!(query "nextcloud.addAccount", "Add Nextcloud Account…", [], None, "{server, trust?: fingerprint, wait?: false} → {state: needsTrust, fingerprint} | {state: waiting, loginUrl, task} | {state: signedIn, account} — sign in through the browser: open `loginUrl` and approve; the app password goes to the OS keyring. An untrusted certificate answers needsTrust: check the fingerprint with the server's admin and run again with `trust`. Waiting shows a cancellable activity row (id `task`); how it ends shows in nextcloud.preferences → signIns; `wait: true` blocks until approved", always, add_account),
        cmd!(query "nextcloud.signIn", "Sign In to Nextcloud", [], None, "{account: user@host, trust?: fingerprint, wait?: false} → like nextcloud.addAccount — sign the account in again (after its app password was revoked or its certificate changed), with its server and pinned certificate. A certificate other than the pinned one answers {state: needsTrust, fingerprint, changed: true}: check it with the server's admin and run again with `trust` to pin it", always, sign_in),
        cmd!(query "nextcloud.preferences", "Nextcloud Preferences", [], None, "{cacheBudgetMb?} → {accounts: [{account, server, user, root, state: signedIn | signedOut | unreachable | signingIn}], cacheBudgetMb, cachedBytes, pinnedBytes, signIns: [{task, state: waiting | signedIn | failed | cancelled | timedOut, account?, error?}]} — the Nextcloud accounts on this computer and the last 8 sign-ins; `cacheBudgetMb` (at least 1000) sets the disk budget for downloaded originals", always, preferences),
        cmd!(query "nextcloud.removeAccount", "Remove Nextcloud Account", [], None, "{account: user@host} → {removed} — forget the account on this computer (its app password and settings); its photos stay in the library, offline", always, remove_account),
    ]
}
