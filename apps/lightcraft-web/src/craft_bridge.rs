//! Per-page recovery owner. Each load writes to a fresh id, including duplicated tabs.
//! Storage never gates the app's event loop. Controls use egui and browser APIs use web-sys.
use super::browser_store as store;
use lightcraft_engine::Session;
use std::{cell::RefCell, rc::Rc, time::Duration};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

const TAB_KEY: &str = "lightcraft-rust-session";
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Loading,
    Offer,
    Ready,
    Disabled,
}
struct State {
    mode: Mode,
    started: bool,
    epoch: u64,
    db: Option<web_sys::IdbDatabase>,
    id: String,
    snapshot: Option<Rc<Vec<u8>>>,
    committed_bytes: Option<Rc<Vec<u8>>>,
    restore_requested: bool,
    auto_restore: bool,
    error: Option<String>,
    options: bool,
    saving: bool,
    save_failed: bool,
    candidate: String,
    committed: String,
    changed: f64,
    last_save: f64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            mode: Mode::Loading,
            started: false,
            epoch: 0,
            db: None,
            id: String::new(),
            snapshot: None,
            committed_bytes: None,
            restore_requested: false,
            auto_restore: false,
            error: None,
            options: false,
            saving: false,
            save_failed: false,
            candidate: String::new(),
            committed: String::new(),
            changed: 0.,
            last_save: 0.,
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct Recovery(Rc<RefCell<State>>);
impl Recovery {
    pub fn start(&self, ctx: egui::Context) {
        let epoch = {
            let mut state = self.0.borrow_mut();
            state.epoch = state.epoch.saturating_add(1);
            state.mode = Mode::Loading;
            state.error = None;
            state.epoch
        };
        let owner = self.clone();
        spawn_local(async move {
            let result = async {
                let window = web_sys::window().ok_or("No browser window")?;
                let id = window.crypto().map_err(|_| "Secure browser randomness unavailable")?.random_uuid();
                let tab_storage = window.session_storage().ok().flatten();
                let reload = window
                    .performance()
                    .and_then(|p| p.get_entries_by_type("navigation").get(0).dyn_into::<web_sys::PerformanceNavigationTiming>().ok())
                    .is_some_and(|p| p.type_() == web_sys::NavigationType::Reload);
                let previous = tab_storage.as_ref().and_then(|s| s.get_item(TAB_KEY).ok().flatten());
                let db = store::open().await?;
                let skip = tab_storage.as_ref().is_some_and(|s| s.get_item("lightcraft-rust-session:skip").ok().flatten().as_deref() == Some("1"));
                if skip && let Some(s) = &tab_storage {
                    let _ = s.remove_item("lightcraft-rust-session:skip");
                }
                let previous = if skip {
                    None
                } else if reload && previous.is_some() {
                    previous
                } else {
                    store::read(&db, "latest").await?.as_string()
                };
                let snapshot = match previous {
                    Some(id) => store::load(&db, &id).await?,
                    None => None,
                };
                Ok::<_, String>((db, id, snapshot, reload))
            }
            .await;
            let mut state = owner.0.borrow_mut();
            if state.epoch != epoch {
                return;
            }
            match result {
                Ok((db, id, snapshot, reload)) => {
                    state.db = Some(db);
                    state.id = id;
                    if let Some(bytes) = snapshot {
                        match Session::craft_ui(&bytes) {
                            Ok(_) => state.auto_restore = reload,
                            Err(error) => state.error = Some(error),
                        }
                        state.snapshot = Some(Rc::new(bytes));
                        state.mode = Mode::Offer;
                    } else {
                        state.mode = Mode::Ready;
                    }
                }
                Err(error) => {
                    state.error = Some(error);
                    state.mode = Mode::Disabled;
                }
            }
            ctx.request_repaint();
        });
    }
    pub fn restore(&self, allowed: bool, mut apply: impl FnMut(&[u8]) -> Result<(), String>) {
        let bytes = {
            let mut s = self.0.borrow_mut();
            let wanted = std::mem::take(&mut s.auto_restore) || std::mem::take(&mut s.restore_requested);
            if !wanted {
                return;
            }
            if !allowed {
                s.error = Some("Current work is open. Close it before restoring a recovery copy.".into());
                return;
            }
            s.snapshot.clone()
        };
        let Some(bytes) = bytes else { return };
        let result = apply(&bytes);
        let mut s = self.0.borrow_mut();
        match result {
            Ok(()) => {
                s.mode = Mode::Ready;
                s.snapshot = None;
                s.error = None;
                s.options = false;
                s.candidate.clear();
                s.committed.clear();
            }
            Err(error) => s.error = Some(format!("Could not restore: {error}. The saved copy is unchanged.")),
        }
    }
    fn fresh(&self) {
        let mut state = self.0.borrow_mut();
        state.auto_restore = false;
        state.snapshot = None;
        state.restore_requested = false;
        state.error = None;
        state.options = false;
        state.mode = if state.db.is_some() { Mode::Ready } else { Mode::Disabled };
    }
    pub fn ui(&self, allowed: bool, ctx: &egui::Context) {
        let mut restore = false;
        let mut fresh = false;
        let mut retry = false;
        let mut import = false;
        let mut download = None;
        let mut persist = false;
        {
            let mut s = self.0.borrow_mut();
            let label = if s.error.is_some() {
                "Session not saved · Options"
            } else if s.mode == Mode::Loading {
                "Checking saved session…"
            } else if s.mode == Mode::Offer {
                "Saved session available"
            } else if s.mode == Mode::Disabled {
                "Session recovery off"
            } else if s.saving {
                "Saving session…"
            } else if s.committed.is_empty() {
                "Session recovery"
            } else if s.candidate != s.committed {
                "Session changes pending…"
            } else {
                "Session saved on this device"
            };
            egui::Area::new(egui::Id::new("browser-session-status")).anchor(egui::Align2::RIGHT_BOTTOM, [-12., -8.]).show(ctx, |ui| {
                if ui.small_button(label).clicked() {
                    s.options = true;
                }
            });
            if s.mode == Mode::Offer || s.options {
                egui::Window::new("Session recovery").collapsible(false).resizable(false)
                    .default_width(430.).anchor(egui::Align2::CENTER_CENTER, [0., 0.]).show(ctx, |ui| {
                    if let Some(error) = &s.error { ui.colored_label(ui.visuals().error_fg_color, error); }
                    if s.snapshot.is_some() {
                        ui.label("Restore your saved documents, tabs, view and editing history?");
                        if !allowed { ui.label("Close current documents before restoring. Starting fresh keeps the saved recovery copy."); }
                        ui.horizontal(|ui| {
                            restore = ui.add_enabled(allowed, egui::Button::new("Restore session")).clicked();
                            fresh = ui.button("Start fresh").clicked();
                        });
                    } else {
                        ui.label("Documents and editing history are saved in this browser. Clearing site data removes recovery copies. Save important work separately.");
                        if ui.button("Continue editing").clicked() { s.options = false; }
                    }
                    if ui.button("Download recovery copy").clicked() { download = s.snapshot.clone().or_else(|| s.committed_bytes.clone()); }
                    import = ui.add_enabled(!s.saving && allowed, egui::Button::new("Import recovery copy")).clicked();
                    persist = ui.button("Keep on this device").clicked();
                    retry = ui.button("Retry storage").clicked();
                    if s.mode == Mode::Loading && ui.button("Continue without recovery").clicked() {
                        s.epoch = s.epoch.saturating_add(1); s.mode = Mode::Disabled; s.options = false;
                    }
                });
            }
        }
        if restore {
            self.0.borrow_mut().restore_requested = true;
        }
        if fresh {
            self.fresh();
        }
        if retry {
            if self.0.borrow().db.is_none() {
                self.start(ctx.clone());
            } else {
                let mut s = self.0.borrow_mut();
                s.error = None;
                s.save_failed = false;
                s.committed.clear();
                drop(s);
                let owner = self.clone();
                let ctx = ctx.clone();
                spawn_local(async move {
                    match store::open().await {
                        Ok(db) => owner.0.borrow_mut().db = Some(db),
                        Err(error) => {
                            let mut s = owner.0.borrow_mut();
                            s.save_failed = true;
                            s.error = Some(error);
                        }
                    }
                    ctx.request_repaint();
                });
            }
        }
        if let Some(bytes) = download {
            let owner = self.clone();
            let ctx = ctx.clone();
            spawn_local(async move {
                if let Some(file) = rfd::AsyncFileDialog::new().set_file_name("lightcraft-session.craft-session").save_file().await
                    && let Err(error) = file.write(&bytes).await
                {
                    owner.0.borrow_mut().error = Some(format!("Could not download recovery copy: {error}"));
                    ctx.request_repaint();
                }
            });
        }
        if import {
            let owner = self.clone();
            let ctx = ctx.clone();
            spawn_local(async move {
                if let Some(file) = rfd::AsyncFileDialog::new().add_filter("Recovery copy", &["craft-session"]).pick_file().await {
                    if file.inner().size() > store::LIMIT as f64 {
                        owner.0.borrow_mut().error = Some("Recovery copy exceeds 512 MiB".into());
                        ctx.request_repaint();
                        return;
                    }
                    let bytes = match wasm_bindgen_futures::JsFuture::from(file.inner().array_buffer()).await {
                        Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                        Err(_) => {
                            owner.0.borrow_mut().error = Some("Could not read the recovery copy".into());
                            ctx.request_repaint();
                            return;
                        }
                    };
                    let result = if bytes.len() > store::LIMIT { Err("Recovery copy exceeds 512 MiB".into()) } else { Session::craft_ui(&bytes) };
                    let mut s = owner.0.borrow_mut();
                    match result {
                        Ok(_) => {
                            s.snapshot = Some(Rc::new(bytes));
                            s.mode = Mode::Offer;
                            s.error = None;
                        }
                        Err(error) => s.error = Some(error),
                    }
                    ctx.request_repaint();
                }
            });
        }
        if persist {
            let owner = self.clone();
            let ctx = ctx.clone();
            spawn_local(async move {
                let granted = if let Some(w) = web_sys::window() {
                    match w.navigator().storage().persist() {
                        Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok().and_then(|v| v.as_bool()).unwrap_or(false),
                        Err(_) => false,
                    }
                } else {
                    false
                };
                if !granted {
                    owner.0.borrow_mut().error = Some("Persistent storage was not granted. Keep separate copies of important work.".into());
                }
                ctx.request_repaint();
            });
        }
    }
    pub fn save(&self, signature: String, capture: impl FnOnce() -> Result<Vec<u8>, String>, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_secs(1));
        let now = js_sys::Date::now();
        let (db, id, previous) = {
            let mut s = self.0.borrow_mut();
            if s.mode != Mode::Ready || s.saving || s.save_failed {
                return;
            }
            if s.candidate != signature {
                s.candidate = signature.clone();
                s.changed = now;
            }
            if signature == s.committed || now - s.changed < 1500. || now - s.last_save < 2000. || ctx.input(|i| i.pointer.any_down()) {
                return;
            }
            let Some(db) = s.db.clone() else { return };
            s.saving = true;
            s.last_save = now;
            (db, s.id.clone(), s.committed_bytes.clone())
        };
        let bytes = match capture() {
            Ok(bytes) => Rc::new(bytes),
            Err(error) => {
                let mut s = self.0.borrow_mut();
                s.saving = false;
                s.save_failed = true;
                s.error = Some(error);
                return;
            }
        };
        let owner = self.clone();
        let ctx = ctx.clone();
        spawn_local(async move {
            let result = store::write(&db, &id, &bytes, previous.as_ref().map(|b| b.as_slice())).await;
            let mut s = owner.0.borrow_mut();
            s.saving = false;
            match result {
                Ok(()) => {
                    s.committed = signature;
                    s.committed_bytes = Some(bytes);
                    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
                        let _ = storage.set_item(TAB_KEY, &id);
                    }
                }
                Err(error) => {
                    s.save_failed = true;
                    s.error = Some(error);
                }
            }
            ctx.request_repaint();
        });
    }
}

thread_local! { static OWNER: Recovery = Recovery::default(); }
pub fn wake(ctx: egui::Context) {
    OWNER.with(|o| {
        let started = {
            let mut state = o.0.borrow_mut();
            std::mem::replace(&mut state.started, true)
        };
        if !started {
            o.start(ctx);
        }
    });
}
pub fn restore(allowed: bool, apply: impl FnMut(&[u8]) -> Result<(), String>) -> bool {
    OWNER.with(|o| o.restore(allowed, apply));
    true // Recovery errors must never freeze editing or hide its error controls.
}
pub fn ui(allowed: bool, ctx: &egui::Context) {
    OWNER.with(|o| o.ui(allowed, ctx));
}
pub fn save(signature: Result<String, String>, capture: impl FnOnce() -> Result<Vec<u8>, String>, ctx: &egui::Context) {
    OWNER.with(|o| match signature {
        Ok(signature) => o.save(signature, capture, ctx),
        Err(error) => o.0.borrow_mut().error = Some(error),
    });
}
