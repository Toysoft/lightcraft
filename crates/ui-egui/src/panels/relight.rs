//! The Relight panel: the lights placed on the photo, and how the scene takes them. The lights are
//! placed on the photo itself (`detail::relight_overlay`).

use egui::RichText;
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, MAX_RELIGHT_LIGHTS, RelightKind};
use serde_json::json;

use super::edit::{control, sub_title};
use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{divider, register, segmented, text_button};

fn padded(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 6 }).show(ui, add);
}

/// The selected light, kept inside the list (lights come and go by undo, sync and scripts).
pub(crate) fn selected(app: &mut LightcraftApp, d: &DevelopSettings) -> usize {
    let n = d.relight.lights.len();
    if app.ui.relight_light >= n {
        app.ui.relight_light = n.saturating_sub(1);
    }
    app.ui.relight_light
}

/// Add a light at normalized `at` and select it.
pub(crate) fn add_light(app: &mut LightcraftApp, ctx: &egui::Context, at: lightcraft_geom::Point) {
    match app.run("relight.addLight", json!({"position": [at.x, at.y]})) {
        Ok(added) => app.ui.relight_light = added["index"].as_u64().unwrap_or(0) as usize,
        Err(why) => app.toast(ctx, why),
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = app.session.develop_of(id).unwrap_or_default();
    let n = d.relight.lights.len();
    let sel = selected(app, &d);
    super::right::header(ui, "Relight");
    // first of all, the photo's shapes: everything below lights what the analysis found
    scene(app, ui, id, &d);
    padded(ui, |ui| {
        let mut on = d.relight.enabled;
        let check = ui.checkbox(&mut on, crate::i18n::tr("Enable Relight"));
        register(ui.ctx(), "check:relight.enabled", check.rect);
        if check.changed() {
            let _ = app.run("relight.enable", json!({"enabled": on}));
        }
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::tr("Add a light, then drag its marker on the photo to place it.")).color(t.text_dim));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for i in 0..n {
                let name = format!("{} {}", crate::i18n::tr("Light"), i + 1);
                if text_button(ui, &format!("relightLight-{i}"), &name, i == sel).clicked() {
                    app.ui.relight_light = i;
                }
            }
            if n < MAX_RELIGHT_LIGHTS && text_button(ui, "relightAdd", "Add Light", false).clicked() {
                // in the middle of what is shown: the crop's centre
                add_light(app, ui.ctx(), d.crop.geometry.rect.center());
            }
        });
    });
    if let Some(light) = d.relight.lights.get(sel).copied() {
        divider(ui);
        sub_title(ui, &format!("{} {}", crate::i18n::tr("Light"), sel + 1));
        let kinds: Vec<(&str, &str)> = RelightKind::ALL.iter().map(|k| (k.label(), kind_key(*k))).collect();
        let mut chosen = None;
        padded(ui, |ui| {
            chosen = segmented(ui, "relightKind", &kinds, RelightKind::ALL.iter().position(|k| *k == light.kind), 3);
        });
        if let Some(kind) = chosen.and_then(|i| RelightKind::ALL.get(i)) {
            let _ = app.run("relight.updateLight", json!({"index": sel, "kind": kind_key(*kind)}));
        }
        let mut fields = vec!["intensity"];
        if light.kind.has_direction() {
            fields.extend(["angle", "elevation"]);
        }
        if light.kind.has_position() {
            fields.extend(["depth", "falloff"]);
        }
        if light.kind == RelightKind::Spot {
            fields.push("cone");
        }
        for field in fields {
            control(app, ui, &d, &format!("relightLight.{sel}.{field}"), true);
        }
        sub_title(ui, "Light Color");
        control(app, ui, &d, &format!("relightLight.{sel}.hue"), true);
        control(app, ui, &d, &format!("relightLight.{sel}.sat"), true);
        padded(ui, |ui| {
            if text_button(ui, "relightRemove", "Remove Light", false).clicked() {
                let _ = app.run("relight.deleteLight", json!({"index": sel}));
            }
        });
    }
    divider(ui);
    sub_title(ui, "Scene");
    for id in ["relight.ambient", "relight.softness", "relight.shine"] {
        control(app, ui, &d, id, true);
    }
    padded(ui, |ui| {
        let mut on = d.relight.shadows;
        let check = ui.checkbox(&mut on, crate::i18n::tr("Cast Shadows"));
        register(ui.ctx(), "check:relight.shadows", check.rect);
        if check.changed() {
            let _ = app.run("relight.shadows", json!({"on": on}));
        }
    });
    control(app, ui, &d, "relight.shadowSoftness", d.relight.shadows);
    padded(ui, |ui| {
        if !d.relight.is_default() && text_button(ui, "relightReset", "Reset Relight", false).clicked() {
            let _ = app.run("relight.reset", json!({}));
        }
    });
    ui.add_space(40.0);
}

