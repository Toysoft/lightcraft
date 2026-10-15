//! Auto updates as commands: check the release feed, download what it offers, install it, and the
//! two settings that decide when all that happens ([`crate::update`]).
//!
//! The UI (Help ▸ Check for Updates…), the CLI and the control channel all dispatch these by id, so
//! an agent can drive an update exactly the way the dialog does.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::update::{self, Kind, Progress, Staged};
use crate::{Result, Session};

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(s.updater.status())
}

/// `update.check [wait, feed]`: start a check (background), or run it here and answer with the
/// outcome. `feed` reads another URL instead (a mirror, a local feed); it lasts for this session.
fn check(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "update.check";
    if let Some(feed) = p.get("feed") {
        match feed {
            Value::Null | Value::String(_) => {
                let url = feed.as_str().map(str::to_string);
                if url.as_ref().is_some_and(|u| u.len() > 2_000) {
                    return Err(bad(C, "feed is at most 2000 characters"));
                }
                s.updater.set_manifest_url(url);
            }
            _ => return Err(bad(C, "feed must be a URL or null (the project's own feed)")),
        }
    }
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    if wait && !s.updater.checking() {
        s.updater.check_now();
    } else if !s.updater.start_check() && !s.updater.checking() {
        return Err(bad(C, "a check could not be started; try again"));
    }
    if wait {
        s.updater.wait_for_check();
    }
    Ok(s.updater.status())
}

/// `update.download [wait]`: fetch the file the check offered.
fn download(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "update.download";
    let activity = s.activity.clone();
    s.updater.start_download(&activity).map_err(|e| bad(C, e))?;
    if p.get("wait").and_then(Value::as_bool) == Some(true) {
        s.updater.wait_for_download();
        if let Progress::Failed(why) = s.updater.progress() {
            return Err(bad(C, why.clone()));
        }
    }
    Ok(s.updater.status())
}

fn cancel_download(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"cancelled": s.updater.cancel_download()}))
}

/// `update.install [confirm]`: apply the downloaded file. Without `confirm` it answers with what
/// installing would do (the dialog shows that first).
fn install(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "update.install";
    let Some(staged) = s.updater.staged().cloned() else {
        return Err(bad(C, "no update has been downloaded: run update.check then update.download"));
    };
    if staged.kind == Kind::Checksums {
        return Err(bad(C, "that release offers no file for this system"));
    }
    let plan = update::plan(&staged);
    if p.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Ok(json!({
            "version": staged.version,
            "summary": plan.summary,
            "steps": plan.steps,
            "destination": plan.destination.map(|d| d.to_string_lossy().to_string()),
            "restart": restart_text(plan.restart),
            "file": staged.file,
            "bytes": staged.bytes,
            "sha256": staged.sha256,
        }));
    }
    let installed = update::install(&staged, s.desktop.as_ref()).map_err(|e| bad(C, e))?;
    let exit = installed.exit;
    let out = json!({"version": installed.version, "message": installed.message, "exit": exit});
    s.updater.set_installed(installed);
    Ok(out)
}

fn restart_text(restart: lcc_updater::Restart) -> &'static str {
    match restart {
        lcc_updater::Restart::QuitAndRelaunch => "quitAndRelaunch",
        lcc_updater::Restart::QuitOnly => "quit",
        lcc_updater::Restart::Manual => "manual",
    }
}

/// `update.discard`: delete the downloaded file.
fn discard(s: &mut Session, _: &Value) -> Result<Value> {
    match s.updater.staged().cloned() {
        Some(staged) => {
            update::discard(&staged);
            s.updater.clear_installed();
            Ok(s.updater.status())
        }
        None => Err(bad("update.discard", "no update has been downloaded")),
    }
}

/// `update.prefs`: the two settings, and whether they are remembered between runs.
fn prefs(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "update.prefs";
    let flag = |key: &str| -> Result<Option<bool>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(v)) => Ok(Some(*v)),
            Some(_) => Err(bad(C, format!("{key} must be true or false"))),
        }
    };
    let check_at_startup = flag("checkAtStartup")?;
    let prerelease = flag("prerelease")?;
    if check_at_startup.is_some() || prerelease.is_some() {
        s.updater.set_prefs(check_at_startup, prerelease);
    }
    let mut v = s.updater.status();
    v["persistent"] = json!(s.updater.persistent());
    v["staging"] = json!(s.updater.stage_dir().to_string_lossy());
    Ok(v)
}

/// `update.dismiss`: the user was told about `version` and does not want to be told again.
fn dismiss(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "update.dismiss";
    let version = match p.get("version").and_then(Value::as_str) {
        Some(v) if !v.is_empty() && v.len() <= 64 => v.to_string(),
        _ => match s.updater.available() {
            Some(a) => a.version.clone(),
            None => return Err(bad(C, "no version to dismiss")),
        },
    };
    let skipped = p.get("skipped").and_then(Value::as_bool).unwrap_or(true);
    s.updater.mark_seen(&version, skipped);
    Ok(s.updater.status())
}

