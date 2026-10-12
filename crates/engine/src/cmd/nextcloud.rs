//! Nextcloud commands (`nextcloud.*`): signing in, the accounts on this computer and the originals
//! cache budget. Settings of this computer, not of the library: not journaled.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
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
    // ponytail: no originals cache yet (NC5 opens it and reports its usage)
    Ok(json!({"accounts": accounts, "cacheBudgetMb": s.remote.cache_budget_mb(), "cachedBytes": 0, "pinnedBytes": 0}))
}

fn remove_account(s: &mut Session, p: &Value) -> Result<Value> {
    let key = str_param(p, "account").ok_or_else(|| bad("nextcloud.removeAccount", "missing `account` (user@host)"))?;
    s.remote.remove_account(key).map_err(EngineError::Other)?;
    Ok(json!({"removed": key}))
}

fn add_account(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "nextcloud.addAccount";
    let server = str_param(p, "server").ok_or_else(|| bad(C, "missing `server` (the Nextcloud address)"))?;
    let trust = match str_param(p, "trust") {
        Some(t) => Some(t.parse().map_err(|_| bad(C, "`trust` must be the certificate's SHA-256 fingerprint (AB:CD:…)"))?),
        None => None,
    };
    s.remote.sign_in(server, trust, bool_or(p, "wait", false)).map_err(EngineError::Other)
}

fn sign_in(s: &mut Session, p: &Value) -> Result<Value> {
    let key = str_param(p, "account").ok_or_else(|| bad("nextcloud.signIn", "missing `account` (user@host)"))?;
    s.remote.sign_in_again(key, bool_or(p, "wait", false)).map_err(EngineError::Other)
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "nextcloud.addAccount", "Add Nextcloud Account…", [], None, "{server, trust?: fingerprint, wait?: false} → {state: needsTrust, fingerprint} | {state: waiting, loginUrl} | {state: signedIn, account} — sign in through the browser: open `loginUrl` and approve; the app password goes to the OS keyring. An untrusted certificate answers needsTrust: check the fingerprint with the server's admin and run again with `trust`. Waiting shows a cancellable activity row; `wait: true` blocks until approved", always, add_account),
        cmd!(query "nextcloud.signIn", "Sign In to Nextcloud", [], None, "{account: user@host, wait?: false} → like nextcloud.addAccount — sign the account in again (after its app password was revoked or its certificate changed), with its server and trusted certificate", always, sign_in),
        cmd!(query "nextcloud.preferences", "Nextcloud Preferences", [], None, "{cacheBudgetMb?} → {accounts: [{account, server, user, root, state: signedIn | signedOut | unreachable | signingIn}], cacheBudgetMb, cachedBytes, pinnedBytes} — the Nextcloud accounts on this computer; `cacheBudgetMb` (at least 1000) sets the disk budget for downloaded originals", always, preferences),
        cmd!(query "nextcloud.removeAccount", "Remove Nextcloud Account", [], None, "{account: user@host} → {removed} — forget the account on this computer (its app password and settings); its photos stay in the library, offline", always, remove_account),
    ]
}
