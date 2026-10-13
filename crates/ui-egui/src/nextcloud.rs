//! Nextcloud in the app: the sign-in and import dialogs, the commands whose first request waits for
//! the server (run on a worker, never on the UI thread), what the app checks every frame (sign-ins
//! and imports to their end, results to apply), and Develop's download policy: an unedited photo
//! only looked at stays on the server's preview; an edit tool or a percentage zoom downloads the
//! open photo's original and prefetches the next one's.

use egui::RichText;
use lightcraft_catalog::{Photo, PhotoId, Source, is_remote};
use lightcraft_engine::media::QuickSource;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::render::Slot;
use crate::state::{Dialog, Zoom};
use crate::theme::Tokens;

/// Commands whose first request waits for the server, up to its connect and stall limits.
pub const OFF_UI_THREAD: [&str; 3] = ["nextcloud.addAccount", "nextcloud.signIn", "nextcloud.browse"];
/// How often the app asks how sign-ins and imports are going (s).
const POLL_SECS: f64 = 2.0;
/// The activity row of a sign-in's first request.
const CONNECTING: &str = "Connecting to Nextcloud";

/// The app's Nextcloud state between frames.
#[derive(Default)]
pub struct Nextcloud {
    /// Sign-ins and imports started here whose end is still to be told (activity row ids).
    pending: Vec<u64>,
    /// `nextcloud.preferences` as last read, and when (egui time).
    prefs: Option<(f64, Value)>,
}

/// `nextcloud.preferences`, read at most every 2 s.
pub(crate) fn prefs(app: &mut LightcraftApp, now: f64) -> Value {
    if let Some((at, v)) = &app.nextcloud.prefs
        && (0.0..POLL_SECS).contains(&(now - at))
    {
        return v.clone();
    }
    let v = app.session.execute("nextcloud.preferences", &json!({})).unwrap_or(Value::Null);
    app.nextcloud.prefs = Some((now, v.clone()));
    v
}

/// Read `nextcloud.preferences` again next time (an account or the budget changed).
pub(crate) fn refresh(app: &mut LightcraftApp) {
    app.nextcloud.prefs = None;
}

/// (account key, root folder) of the accounts in `prefs`.
fn accounts(prefs: &Value) -> Vec<(String, String)> {
    prefs["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["account"].as_str()?.to_string(), a["root"].as_str()?.to_string())))
        .collect()
}

/// Run `id` (one of [`OFF_UI_THREAD`]) on a worker with the session's accounts, then `done` with its
/// reply on the UI thread. The activity stack shows it meanwhile.
pub fn query(
    app: &mut LightcraftApp,
    id: &str,
    p: Value,
    done: impl FnOnce(&mut LightcraftApp, &egui::Context, Result<Value, String>) + Send + 'static,
) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, id, p, done);
        Err("Nextcloud isn't available in the browser".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (remote, command) = (app.session.remote.clone(), id.to_string());
        let label = if id == "nextcloud.browse" { "Reading Nextcloud folder" } else { CONNECTING };
        let work = move || {
            // last-resort guard: what a server sends can't take the app down
            lightcraft_engine::guard::catch(&command, || {
                lightcraft_engine::cmd::nextcloud::off_session(&remote, &command, &p)
                    .unwrap_or_else(|| Err(lightcraft_engine::EngineError::UnknownCommand(command.clone())))
                    .map_err(|e| e.to_string())
            })
            .and_then(|r| r)
        };
        crate::tasks::spawn(app, label, Some("nextcloud"), work, done)
    }
}

/// `app.run` of an [`OFF_UI_THREAD`] command: started on a worker; its answer shows as a sign-in
/// would from Settings (the login page opens; a certificate to trust opens the sign-in dialog).
pub fn run(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    let server = p["server"].as_str().unwrap_or_default().to_string();
    let account = p["account"].as_str().map(str::to_string);
    match id {
        "nextcloud.browse" => query(app, id, p.clone(), |_, _, _| {})?,
        _ => query(app, id, p.clone(), move |app, ctx, r| answered(app, ctx, false, server, account, r))?,
    }
    Ok(json!({"running": true}))
}

/// A fresh sign-in dialog: for a new account, or to sign `account` in again.
pub fn sign_in_dialog(account: Option<String>) -> Dialog {
    Dialog::NextcloudSignIn { server: String::new(), account, fingerprint: None, changed: false, login_url: None, task: None, error: None }
}

