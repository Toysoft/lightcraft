//! Nextcloud accounts and settings (`engine::remote`, `nextcloud.*`): the settings file, removing an
//! account, an account this computer doesn't have, signing in (against craft's `FakeServer`), and the app
//! password never leaving the credential store.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_nextcloud::testing::FakeServer;
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

/// Another thread's request is rejected (401) while this one reads the password: the client built
/// with that password must not be handed out or cached for the now signed-out account.
#[test]
fn a_password_rejected_meanwhile_is_not_cached() {
    struct RejectedWhileReading(std::sync::OnceLock<Arc<crate::remote::Remote>>);
    impl CredentialStore for RejectedWhileReading {
        fn get(&self, a: &Account) -> Result<Option<String>, String> {
            if let Some(remote) = self.0.get() {
                remote.note_error(&a.key(), &craft_nextcloud::Error::Unauthorized);
            }
            Ok(Some("rejected-pw".into()))
        }
        fn set(&self, _: &Account, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &Account) -> Result<(), String> {
            Ok(())
        }
    }
    let d = temp("race");
    write_settings(&d, &[account("bob")]);
    let s = Session::new();
    let store = Arc::new(RejectedWhileReading(std::sync::OnceLock::new()));
    let _ = store.0.set(s.remote.clone());
    s.remote.configure(RemoteConfig { settings_file: Some(d.join("nextcloud.json")), cache_dir: None, credentials: Some(store) });
    let again = "Sign in to Nextcloud (bob@cloud.example) again: Settings ▸ Nextcloud";
    assert_eq!(s.remote.client("bob@cloud.example").unwrap_err(), again);
    assert_eq!(s.remote.client("bob@cloud.example").unwrap_err(), again, "nothing was cached");
    assert_eq!(keys(&s), [("bob@cloud.example".to_string(), AccountState::SignedOut)]);
}

// ---------- sign-in (NC4)

/// Poll until `done` holds (fail after 10 s).
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let t = std::time::Instant::now();
    while !done() {
        assert!(t.elapsed() < std::time::Duration::from_secs(10), "{what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The one sign-in row: (kind, label, cancellable).
fn sign_in_row(s: &Session) -> (&'static str, String, bool) {
    let rows = s.activity.list();
    assert_eq!(rows.len(), 1, "{rows:?}");
    (rows[0].kind, rows[0].label.clone(), rows[0].cancellable)
}

const WAITING: &str = "Waiting for Nextcloud approval…";

/// What `nextcloud.preferences` says about sign-in `task`.
fn sign_in_outcome(s: &mut Session, task: &Value) -> Value {
    let prefs = s.execute("nextcloud.preferences", &json!({})).unwrap();
    prefs["signIns"].as_array().unwrap().iter().find(|o| o["task"] == *task).cloned().unwrap_or_else(|| panic!("no sign-in {task}: {prefs}"))
}

#[test]
fn plain_sign_in_adds_the_account() {
    let fake = FakeServer::start();
    let d = temp("plain-sign-in");
    let (mut s, creds) = nc_session(&d);
    let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url()})).unwrap();
    assert_eq!(r["state"], "waiting", "{r}");
    assert!(r["loginUrl"].as_str().unwrap().starts_with(&fake.url()), "{r}");
    assert_eq!(sign_in_row(&s), ("nextcloud", WAITING.to_string(), true));
    assert_eq!(r["task"], s.activity.list()[0].id, "the reply names the activity row");
    assert_eq!(sign_in_outcome(&mut s, &r["task"]), json!({"task": r["task"], "state": "waiting"}));
    assert!(s.remote.accounts().is_empty(), "not added before approval");

    fake.approve_login();
    wait_until("the sign-in never finished", || s.activity.list().is_empty());
    let key = fake.account().key();
    assert_eq!(keys(&s), [(key.clone(), AccountState::SignedIn)]);
    assert_eq!(sign_in_outcome(&mut s, &r["task"]), json!({"task": r["task"], "state": "signedIn", "account": key}));
    assert_eq!(creds.get(&fake.account()).unwrap().as_deref(), Some(FakeServer::PASSWORD));
    assert!(!r.to_string().contains(FakeServer::PASSWORD), "{r}");
    let file = std::fs::read_to_string(d.join("nextcloud.json")).unwrap();
    assert!(file.contains(FakeServer::USER) && !file.contains(FakeServer::PASSWORD), "{file}");
    let root = s.remote.accounts()[0].0.root();
    assert!(s.remote.client(&key).unwrap().list(&root).is_ok(), "the stored password works");
    let prefs = s.execute("nextcloud.preferences", &json!({})).unwrap();
    assert!(!prefs.to_string().contains(FakeServer::PASSWORD), "{prefs}");
}

