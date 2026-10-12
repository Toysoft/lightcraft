//! Relight commands and controls.

use serde_json::json;

use crate::Session;
use lightcraft_develop::{MAX_RELIGHT_LIGHTS, RelightKind};

fn demo() -> Session {
    Session::with_demo()
}

fn relight(s: &Session) -> lightcraft_develop::Relight {
    s.develop_of(s.active().unwrap()).unwrap().relight.clone()
}

/// Mean brightness of a square of half-side `r` around normalized (`x`, `y`).
fn brightness(img: &lightcraft_raster::Rgba8, x: f64, y: f64, r: usize) -> f64 {
    let (cx, cy) = ((img.width as f64 * x) as usize, (img.height as f64 * y) as usize);
    let (mut sum, mut n) = (0.0, 0.0);
    for py in cy.saturating_sub(r)..(cy + r).min(img.height) {
        for px in cx.saturating_sub(r)..(cx + r).min(img.width) {
            let p = img.get(px, py);
            sum += p[0] as f64 + p[1] as f64 + p[2] as f64;
            n += 3.0;
        }
    }
    sum / n
}

// Given a photo, when a light is added, then Relight is on, the light is where it was put, and the render shows it
#[test]
fn adding_a_light_switches_relight_on_and_lights_the_photo() {
    let mut s = demo();
    let id = s.active().unwrap();
    let before = s.render_now(id, 384, 384).unwrap().image;
    let r = s.execute("relight.addLight", &json!({"position": [0.3, 0.5], "falloff": 15, "depth": 15})).unwrap();
    assert_eq!(r["index"], 0);
    let l = relight(&s);
    assert!(l.enabled && l.is_active());
    assert_eq!((l.lights[0].kind, l.lights[0].position.x, l.lights[0].falloff), (RelightKind::Point, 0.3, 15.0));
    let after = s.render_now(id, 384, 384).unwrap().image;
    assert!(brightness(&after, 0.3, 0.5, 16) > brightness(&before, 0.3, 0.5, 16) + 10.0);
    // switched off, the photo is as it was; the light is kept
    assert_eq!(s.execute("relight.enable", &json!({"enabled": false})).unwrap()["enabled"], false);
    assert_eq!(s.render_now(id, 384, 384).unwrap().image.data, before.data);
    assert_eq!(relight(&s).lights.len(), 1);
    assert_eq!(s.execute("relight.enable", &json!({})).unwrap()["enabled"], true, "without a value it toggles");
}

// Given lights, their sliders are develop controls, and their kind and place a command
#[test]
fn lights_are_edited_by_controls_and_commands() {
    let mut s = demo();
    s.execute("relight.addLight", &json!({})).unwrap();
    s.execute("relight.addLight", &json!({"kind": "spot", "position": [0.7, 0.2], "angle": 500, "cone": -3})).unwrap();
    let l = relight(&s);
    assert_eq!((l.lights[1].kind, l.lights[1].angle, l.lights[1].cone), (RelightKind::Spot, 360.0, 0.0), "numbers are brought into range");
    s.execute("develop.set", &json!({"values": {"relightLight.1.intensity": 150, "relightLight.0.elevation": 400, "relight.ambient": -40}})).unwrap();
    let l = relight(&s);
    assert_eq!((l.lights[1].intensity, l.lights[0].elevation, l.ambient), (150.0, 90.0, -40.0));
    let ctl = s.execute("develop.controls", &json!({"section": "relight"})).unwrap();
    let ids: Vec<&str> = ctl.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"relight.softness") && ids.contains(&"relightLight.1.cone"), "{ids:?}");
    assert_eq!(ids.len(), 4 + 2 * 8, "{ids:?}");
    s.execute("relight.updateLight", &json!({"index": 0, "kind": "directional"})).unwrap();
    s.execute("relight.updateLight", &json!({"index": 1, "position": [9.0, -9.0]})).unwrap();
    let l = relight(&s);
    assert_eq!(l.lights[0].kind, RelightKind::Directional);
    assert_eq!((l.lights[1].position.x, l.lights[1].position.y), (2.0, -1.0), "a light can sit off the photo, not anywhere");
    assert_eq!(s.execute("relight.shadows", &json!({})).unwrap()["on"], true);
    assert!(relight(&s).shadows);
    // the render runs with every kind of light and shadows
    let id = s.active().unwrap();
    assert!(s.render_now(id, 200, 200).is_ok());
    // it survives the settings JSON (what the catalog and XMP sidecars store)
    let d = (*s.develop_of(id).unwrap()).clone();
    assert_eq!(lightcraft_develop::DevelopSettings::from_json(&d.to_json()).unwrap(), d);
    assert_eq!(s.execute("relight.deleteLight", &json!({"index": 0})).unwrap()["lights"], 1);
    assert_eq!(relight(&s).lights[0].kind, RelightKind::Spot);
    s.execute("relight.reset", &json!({})).unwrap();
    assert!(relight(&s).is_default());
}

