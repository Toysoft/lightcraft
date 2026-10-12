//! Camera calibration wizard; every mutation goes through the engine command registry.
use crate::{LightcraftApp, widgets::register};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Wizard {
    pub folder: String,
    pub scope: usize,
    pub names: std::collections::BTreeMap<String, String>,
    pub allow_worse: bool,
    pub error: Option<String>,
}
fn action(app: &mut LightcraftApp, state: &mut Wizard, command: &str, params: serde_json::Value) {
    state.error = app.run(command, params).err();
}
fn button(ui: &mut egui::Ui, label: &str, id: &str) -> bool {
    let response = ui.button(label);
    register(ui.ctx(), id, response.rect);
    response.clicked()
}
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, state: &mut Wizard) {
    ui.label("Build a starting-colour profile from your RAWs and their embedded camera JPEGs. Photos stay on this computer.");
    ui.label("Choose varied subjects and lighting, all shot with the same picture style. Standard with DRO off is recommended. Include at least 7 usable photos per camera; 20–50 is better.");
    ui.small("About one in five usable photos is kept out of training. Camera styles cannot currently be identified automatically; split different styles into separate folders or selections.");
    ui.separator();
    let report = app.session.calibration.snapshot();
    ui.add_enabled_ui(!report.running, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.scope, 0, "Selected photos");
            ui.selectable_value(&mut state.scope, 1, "Current collection");
            ui.selectable_value(&mut state.scope, 2, "Folder");
        });
        if state.scope == 2 {
            ui.horizontal(|ui| {
                let response = ui.text_edit_singleline(&mut state.folder);
                register(ui.ctx(), "field:calibrationFolder", response.rect);
                if button(ui, "Browse…", "button:calibrationBrowse")
                    && let Some(path) = app.services.pick_folder.as_mut().and_then(|pick| pick())
                {
                    state.folder = path;
                }
            });
        }
        if button(ui, "Build and check profiles", "button:calibrationStart") {
            let params = match state.scope {
                2 => json!({"paths": [state.folder]}),
                1 => json!({"scope": "collection"}),
                _ => json!({}),
            };
            state.allow_worse = false;
            action(app, state, "cameraProfile.start", params);
        }
    });
    if !report.stage.is_empty() {
        ui.strong(&report.stage);
        if report.running {
            let fraction = if report.total > 0 { report.done as f32 / report.total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(fraction).text(format!("{} / {}", report.done, report.total)));
            if button(ui, "Cancel processing", "button:calibrationCancel") {
                action(app, state, "cameraProfile.cancel", json!({}));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
        for (model, count) in &report.models {
            ui.label(format!("{model}: {count} usable photos"));
        }
        for warning in &report.warnings {
            ui.small(warning);
        }
    }
    if !report.running {
        for result in &report.results {
            ui.separator();
            ui.strong(&result.profile.model);
            ui.label(format!("{} training · {} held-out photos · {} colour pairs", result.profile.files, result.holdout, result.profile.samples));
            ui.label(format!("Held-out camera-JPEG RMS: {:.5} current → {:.5} proposed (lower is better)", result.before, result.after));
            ui.small("Measures starting-colour agreement with each camera JPEG, including the normal per-photo tone fit; it is not an absolute colour-accuracy score.");
            if !result.improves() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Held-out error did not improve. Keep the current profile unless you deliberately choose otherwise.",
                );
            }
            let name = state.names.entry(result.profile.model.clone()).or_insert_with(|| format!("{} Standard", result.profile.model));
            let response = ui.text_edit_singleline(name);
            register(ui.ctx(), format!("field:calibrationName:{}", result.profile.model), response.rect);
            let name = name.clone();
            if !result.improves() {
                ui.checkbox(&mut state.allow_worse, "Allow installing a profile that did not improve");
            }
            ui.add_enabled_ui(result.improves() || state.allow_worse, |ui| {
                if button(ui, "Install named profile", &format!("button:calibrationInstall:{}", result.profile.model)) {
                    action(
                        app,
                        state,
                        "cameraProfile.install",
                        json!({"model": result.profile.model, "name": name, "allowWorse": state.allow_worse}),
                    );
                }
            });
        }
    }
    ui.separator();
    ui.strong("Installed profiles");
    ui.small("Installing adds a choice. Set a model default to use it for photos following that default, or choose it for selected photos in the Profile panel. Removing keeps a copy so it can be restored.");
    for entry in lightcraft_engine::camera_profile_store::list("") {
        let current = lightcraft_engine::camera_profile_store::proxy_choice(&entry.model, "");
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} · {}{}",
                entry.model,
                entry.name,
                if entry.removed {
                    " (removed)"
                } else if current == entry.id {
                    " (model default)"
                } else {
                    ""
                }
            ));
            if !entry.removed && button(ui, "Use as default", &format!("button:calibrationDefault:{}", entry.id)) {
                action(app, state, "cameraProfile.default", json!({"model": entry.model, "profile": entry.id}));
            }
            if button(ui, if entry.removed { "Restore" } else { "Remove" }, &format!("button:calibrationRemove:{}", entry.id)) {
                action(app, state, "cameraProfile.remove", json!({"profile": entry.id, "removed": !entry.removed}));
            }
            if button(ui, "Built-in default", &format!("button:calibrationBuiltin:{}", entry.id)) {
                action(app, state, "cameraProfile.default", json!({"model": entry.model, "profile": "builtin"}));
            }
        });
    }
    if button(ui, "Reload profiles", "button:calibrationReload") {
        action(app, state, "cameraProfile.reload", json!({}));
    }
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