#[test]
fn wait_true_blocks_until_signed_in() {
    let fake = FakeServer::start();
    let d = temp("wait-sign-in");
    let (mut s, _) = nc_session(&d);
    let activity = s.activity.clone();
    let key = fake.account().key();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            wait_until("no sign-in row", || activity.list().iter().any(|r| r.kind == "nextcloud" && r.label == WAITING));
            fake.approve_login();
        });
        let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url(), "wait": true})).unwrap();
        assert_eq!(r, json!({"state": "signedIn", "account": key}));
    });
    assert!(s.activity.list().is_empty(), "the row is gone");
    assert_eq!(keys(&s), [(key, AccountState::SignedIn)]);
}

#[test]
fn tls_sign_in_asks_for_trust() {
    let fake = FakeServer::start_tls(crate::remote::provider());
    let fp = fake.fingerprint().unwrap();
    let d = temp("tls-sign-in");
    let (mut s, _) = nc_session(&d);
    let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url()})).unwrap();
    assert_eq!(r, json!({"state": "needsTrust", "fingerprint": fp.to_string()}));
    assert!(s.activity.list().is_empty() && s.remote.accounts().is_empty(), "nothing waits for the answer");

    let other = craft_nextcloud::Fingerprint([0xAB; 32]).to_string();
    let e = s.execute("nextcloud.addAccount", &json!({"server": fake.url(), "trust": other})).unwrap_err().to_string();
    assert!(e.contains("The certificate's fingerprint changed; check it with the server's admin"), "{e}");
    let e = s.execute("nextcloud.addAccount", &json!({"server": fake.url(), "trust": "nope"})).unwrap_err().to_string();
    assert!(e.contains("trust"), "{e}");
    assert!(s.activity.list().is_empty() && s.remote.accounts().is_empty());

    let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url(), "trust": fp.to_string()})).unwrap();
    assert_eq!(r["state"], "waiting", "{r}");
    fake.approve_login();
    wait_until("the sign-in never finished", || s.activity.list().is_empty());
    let accounts = s.remote.accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!((accounts[0].0.trust, accounts[0].1), (Some(fp), AccountState::SignedIn));
    let key = accounts[0].0.key();
    assert!(s.remote.client(&key).unwrap().list(&accounts[0].0.root()).is_ok(), "the pinned certificate is trusted");

    // signing in again reuses the pinned certificate: no question asked
    s.remote.note_error(&key, &craft_nextcloud::Error::Unauthorized);
    let r = s.execute("nextcloud.signIn", &json!({"account": key, "wait": true})).unwrap();
    assert_eq!(r, json!({"state": "signedIn", "account": key}));
    assert_eq!(s.remote.accounts()[0].0.trust, Some(fp));
}