/// The staged file, for a caller that wants to know where it is.
pub fn staged_of(s: &Session) -> Option<Staged> {
    s.updater.staged().cloned()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "update.status",
            "Update Status",
            [],
            None,
            "{} → {version, platform, supported, check: never | running | current | found, download: idle | running | ready | failed | cancelled, checkAtStartup, prerelease, lastChecked, seenVersion, repo, page, staging, available?: {version, current, title, tag, published, notes, file, size, action, verified, kind}, progress?: {bytes, total}, staged?: {version, file, bytes, sha256, path, kind}, installed?: {version, message, exit}, error?} — what the updater knows right now",
            always,
            status
        ),
        cmd!(
            query "update.check",
            "Check for Updates",
            [],
            None,
            "{wait?: bool, feed?: url|null} — read the release feed in the background and compare it with this build (one request; a recent answer is reused). `feed` reads another URL instead of the project's own (a mirror, a fork, a local feed) for the rest of the session; null goes back to the default (also `{env}_UPDATE_URL` at start-up). `wait: true` runs it now and answers with the outcome, for one-shot callers → status",
            always,
            check
        ),
        cmd!(
            query "update.download",
            "Download Update",
            [],
            None,
            "{wait?: bool} — fetch the file the check offered for this system into the cache folder (resumable, size- and SHA-256-checked; the activity stack shows it and can cancel it). `wait: true` blocks until it is in, for one-shot callers → status",
            always,
            download
        ),
        cmd!(
            query "update.downloadCancel",
            "Cancel Update Download",
            [],
            None,
            "{} → {cancelled} — stop the transfer; nothing is left behind",
            always,
            cancel_download
        ),
        cmd!(
            query "update.install",
            "Install Update",
            [],
            None,
            "{confirm?: bool} — without `confirm`: what installing the downloaded file would do ({summary, steps, destination, restart: quitAndRelaunch | quit | manual}). With `confirm: true`: do it — run the installer, replace the AppImage, or open the file's folder — and say whether the app has to quit ({message, exit})",
            always,
            install
        ),
        cmd!(
            query "update.discard",
            "Discard Downloaded Update",
            [],
            None,
            "{} — delete the downloaded file → status",
            always,
            discard
        ),
        cmd!(
            query "update.prefs",
            "Update Preferences",
            [],
            None,
            "{checkAtStartup?: bool, prerelease?: bool} — ask the feed when the app starts, and whether prereleases count as updates (saved in the app's own settings folder, `updates.json`; a session opened without a library keeps them in memory only) → status + {persistent, staging}",
            always,
            prefs
        ),
        cmd!(
            query "update.dismiss",
            "Dismiss Update",
            [],
            None,
            "{version?, skipped?: bool} — remember that the user answered about this version, so a start-up check does not ask again (defaults to the version on offer) → status",
            always,
            dismiss
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn session() -> Session {
        // in-memory only: no settings file, no library, no network
        let mut s = Session::new();
        s.updater = crate::update::Updater::new(None);
        s
    }

    #[test]
    fn installing_without_a_download_is_an_error() {
        let mut s = session();
        let err = s.execute("update.install", &json!({"confirm": true})).expect_err("nothing staged");
        assert!(err.to_string().contains("update.download"), "{err}");
        let err = s.execute("update.discard", &json!({})).expect_err("nothing staged");
        assert!(err.to_string().contains("no update"), "{err}");
    }

    #[test]
    fn a_download_without_an_update_is_an_error() {
        let mut s = session();
        let err = s.execute("update.download", &json!({})).expect_err("nothing on offer");
        assert!(err.to_string().contains("check"), "{err}");
    }

    #[test]
    fn the_settings_round_trip_and_are_validated() {
        let mut s = session();
        let v = s.execute("update.prefs", &json!({"checkAtStartup": false, "prerelease": true})).expect("prefs");
        assert_eq!((v["checkAtStartup"].as_bool(), v["prerelease"].as_bool()), (Some(false), Some(true)));
        assert_eq!(v["persistent"], false, "an in-memory session remembers nothing");
        let err = s.execute("update.prefs", &json!({"prerelease": "yes"})).expect_err("not a bool");
        assert!(err.to_string().contains("true or false"), "{err}");
    }

    #[test]
    fn dismissing_remembers_the_version() {
        let mut s = session();
        s.updater.apply(lcc_updater::Checked::Found(Box::new(lcc_updater::Update {
            current: lcc_updater::Version::parse("1.0.0").expect("v"),
            version: lcc_updater::Version::parse("1.2.0").expect("v"),
            title: "App 1.2.0".into(),
            tag: "v1.2.0".into(),
            asset: lcc_updater::Asset {
                name: "app-1.2.0-windows-x64.msi".into(),
                url: "https://example.invalid/a.msi".into(),
                size: 10,
                sha256: None,
                kind: lcc_updater::Kind::Msi,
            },
            sha256: None,
            published: None,
            notes: String::new(),
            action: "install".into(),
        })));
        assert!(s.updater.available().is_some());
        let v = s.execute("update.dismiss", &json!({})).expect("dismiss");
        assert_eq!(v["seenVersion"]["version"], "1.2.0");
        assert_eq!(v["seenVersion"]["skipped"], true);
        // and no update on offer: dismissing with nothing to dismiss is an error
        let mut fresh = session();
        assert!(fresh.execute("update.dismiss", &json!({})).is_err());
    }

    #[test]
    fn the_install_plan_is_available_before_confirming() {
        let mut s = session();
        let dir = std::env::temp_dir().join(format!("lcc-cmd-update-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("app-1.2.0-windows-x64.msi");
        std::fs::write(&file, b"installer").expect("write");
        s.updater.set_progress(Progress::Ready(Box::new(Staged {
            version: "1.2.0".into(),
            path: file,
            bytes: 9,
            sha256: "ab".repeat(32),
            file: "app-1.2.0-windows-x64.msi".into(),
            kind: Kind::Msi,
            notes: String::new(),
        })));
        let v = s.execute("update.install", &json!({})).expect("plan");
        assert_eq!(v["version"], "1.2.0");
        assert!(v["summary"].as_str().is_some_and(|t| t.contains("installer")), "{v}");
        assert!(v["steps"].as_array().is_some_and(|a| !a.is_empty()));
        assert_eq!(v["file"], "app-1.2.0-windows-x64.msi");
        assert_eq!(v["sha256"], "ab".repeat(32));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