/// What Relight knows of the photo's shapes, and the way to more: analyze the photo with the scene
/// geometry model (downloading it first, when it isn't there). Not shown in builds without it.
fn scene(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, d: &DevelopSettings) {
    let mut status = app.session.relief.status();
    if !status.available {
        return;
    }
    // The first time the tool is opened on a photo, its scene is analyzed without being asked
    // (when the model is installed and idle; the model itself is never downloaded unasked).
    if status.installed && status.analyzing.is_none() && d.relight.geometry.is_none() && app.ui.relight_analyzed.insert(id.0) {
        if let Err(why) = app.run("relight.analyze", json!({})) {
            app.toast(ui.ctx(), why);
        }
        status = app.session.relief.status();
    }
    let t = Tokens::get(ui.ctx());
    sub_title(ui, "Scene Analysis");
    let analyzed = d.relight.geometry.is_some();
    let analyzing = status.analyzing == Some(id.0);
    padded(ui, |ui| {
        let state = if analyzing {
            "Analyzing the photo…"
        } else if analyzed {
            "Analyzed: lights follow the photo's shapes and depth."
        } else {
            "Not analyzed: the relief is guessed from brightness."
        };
        ui.label(RichText::new(crate::i18n::tr(state)).color(if analyzed && !analyzing { t.text_label } else { t.text_dim }));
        ui.add_space(6.0);
        if status.downloading {
            let pct = status.done.saturating_mul(100).checked_div(status.total).unwrap_or(0);
            ui.label(RichText::new(format!("{} {pct} %", crate::i18n::tr("Downloading the model…"))).color(t.text_dim));
            ui.add_space(4.0);
            if text_button(ui, "relightModelCancel", "Cancel Download", false).clicked() {
                let _ = app.run("relight.model.cancel", json!({}));
            }
            return;
        }
        if let Some(why) = &status.error {
            ui.label(RichText::new(why).size(11.0).color(t.text_dim));
            ui.add_space(4.0);
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            if !status.installed {
                let download = text_button(ui, "relightModelDownload", "Download Model (134 MB)", false);
                if download.on_hover_text(crate::i18n::tr("The MoGe-2 scene model (MIT License): it finds the shapes and depth in a photo")).clicked()
                    && let Err(why) = app.run("relight.model.download", json!({}))
                {
                    app.toast(ui.ctx(), why);
                }
            } else if status.analyzing.is_none() {
                let label = if analyzed { "Analyze Again" } else { "Analyze Photo" };
                if text_button(ui, "relightAnalyze", label, false).clicked()
                    && let Err(why) = app.run("relight.analyze", json!({}))
                {
                    app.toast(ui.ctx(), why);
                }
            }
            if analyzed && !analyzing && text_button(ui, "relightClearGeometry", "Remove", false).clicked() {
                let _ = app.run("relight.clearGeometry", json!({}));
            }
        });
    });
    divider(ui);
}

/// A light kind as the commands name it.
fn kind_key(kind: RelightKind) -> &'static str {
    match kind {
        RelightKind::Point => "point",
        RelightKind::Directional => "directional",
        RelightKind::Spot => "spot",
    }
}