#[test]
fn cancelling_the_row_stops_sign_in() {
    let fake = FakeServer::start();
    let d = temp("cancel-sign-in");
    let (mut s, creds) = nc_session(&d);
    let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url()})).unwrap();
    let id = s.activity.list()[0].id;
    s.activity.cancel(id).unwrap();
    // the row goes when the sign-in thread ends
    wait_until("the row stayed", || s.activity.list().is_empty());
    assert!(s.remote.accounts().is_empty(), "a cancelled sign-in adds nothing");
    assert_eq!(creds.get(&fake.account()).unwrap(), None);
    assert_eq!(sign_in_outcome(&mut s, &r["task"])["state"], "cancelled");
}

/// Nobody approved before craft's deadline: told apart from a cancel.
#[test]
fn an_unapproved_sign_in_times_out() {
    let fake = FakeServer::start();
    let d = temp("timeout-sign-in");
    let (mut s, _) = nc_session(&d);
    s.remote.expire_sign_ins.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url()})).unwrap();
    assert_eq!(r["state"], "waiting", "{r}");
    wait_until("the row stayed", || s.activity.list().is_empty());
    let outcome = sign_in_outcome(&mut s, &r["task"]);
    assert_eq!(outcome["state"], "timedOut", "{outcome}");
    assert!(outcome["error"].as_str().unwrap().contains("in time"), "{outcome}");
    assert!(s.remote.accounts().is_empty());
}

#[test]
fn sign_in_again_signs_the_account_back_in() {
    let fake = FakeServer::start();
    let d = temp("sign-in-again");
    write_settings(&d, &[fake.account()]);
    let (mut s, creds) = nc_session(&d);
    let key = fake.account().key();
    assert!(s.remote.client(&key).is_err(), "no password: signed out");
    assert_eq!(keys(&s), [(key.clone(), AccountState::SignedOut)]);

    // cancelled: back to signed out
    let r = s.execute("nextcloud.signIn", &json!({"account": key})).unwrap();
    assert_eq!(r["state"], "waiting", "{r}");
    assert_eq!(keys(&s), [(key.clone(), AccountState::SigningIn)]);
    assert!(s.remote.client(&key).is_err(), "no client while signing in");
    assert_eq!(sign_in_row(&s).0, "nextcloud");
    s.activity.cancel(s.activity.list()[0].id).unwrap();
    wait_until("the row stayed", || s.activity.list().is_empty());
    assert_eq!(keys(&s), [(key.clone(), AccountState::SignedOut)]);

    // approved: signed in, with the new password
    s.execute("nextcloud.signIn", &json!({"account": key})).unwrap();
    fake.approve_login();
    wait_until("the sign-in never finished", || s.activity.list().is_empty());
    assert_eq!(keys(&s), [(key.clone(), AccountState::SignedIn)]);
    assert_eq!(creds.get(&fake.account()).unwrap().as_deref(), Some(FakeServer::PASSWORD));
    assert!(s.remote.client(&key).is_ok());

    let e = s.execute("nextcloud.signIn", &json!({"account": "bob@nowhere"})).unwrap_err().to_string();
    assert!(e.contains("No Nextcloud account bob@nowhere"), "{e}");
    let e = s.execute("nextcloud.signIn", &json!({})).unwrap_err().to_string();
    assert!(e.contains("account"), "{e}");
    let e = s.execute("nextcloud.addAccount", &json!({})).unwrap_err().to_string();
    assert!(e.contains("server"), "{e}");
}