// Given a wrong argument, the command says what is wrong and changes nothing
#[test]
fn wrong_arguments_are_refused() {
    let mut s = demo();
    s.execute("relight.addLight", &json!({})).unwrap();
    let was = relight(&s);
    for (command, params, says) in [
        ("relight.addLight", json!({"kind": "laser"}), "unknown light kind"),
        ("relight.addLight", json!({"position": "middle"}), "`position` must be"),
        ("relight.addLight", json!({"position": [0.5]}), "`position` must be"),
        ("relight.updateLight", json!({"index": 0}), "nothing to change"),
        ("relight.updateLight", json!({"index": 7, "kind": "spot"}), "no such light"),
        ("relight.updateLight", json!({"kind": "spot"}), "missing `index`"),
        ("relight.updateLight", json!({"index": -1, "kind": "spot"}), "missing `index`"),
        ("relight.updateLight", json!({"index": 1e300, "kind": "spot"}), "missing `index`"),
        ("relight.deleteLight", json!({"index": 1}), "no such light"),
        ("relight.deleteLight", json!({}), "missing `index`"),
    ] {
        let e = s.execute(command, &params).unwrap_err().to_string();
        assert!(e.contains(says), "{command} {params}: {e}");
        assert_eq!(relight(&s), was, "{command} {params}");
    }
    // numbers that are not numbers take the defaults instead of poisoning the settings
    s.execute("relight.addLight", &json!({"intensity": "bright", "hue": null})).unwrap();
    assert_eq!(relight(&s).lights[1].intensity, 100.0);
    // at most MAX_RELIGHT_LIGHTS lights
    while relight(&s).lights.len() < MAX_RELIGHT_LIGHTS {
        s.execute("relight.addLight", &json!({})).unwrap();
    }
    let e = s.execute("relight.addLight", &json!({})).unwrap_err().to_string();
    assert!(e.contains("at most"), "{e}");
}

