//! Nextcloud accounts and settings (`engine::remote`, `nextcloud.*`): the settings file, removing an
//! account, an account this computer doesn't have, and the app password never leaving the credential store.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::remote::{Account, AccountState, CredentialStore, MemoryCredentials, RemoteConfig};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-nextcloud-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn account(user: &str) -> Account {
    Account { server: "https://cloud.example".into(), user: user.into(), trust: None }
}

/// A session whose Nextcloud settings live in `dir`, with app passwords kept in memory.
fn nc_session(dir: &Path) -> (Session, Arc<MemoryCredentials>) {
    let creds = Arc::new(MemoryCredentials::default());
    let s = Session::new().with_fs();
    s.remote.configure(RemoteConfig {
        settings_file: Some(dir.join("nextcloud.json")),
        cache_dir: Some(dir.join("cache")),
        credentials: Some(creds.clone()),
    });
    (s, creds)
}

fn write_settings(dir: &Path, accounts: &[Account]) {
    std::fs::write(dir.join("nextcloud.json"), serde_json::to_vec(&json!({"accounts": accounts, "cacheBudgetMb": 20000})).unwrap()).unwrap();
}

fn keys(s: &Session) -> Vec<(String, AccountState)> {
    s.remote.accounts().into_iter().map(|(a, state)| (a.key(), state)).collect()
}

#[test]
fn settings_round_trip() {
    let d = temp("round-trip");
    write_settings(&d, &[account("bob"), account("eve")]);
    let (mut s, _) = nc_session(&d);
    assert_eq!(keys(&s), [("bob@cloud.example".to_string(), AccountState::SignedIn), ("eve@cloud.example".to_string(), AccountState::SignedIn)]);
    let r = s.execute("nextcloud.preferences", &json!({})).unwrap();
    assert_eq!(
        r["accounts"][0],
        json!({"account": "bob@cloud.example", "server": "https://cloud.example", "user": "bob", "root": "nextcloud://bob@cloud.example/", "state": "signedIn"})
    );
    assert_eq!((r["cacheBudgetMb"].as_u64(), r["cachedBytes"].as_u64(), r["pinnedBytes"].as_u64()), (Some(20_000), Some(0), Some(0)));

    let r = s.execute("nextcloud.preferences", &json!({"cacheBudgetMb": 5000})).unwrap();
    assert_eq!(r["cacheBudgetMb"], 5000);
    let (again, _) = nc_session(&d);
    assert_eq!(again.remote.cache_budget_mb(), 5000, "the budget is saved");
    assert_eq!(keys(&again).len(), 2, "saving the budget keeps the accounts");

    for bad in [json!(999), json!(-1), json!("lots")] {
        let e = s.execute("nextcloud.preferences", &json!({"cacheBudgetMb": bad})).unwrap_err().to_string();
        assert!(e.contains("the cache budget must be at least 1000 MB"), "{e}");
    }
    assert_eq!(s.remote.cache_budget_mb(), 5000, "a refused budget changes nothing");
}

#[test]
fn no_settings_file_means_defaults() {
    let d = temp("defaults");
    let (s, _) = nc_session(&d);
    assert!(s.remote.accounts().is_empty());
    assert_eq!(s.remote.cache_budget_mb(), 20_000);
    // an unconfigured session (the default) answers too
    let mut plain = Session::new();
    let r = plain.execute("nextcloud.preferences", &json!({})).unwrap();
    assert_eq!(r["accounts"], json!([]));
}

#[test]
fn corrupt_settings_mean_no_accounts() {
    let d = temp("corrupt");
    std::fs::write(d.join("nextcloud.json"), b"{nope").unwrap();
    let (mut s, _) = nc_session(&d);
    assert!(s.remote.accounts().is_empty());
    assert_eq!(s.remote.cache_budget_mb(), 20_000);
    assert_eq!(s.execute("nextcloud.preferences", &json!({})).unwrap()["accounts"], json!([]));

    // a file past 1 MiB isn't read at all, valid or not
    let many: Vec<Account> = (0..20_000).map(|i| account(&format!("user{i}"))).collect();
    write_settings(&d, &many);
    assert!(std::fs::metadata(d.join("nextcloud.json")).unwrap().len() > 1 << 20);
    let (s, _) = nc_session(&d);
    assert!(s.remote.accounts().is_empty());
}