pub fn chooser(app: &mut LightcraftApp, ui: &mut egui::Ui, id: lightcraft_catalog::PhotoId) {
    let Some(photo) = app.session.catalog.photo(id) else { return };
    if !photo.develops_raw() {
        return;
    }
    let camera = photo.meta.camera.clone();
    let choice = photo.develop.profile.camera.clone();
    let entries: Vec<_> = lightcraft_engine::camera_profile_store::list("")
        .into_iter()
        .filter(|e| !e.removed && lightcraft_engine::camera_profile_store::matches_camera(&camera, &e.model))
        .collect();
    let title = if choice.is_empty() {
        "Model default"
    } else if choice == "builtin" {
        "Built in"
    } else {
        entries.iter().find(|e| e.id == choice).map_or("Unavailable — built-in fallback", |e| e.name.as_str())
    };
    let mut selected = choice.clone();
    let response = egui::ComboBox::from_id_salt("camera-profile-choice").selected_text(title).show_ui(ui, |ui| {
        ui.selectable_value(&mut selected, String::new(), "Model default");
        ui.selectable_value(&mut selected, "builtin".into(), "Built in");
        for entry in &entries {
            ui.selectable_value(&mut selected, entry.id.clone(), &entry.name);
        }
    });
    register(ui.ctx(), "combo:cameraProfile", response.response.rect);
    if selected != choice {
        let _ = app.run("cameraProfile.choose", json!({"profile": selected, "ids": [id.0]}));
    }
    if app.session.selection.ids.len() > 1 && button(ui, "Apply to selected photos", "button:cameraProfileSelection") {
        let _ = app.run("cameraProfile.choose", json!({"profile": selected}));
    }
    if button(ui, "Build camera profile…", "button:cameraProfileWizard") {
        let _ = app.run("cameraProfile.wizard", json!({}));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wizard_is_available_without_a_raw_and_explains_validation_and_privacy() {
        let app = crate::LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
        let mut headless = crate::headless::Headless::new(app, [1400.0, 1000.0], 1.0);
        headless.app.run("cameraProfile.wizard", serde_json::json!({})).unwrap();
        for _ in 0..3 {
            headless.step();
        }
        let text = headless.painted_text().join("\n");
        assert!(text.contains("Photos stay on this computer"), "{text}");
        assert!(text.contains("one in five"), "{text}");
        assert!(headless.app.widgets.iter().any(|(name, _)| name == "button:calibrationStart"));
        assert!(headless.app.widgets.iter().any(|(name, _)| name == "button:dialogOk"));
    }
}