#[test]
fn no_keyring_is_an_actionable_error() {
    struct LockedKeyring;
    impl CredentialStore for LockedKeyring {
        fn get(&self, _: &Account) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _: &Account, _: &str) -> Result<(), String> {
            Err("The OS keyring is locked".into())
        }
        fn delete(&self, _: &Account) -> Result<(), String> {
            Ok(())
        }
    }
    let fake = FakeServer::start();
    fake.approve_login();
    let d = temp("no-keyring");
    let mut s = Session::new();
    let config = RemoteConfig { settings_file: Some(d.join("nextcloud.json")), cache_dir: None, credentials: Some(Arc::new(LockedKeyring)) };
    s.remote.configure(config.clone());
    let e = s.execute("nextcloud.addAccount", &json!({"server": fake.url(), "wait": true})).unwrap_err().to_string();
    assert!(e.contains("The OS keyring is locked"), "{e}");
    assert!(s.remote.accounts().is_empty() && s.activity.list().is_empty());

    // without waiting, the failure comes back through nextcloud.preferences
    let r = s.execute("nextcloud.addAccount", &json!({"server": fake.url()})).unwrap();
    assert_eq!(r["state"], "waiting", "{r}");
    wait_until("the row stayed", || s.activity.list().is_empty());
    let outcome = sign_in_outcome(&mut s, &r["task"]);
    assert_eq!(outcome["state"], "failed", "{outcome}");
    assert!(outcome["error"].as_str().unwrap().contains("The OS keyring is locked"), "{outcome}");
    assert!(!outcome.to_string().contains(FakeServer::PASSWORD), "{outcome}");
    assert!(s.remote.accounts().is_empty());

    s.remote.configure(RemoteConfig { credentials: None, ..config });
    let e = s.execute("nextcloud.addAccount", &json!({"server": fake.url()})).unwrap_err().to_string();
    assert!(e.contains("This build can't store Nextcloud passwords (no OS keyring support)"), "{e}");
    assert!(s.activity.list().is_empty());
}

/// R8: a changed certificate signs the account out like a revoked password (see
/// `sign_in_again_asks_to_trust_a_changed_certificate` for signing it back in).
#[test]
fn certificate_change_signs_the_account_out() {
    let d = temp("cert-changed");
    write_settings(&d, &[account("bob")]);
    let (s, creds) = nc_session(&d);
    creds.set(&account("bob"), "pw").unwrap();
    let key = "bob@cloud.example";
    s.remote.client(key).unwrap();
    s.remote.note_error(key, &craft_nextcloud::Error::CertificateChanged);
    assert_eq!(keys(&s)[0].1, AccountState::SignedOut);
    assert!(s.remote.client(key).unwrap_err().contains("again"));
}

/// R10: the server's certificate changed since it was pinned: signing in again shows the new
/// fingerprint (`changed`), and only a `trust` naming it re-pins the account.
#[test]
fn sign_in_again_asks_to_trust_a_changed_certificate() {
    let fake = FakeServer::start_tls(crate::remote::provider());
    let fp = fake.fingerprint().unwrap();
    let old = craft_nextcloud::Fingerprint([0xAB; 32]);
    let d = temp("re-trust");
    write_settings(&d, &[Account { trust: Some(old), ..fake.account() }]);
    let (mut s, _) = nc_session(&d);
    let key = fake.account().key();

    let r = s.execute("nextcloud.signIn", &json!({"account": key})).unwrap();
    assert_eq!(r, json!({"state": "needsTrust", "fingerprint": fp.to_string(), "changed": true}));
    assert!(s.activity.list().is_empty());
    assert_eq!(s.remote.accounts()[0].0.trust, Some(old), "nothing re-pinned yet");

    let e = s.execute("nextcloud.signIn", &json!({"account": key, "trust": old.to_string()})).unwrap_err().to_string();
    assert!(e.contains("The certificate's fingerprint changed; check it with the server's admin"), "{e}");
    let e = s.execute("nextcloud.signIn", &json!({"account": key, "trust": "nope"})).unwrap_err().to_string();
    assert!(e.contains("trust"), "{e}");

    let r = s.execute("nextcloud.signIn", &json!({"account": key, "trust": fp.to_string()})).unwrap();
    assert_eq!(r["state"], "waiting", "{r}");
    fake.approve_login();
    wait_until("the sign-in never finished", || s.activity.list().is_empty());
    assert_eq!(keys(&s), [(key.clone(), AccountState::SignedIn)]);
    assert_eq!(s.remote.accounts()[0].0.trust, Some(fp), "pinned to the confirmed certificate");
}