#[test]
fn remove_account_deletes_password_and_entry() {
    let d = temp("remove");
    write_settings(&d, &[account("bob"), account("eve")]);
    let (mut s, creds) = nc_session(&d);
    creds.set(&account("bob"), "secret-pw").unwrap();
    assert!(s.remote.client("bob@cloud.example").is_ok());

    let r = s.execute("nextcloud.removeAccount", &json!({"account": "bob@cloud.example"})).unwrap();
    assert_eq!(r, json!({"removed": "bob@cloud.example"}));
    assert_eq!(creds.get(&account("bob")).unwrap(), None, "the password is gone");
    assert_eq!(keys(&s), [("eve@cloud.example".to_string(), AccountState::SignedIn)]);
    let (again, _) = nc_session(&d);
    assert_eq!(keys(&again), [("eve@cloud.example".to_string(), AccountState::SignedIn)], "the settings file has no entry");
    let e = s.remote.client("bob@cloud.example").unwrap_err();
    assert!(e.contains("add it with File ▸ Add Nextcloud Account…"), "the cached client is dropped too: {e}");

    let e = s.execute("nextcloud.removeAccount", &json!({})).unwrap_err().to_string();
    assert!(e.contains("account"), "{e}");
}

/// Review Focus 4: the catalog names an account this computer doesn't have (removed, or the library
/// came from another computer).
#[test]
fn unknown_account_is_offline() {
    let d = temp("unknown");
    let (mut s, _) = nc_session(&d);
    assert_eq!(
        s.remote.client("bob@nowhere").unwrap_err(),
        "No Nextcloud account bob@nowhere on this computer; add it with File ▸ Add Nextcloud Account…"
    );
    let e = s.execute("nextcloud.removeAccount", &json!({"account": "bob@nowhere"})).unwrap_err().to_string();
    assert!(e.contains("No Nextcloud account bob@nowhere"), "{e}");
    // an unconfigured session says the same
    assert!(Session::new().remote.client("bob@nowhere").unwrap_err().contains("File ▸ Add Nextcloud Account…"));
}

#[test]
fn account_without_password_must_sign_in_again() {
    let d = temp("no-password");
    write_settings(&d, &[account("bob")]);
    let (s, creds) = nc_session(&d);
    let again = "Sign in to Nextcloud (bob@cloud.example) again: Settings ▸ Nextcloud";
    assert_eq!(s.remote.client("bob@cloud.example").unwrap_err(), again);
    assert_eq!(keys(&s), [("bob@cloud.example".to_string(), AccountState::SignedOut)]);
    // signed out short-circuits: a password that turns up isn't used until the account signs in again
    creds.set(&account("bob"), "pw").unwrap();
    assert_eq!(s.remote.client("bob@cloud.example").unwrap_err(), again);
}

#[test]
fn errors_change_the_account_state() {
    let d = temp("errors");
    write_settings(&d, &[account("bob")]);
    let (s, creds) = nc_session(&d);
    creds.set(&account("bob"), "pw").unwrap();
    let key = "bob@cloud.example";
    s.remote.client(key).unwrap();

    s.remote.note_error(key, &craft_nextcloud::Error::NotFound);
    assert_eq!(keys(&s)[0].1, AccountState::SignedIn, "a missing file says nothing about the account");
    s.remote.note_error(key, &craft_nextcloud::Error::Unreachable("timed out".into()));
    assert_eq!(keys(&s)[0].1, AccountState::Unreachable);
    assert!(s.remote.client(key).is_ok(), "an unreachable account keeps its client (cached originals still open)");

    s.remote.note_error(key, &craft_nextcloud::Error::Unauthorized);
    assert_eq!(keys(&s)[0].1, AccountState::SignedOut);
    assert!(s.remote.client(key).unwrap_err().contains("again"), "a revoked password short-circuits");
    s.remote.note_error(key, &craft_nextcloud::Error::Unreachable("timed out".into()));
    assert_eq!(keys(&s)[0].1, AccountState::SignedOut, "being unreachable doesn't sign the account back in");
}

#[test]
fn preferences_never_show_the_password() {
    let d = temp("secret");
    write_settings(&d, &[account("bob")]);
    let (mut s, creds) = nc_session(&d);
    creds.set(&account("bob"), "hunter2-secret").unwrap();
    s.remote.client("bob@cloud.example").unwrap();
    let r: Value = s.execute("nextcloud.preferences", &json!({"cacheBudgetMb": 3000})).unwrap();
    assert!(!r.to_string().contains("hunter2-secret"), "{r}");
    let file = std::fs::read_to_string(d.join("nextcloud.json")).unwrap();
    assert!(file.contains("bob") && !file.contains("hunter2-secret"), "{file}");
}