/// File ▸ Import from Nextcloud…: with one account, its files at once.
pub fn open_import(app: &mut LightcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    refresh(app);
    let accounts = accounts(&prefs(app, ctx.input(|i| i.time)));
    let mut dlg = Dialog::NextcloudImport { dir: String::new(), folders: vec![], chosen: vec![], loading: false, error: None };
    if let [(_, root)] = accounts.as_slice() {
        navigate(app, &mut dlg, root.clone());
    }
    app.ui.dialog = Some(dlg);
    Ok(Value::Null)
}

/// Show `dir`'s folders in the import dialog `dlg` (read on a worker).
fn navigate(app: &mut LightcraftApp, dlg: &mut Dialog, to: String) {
    let Dialog::NextcloudImport { dir, folders, chosen, loading, error } = dlg else { return };
    // (one import is one account: folders chosen on another go)
    let account = |p: &str| p.strip_prefix("nextcloud://").and_then(|r| r.split('/').next()).map(str::to_string);
    if !to.is_empty() {
        chosen.retain(|c| account(c) == account(&to));
    }
    *dir = to.clone();
    folders.clear();
    *error = None;
    *loading = !to.is_empty();
    if to.is_empty() {
        return;
    }
    let shown = to.clone();
    if let Err(e) = query(app, "nextcloud.browse", json!({"dir": to}), move |app, _, r| browsed(app, &shown, r)) {
        *loading = false;
        *error = Some(e);
    }
}

/// What reading `dir` found, shown if the import dialog still shows `dir`.
fn browsed(app: &mut LightcraftApp, read: &str, r: Result<Value, String>) {
    let Some(Dialog::NextcloudImport { dir, folders, loading, error, .. }) = &mut app.ui.dialog else { return };
    if dir != read {
        return;
    }
    *loading = false;
    match r {
        Ok(v) => {
            *folders = v["folders"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| Some((f["name"].as_str()?.to_string(), f["path"].as_str()?.to_string())))
                .collect();
        }
        Err(e) => *error = Some(e),
    }
}

/// The folder above `dir` (`None` at an account's top).
fn parent(dir: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        lightcraft_engine::remote::RemotePath::parse(dir).ok()?.parent().map(|p| p.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = dir;
        None
    }
}

/// How a sign-in's first request answered (`from_dialog`: asked by the sign-in dialog for `server` /
/// `account`, which shows it if still open).
fn answered(app: &mut LightcraftApp, ctx: &egui::Context, from_dialog: bool, server: String, account: Option<String>, r: Result<Value, String>) {
    let mine = |d: &Option<Dialog>| matches!(d, Some(Dialog::NextcloudSignIn { server: s, account: a, .. }) if *s == server && *a == account);
    let shown = from_dialog && mine(&app.ui.dialog);
    match r {
        Ok(v) if v["state"] == "waiting" => {
            let task = v["task"].as_u64();
            if from_dialog && !shown {
                // the dialog was closed meanwhile: stop the sign-in it started
                if let Some(t) = task {
                    crate::panels::activity::cancel(app, t);
                }
                return;
            }
            let url = v["loginUrl"].as_str().unwrap_or_default().to_string();
            app.nextcloud.pending.extend(task);
            refresh(app);
            if let Err(e) = crate::links::open(app, &url) {
                log::warn!("Nextcloud login page {url}: {e}");
            }
            match &mut app.ui.dialog {
                Some(Dialog::NextcloudSignIn { login_url, task: t, error, .. }) if shown => {
                    *login_url = Some(url);
                    *t = task;
                    *error = None;
                }
                _ => app.toast(ctx, tr("Approve LightCraft in your browser…")),
            }
        }
        Ok(v) if v["state"] == "needsTrust" => {
            let fp = v["fingerprint"].as_str().map(str::to_string);
            let is_changed = v["changed"].as_bool().unwrap_or(false);
            if !shown {
                if from_dialog {
                    return;
                }
                app.ui.dialog =
                    Some(Dialog::NextcloudSignIn { server, account, fingerprint: None, changed: false, login_url: None, task: None, error: None });
            }
            if let Some(Dialog::NextcloudSignIn { fingerprint, changed, error, .. }) = &mut app.ui.dialog {
                *fingerprint = fp;
                *changed = is_changed;
                *error = None;
            }
        }
        // signed in at once (`wait`)
        Ok(_) => {
            refresh(app);
            if shown {
                app.ui.dialog = None;
            }
        }
        Err(e) if shown => {
            if let Some(Dialog::NextcloudSignIn { error, .. }) = &mut app.ui.dialog {
                *error = Some(e);
            }
        }
        Err(e) if !from_dialog => app.toast_error(ctx, e),
        Err(_) => {}
    }
}

