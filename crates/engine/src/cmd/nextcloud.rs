//! Nextcloud commands (`nextcloud.*`): the accounts on this computer and the originals cache budget.
//! Settings of this computer, not of the library: not journaled.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
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

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "nextcloud.preferences", "Nextcloud Preferences", [], None, "{cacheBudgetMb?} → {accounts: [{account, server, user, root, state: signedIn | signedOut | unreachable | signingIn}], cacheBudgetMb, cachedBytes, pinnedBytes} — the Nextcloud accounts on this computer; `cacheBudgetMb` (at least 1000) sets the disk budget for downloaded originals", always, preferences),
        cmd!(query "nextcloud.removeAccount", "Remove Nextcloud Account", [], None, "{account: user@host} → {removed} — forget the account on this computer (its app password and settings); its photos stay in the library, offline", always, remove_account),
    ]
}