// Given a light was added, undo takes it away again in one step
#[test]
fn a_light_is_one_undo_step() {
    let mut s = demo();
    s.execute("relight.addLight", &json!({"kind": "directional"})).unwrap();
    assert_eq!(relight(&s).lights.len(), 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(relight(&s).is_default());
}

// Given a photo with an analyzed scene, the commands report the model, and the analysis can be removed
#[test]
fn the_scene_analysis_is_reported_and_removed() {
    let mut s = demo();
    let id = s.active().unwrap();
    s.relief.dir = Some(std::env::temp_dir().join("lightcraft-tests-no-relief-model"));
    let model = s.execute("relight.model", &json!({})).unwrap();
    assert_eq!(
        (model["installed"].as_bool(), model["downloading"].as_bool(), model["analyzing"].is_null()),
        (Some(false), Some(false), true),
        "{model}"
    );
    assert!(model["dir"].as_str().unwrap().contains("lightcraft-tests-no-relief-model"));
    assert_eq!(model["available"], crate::relief::Relief::AVAILABLE);
    // without the model the analysis says what is missing and changes nothing
    let e = s.execute("relight.analyze", &json!({})).unwrap_err().to_string();
    assert!(e.contains("not installed") || e.contains("not available"), "{e}");
    assert_eq!(s.execute("relight.model.cancel", &json!({})).unwrap()["cancelled"], false);
    // a scene stored on the photo lights it differently from the estimate, and can be removed again
    s.execute("relight.addLight", &json!({"kind": "directional", "angle": 270, "elevation": 15})).unwrap();
    let estimated = s.render_now(id, 256, 256).unwrap().image;
    let n = 32 * 32;
    let normal: Vec<[f32; 3]> = (0..n).map(|i| if i % 32 < 16 { [-0.7, 0.0, -0.7] } else { [0.7, 0.0, -0.7] }).collect();
    let geometry = crate::relief::stored_geometry(32, 32, &normal, &vec![2.0; n], &vec![1.0; n]).unwrap();
    s.relief_store(id, geometry).unwrap();
    let seen = s.render_now(id, 256, 256).unwrap().image;
    assert!(brightness(&seen, 0.25, 0.5, 20) > brightness(&seen, 0.75, 0.5, 20) + 10.0, "the half that faces the light is the bright one");
    assert_ne!(seen.data, estimated.data);
    // it survives the settings JSON
    let d = (*s.develop_of(id).unwrap()).clone();
    assert!(d.to_json()["relight"]["geometry"]["data"].is_string());
    assert_eq!(lightcraft_develop::DevelopSettings::from_json(&d.to_json()).unwrap(), d);
    s.execute("relight.clearGeometry", &json!({})).unwrap();
    assert!(relight(&s).geometry.is_none());
    assert_eq!(s.render_now(id, 256, 256).unwrap().image.data, estimated.data);
}

// With the real model (set LIGHTCRAFT_RELIEF_DIR to a folder holding model.pt): a photo is analyzed and keeps what was seen
#[cfg(feature = "relief")]
#[test]
fn the_model_analyzes_a_photo_when_it_is_installed() {
    let Some(dir) = std::env::var_os("LIGHTCRAFT_RELIEF_DIR").map(std::path::PathBuf::from).filter(|d| d.join("model.pt").is_file()) else {
        eprintln!("skipped: LIGHTCRAFT_RELIEF_DIR does not hold the model");
        return;
    };
    let mut s = demo();
    s.relief.dir = Some(dir);
    assert_eq!(s.execute("relight.model", &json!({})).unwrap()["installed"], true);
    assert_eq!(s.execute("relight.analyze", &json!({})).unwrap()["done"], true);
    let geometry = relight(&s).geometry.expect("geometry stored");
    let (w, h) = geometry.size().unwrap();
    assert_eq!(w.max(h), 256);
    let samples = geometry.decode().unwrap();
    let heights: Vec<f32> = samples.iter().map(|s| s.height).collect();
    assert!(heights.iter().any(|v| *v > 0.8) && heights.iter().any(|v| *v < 0.2), "the scene has a near and a far");
    // in the background the result arrives by polling
    s.execute("relight.clearGeometry", &json!({})).unwrap();
    s.relief.background = true;
    assert_eq!(s.execute("relight.analyze", &json!({})).unwrap()["started"], true);
    assert!(s.execute("relight.analyze", &json!({})).unwrap_err().to_string().contains("being analyzed already"));
    assert!(s.activity.list().iter().any(|t| t.kind == "relight"), "the analysis shows in the activity stack");
    let started = std::time::Instant::now();
    let done = loop {
        if let Some(outcome) = s.relief_poll() {
            break outcome;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(600), "the analysis never finished");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(done, Ok(s.active().unwrap()));
    assert!(relight(&s).geometry.is_some() && s.relief.analyzing().is_none());
}

// Relight belongs to one photo's picture: copying settings leaves it out unless it is asked for
#[test]
fn relight_is_copied_only_when_asked() {
    use lightcraft_develop::SettingsGroup;
    assert!(!SettingsGroup::default_copy().contains(&SettingsGroup::Relight));
    let mut d = lightcraft_develop::DevelopSettings::default();
    d.relight.enabled = true;
    d.relight.lights.push(Default::default());
    let copied = lightcraft_develop::extract_groups(&d, &[SettingsGroup::Relight]);
    assert_eq!(copied["relight"]["lights"].as_array().map(Vec::len), Some(1), "{copied}");
    assert!(lightcraft_develop::extract_groups(&d, &SettingsGroup::default_copy()).get("relight").is_none());
}