/// Every frame: apply what work off the session found, keep the photo open in Develop on disk (and
/// download the next one), and every 2 s, while sign-ins or imports started here run, tell how they
/// ended.
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context) {
    app.session.drain_remote();
    #[cfg(not(target_arch = "wasm32"))]
    develop_downloads(app);
    if app.nextcloud.pending.is_empty() {
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(POLL_SECS));
    let prefs = prefs(app, ctx.input(|i| i.time));
    let ended = |list: &str| -> Vec<Value> {
        prefs[list]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| !matches!(o["state"].as_str(), Some("waiting" | "running")))
            .filter(|o| o["task"].as_u64().is_some_and(|t| app.nextcloud.pending.contains(&t)))
            .cloned()
            .collect()
    };
    let (sign_ins, imports) = (ended("signIns"), ended("imports"));
    for o in sign_ins.iter().chain(&imports) {
        app.nextcloud.pending.retain(|t| o["task"].as_u64() != Some(*t));
    }
    for o in sign_ins {
        signed_in(app, ctx, &o);
    }
    for o in imports {
        let n = o["failedFolders"].as_u64().unwrap_or(0);
        match o["state"].as_str() {
            Some("failed") => app.toast_error(ctx, o["error"].as_str().unwrap_or("The Nextcloud import failed").to_string()),
            Some("done") if n > 0 => app.toast_error(ctx, tr("Some Nextcloud folders couldn't be read: import them again to retry").to_string()),
            _ => {}
        }
    }
}

/// How sign-in `o` (a `signIns` entry) ended: in its dialog when open, else a toast if it failed.
fn signed_in(app: &mut LightcraftApp, ctx: &egui::Context, o: &Value) {
    refresh(app);
    let state = o["state"].as_str().unwrap_or_default();
    let shown = matches!(&app.ui.dialog, Some(Dialog::NextcloudSignIn { task: Some(t), .. }) if o["task"].as_u64() == Some(*t));
    let why = o["error"].as_str().unwrap_or("The sign-in failed").to_string();
    match state {
        "signedIn" | "cancelled" if shown => app.ui.dialog = None,
        "failed" | "timedOut" if shown => {
            if let Some(Dialog::NextcloudSignIn { login_url, task, error, .. }) = &mut app.ui.dialog {
                *login_url = None;
                *task = None;
                *error = Some(why);
            }
        }
        "failed" | "timedOut" => app.toast_error(ctx, why),
        _ => {}
    }
}

/// A Nextcloud dialog was closed: a sign-in waiting for approval stops.
pub fn closed(app: &mut LightcraftApp, dlg: &Dialog) {
    if let Dialog::NextcloudSignIn { task: Some(t), .. } = dlg {
        crate::panels::activity::cancel(app, *t);
    }
}

/// Develop wants the open photo's original: an edit tool is open, or the zoom is a percentage.
pub fn wants_original(app: &LightcraftApp) -> bool {
    app.ui.right.is_edit_tool() || matches!(app.ui.zoom, Zoom::Percent(_))
}

/// `photo` is on Nextcloud, unedited and only looked at: its quick view (the server's preview) is all
/// the Detail view shows, and its original isn't downloaded.
pub fn quick_view_is_final(app: &LightcraftApp, photo: &Photo) -> bool {
    matches!(&photo.source, Source::File { path } if is_remote(path)) && !wants_original(app) && app.session.embedded_thumb_is_final(photo)
}

/// The quick view of photo `id` in `slot` didn't come from the server's preview (a thumbnail render
/// of the original instead, or nothing, the request for `quick_key` having failed): the server has
/// none, so the view renders the original as for any other photo.
pub fn no_server_preview(app: &LightcraftApp, slot: Slot, id: PhotoId, quick_key: Option<u64>) -> bool {
    let r = &app.renderer;
    r.textures.get(&slot).is_some_and(|t| t.photo == id && !matches!(t.quick, Some(QuickSource::Embedded | QuickSource::Cached)))
        || quick_key.is_some_and(|k| r.failed_key(slot) == Some(k))
}

/// The quick view (the server's preview) of `photo` for `slot` when that is all the view shows
/// ([`quick_view_is_final`], and the server has a preview), at `edge` px.
pub fn quick_only_job(app: &mut LightcraftApp, slot: Slot, photo: &Photo, edge: usize) -> Option<lightcraft_engine::media::QuickJob> {
    if !quick_view_is_final(app, photo) {
        return None;
    }
    let q = app.session.quick_view_job(photo.id, edge, true)?;
    (!no_server_preview(app, slot, photo.id, Some(q.key))).then_some(q)
}

/// In Develop on a Nextcloud photo: hold its original on disk and, once it is here, download the next
/// photo's; anywhere else, nothing (both calls are no-ops when nothing changed), and the loupe's renders of
/// a Nextcloud photo left behind stop their downloads.
#[cfg(not(target_arch = "wasm32"))]
fn develop_downloads(app: &mut LightcraftApp) {
    let remote_path = |app: &LightcraftApp, id| match &app.session.catalog.photo(id)?.source {
        Source::File { path } if is_remote(path) => Some(path.clone()),
        _ => None,
    };
    let develop = app.ui.view == crate::state::ViewMode::Detail && wants_original(app);
    let open = app.session.active().filter(|_| develop).and_then(|id| Some((id, remote_path(app, id)?)));
    // (the open photo's own download first: the next one's waits until it is on this computer)
    let next = open.as_ref().filter(|(_, p)| app.session.remote.has_local_copy(p)).and_then(|(id, _)| {
        let (_, visible) = app.session.visible_shared();
        let at = visible.iter().position(|p| p == id)?;
        remote_path(app, *visible.get(at.checked_add(1)?)?)
    });
    // (the open photo first: prefetching first would cancel a prefetch of the open photo `hold` keeps)
    app.session.remote.hold(open.as_ref().map(|(_, p)| p.as_str()));
    app.session.remote.prefetch(next.as_deref());
    if app.ui.view != crate::state::ViewMode::Detail {
        for slot in [Slot::Main, Slot::Region, Slot::RegionBefore, Slot::Before, Slot::Hover] {
            if app.renderer.photo_in(slot).is_some_and(|id| remote_path(app, id).is_some()) {
                app.renderer.cancel(slot);
            }
        }
    }
}

// ------------------------------------------------------------------------------------- dialogs

/// The dialogs' bodies; true when Return confirms.
pub fn body(app: &mut LightcraftApp, ui: &mut egui::Ui, dlg: &mut Dialog) -> bool {
    let t = Tokens::get(ui.ctx());
    if matches!(dlg, Dialog::NextcloudImport { .. }) {
        import_body(app, ui, &t, dlg);
        return false;
    }
    let Dialog::NextcloudSignIn { server, account, fingerprint, changed, login_url, error, .. } = dlg else { return false };
    let busy = app.tasks.is_running(CONNECTING);
    let mut confirm = false;
    if let Some(url) = login_url {
        ui.label(tr("Approve LightCraft in your browser…"));
        ui.label(RichText::new(tr("If no browser window opened, go to:")).color(t.text_dim));
        ui.add(egui::Label::new(RichText::new(url.as_str()).color(t.text_label)).selectable(true).wrap());
    } else if let Some(fp) = fingerprint {
        let why = if *changed {
            "The server's certificate changed since you trusted it. Trust the new one only if its fingerprint is the one the server's admin gives you:"
        } else {
            "LightCraft doesn't know this server's certificate. Trust it only if its fingerprint is the one the server's admin gives you:"
        };
        ui.label(tr(why));
        ui.add(egui::Label::new(RichText::new(fp.as_str()).monospace().color(t.text)).selectable(true).wrap());
    } else if let Some(a) = account {
        ui.label(tr("Sign this account in again in your browser:"));
        ui.label(RichText::new(a.as_str()).color(t.text));
    } else {
        ui.label(RichText::new(tr("Your Nextcloud server's address")).color(t.text_label));
        confirm = crate::panels::dialogs::dialog_field(ui, "field:nextcloudServer", server, "cloud.example.com") && !server.trim().is_empty();
    }
    if busy {
        ui.label(RichText::new(tr("Connecting…")).color(t.text_dim));
    } else if let Some(e) = error {
        ui.label(RichText::new(e.as_str()).color(t.reject));
    }
    confirm && !busy
}

fn import_body(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, dlg: &mut Dialog) {
    let accounts = accounts(&prefs(app, ui.input(|i| i.time)));
    if accounts.is_empty() {
        ui.label(tr("There's no Nextcloud account on this computer yet."));
        return;
    }
    let Dialog::NextcloudImport { dir, folders, chosen, loading, error } = &mut *dlg else { return };
    // (where to go once drawn: a folder, its parent, or the accounts (empty))
    let mut go = None;
    if dir.is_empty() {
        ui.label(RichText::new(tr("Choose an account:")).color(t.text_label));
        for (i, (key, root)) in accounts.iter().enumerate() {
            let r = ui.button(key.as_str());
            crate::widgets::register(ui.ctx(), format!("button:nextcloudAccount-{i}"), r.rect);
            if r.clicked() {
                go = Some(root.clone());
            }
        }
    } else {
        let up = parent(dir).or_else(|| (accounts.len() > 1).then(String::new));
        ui.horizontal(|ui| {
            let r = ui.add_enabled(up.is_some() && !*loading, egui::Button::new("‹"));
            crate::widgets::register(ui.ctx(), "button:nextcloudUp", r.rect);
            if r.clicked() {
                go = up.clone();
            }
            ui.add(egui::Label::new(RichText::new(dir.strip_prefix("nextcloud://").unwrap_or(dir)).color(t.text)).truncate());
        });
        if *loading {
            ui.label(RichText::new(tr("Loading…")).color(t.text_dim));
        } else if let Some(e) = error {
            ui.label(RichText::new(e.as_str()).color(t.reject));
        } else if folders.is_empty() {
            ui.label(RichText::new(tr("No folders here.")).color(t.text_dim));
        }
        egui::ScrollArea::vertical().max_height(crate::panels::dialogs::list_height(ui, 320.0)).auto_shrink([false, true]).show(ui, |ui| {
            for (i, (name, path)) in folders.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut on = chosen.contains(path);
                    let r = ui.checkbox(&mut on, name.as_str());
                    crate::widgets::register(ui.ctx(), format!("check:nextcloudFolder-{i}"), r.rect);
                    if r.changed() {
                        chosen.retain(|c| c != path);
                        if on {
                            chosen.push(path.clone());
                        }
                    }
                    let r = ui.small_button("›").on_hover_text(tr("Show the folders inside"));
                    crate::widgets::register(ui.ctx(), format!("button:nextcloudOpen-{i}"), r.rect);
                    if r.clicked() {
                        go = Some(path.clone());
                    }
                });
            }
        });
    }
    if !chosen.is_empty() {
        ui.label(
            RichText::new(tr("The photos in the chosen folders and their subfolders are added where they are, without downloading them."))
                .color(t.text_dim),
        );
    }
    if let Some(to) = go {
        navigate(app, dlg, to);
    }
}

/// The confirm button's label.
pub fn ok_label(app: &LightcraftApp, dlg: &Dialog) -> &'static str {
    match dlg {
        Dialog::NextcloudSignIn { login_url: Some(_), .. } => "",
        Dialog::NextcloudSignIn { fingerprint: Some(_), .. } => "Trust This Certificate",
        Dialog::NextcloudSignIn { .. } => "Sign In",
        _ if !has_accounts(app) => "Add Nextcloud Account…",
        _ => "Import",
    }
}

fn has_accounts(app: &LightcraftApp) -> bool {
    app.nextcloud.prefs.as_ref().is_some_and(|(_, p)| !accounts(p).is_empty())
}

/// Whether the confirm button can be pressed.
pub fn can_confirm(app: &LightcraftApp, dlg: &Dialog) -> bool {
    match dlg {
        Dialog::NextcloudSignIn { server, account, fingerprint, .. } => {
            !app.tasks.is_running(CONNECTING) && (account.is_some() || fingerprint.is_some() || !server.trim().is_empty())
        }
        Dialog::NextcloudImport { chosen, loading, .. } => !has_accounts(app) || (!chosen.is_empty() && !loading),
        _ => true,
    }
}

/// The confirm button: sign in (trusting the certificate shown), or import the chosen folders.
pub fn confirm(app: &mut LightcraftApp, dlg: &Dialog) -> Result<Value, String> {
    match dlg {
        Dialog::NextcloudSignIn { server, account, fingerprint, .. } => {
            let (id, mut p) = match account {
                Some(a) => ("nextcloud.signIn", json!({"account": a})),
                None => ("nextcloud.addAccount", json!({"server": server.trim()})),
            };
            if let Some(fp) = fingerprint {
                p["trust"] = json!(fp);
            }
            let (server, account) = (server.clone(), account.clone());
            query(app, id, p, move |app, ctx, r| answered(app, ctx, true, server, account, r))?;
            Ok(json!({"running": true}))
        }
        Dialog::NextcloudImport { .. } if !has_accounts(app) => {
            app.ui.dialog = Some(sign_in_dialog(None));
            Ok(Value::Null)
        }
        Dialog::NextcloudImport { chosen, .. } => {
            let r = app.run("library.importNextcloud", json!({"dirs": chosen}))?;
            app.nextcloud.pending.extend(r["task"].as_u64());
            Ok(r)
        }
        _ => Ok(Value::Null),
    }
}
