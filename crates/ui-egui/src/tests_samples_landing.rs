//! Every way into the sample photos lands in the same place: the welcome's thumbnails, its Try
//! Sample Photos button, Help ▸ Add Sample Photos and agents open a sample in Detail, ready to
//! edit. There, until the first change, the banner invites one.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::panels::samples;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

fn library(tag: &str) -> lightcraft_engine::Session {
    let dir = std::env::temp_dir().join(format!("lc-ui-landing-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    s
}

fn headless(session: lightcraft_engine::Session) -> Headless {
    let mut a = LightcraftApp::new(session, Services::default());
    a.ui.view = ViewMode::PhotoGrid;
    let mut h = Headless::new(a, [1300.0, 900.0], 1.0);
    h.step();
    h.step();
    h
}

fn active_scene(h: &Headless) -> Option<u32> {
    let id = h.app.session.active()?;
    match h.app.session.catalog.photo(id)?.source {
        lightcraft_catalog::Source::Demo { scene } => Some(scene),
        _ => None,
    }
}

#[test]
fn samples_added_from_the_menu_land_in_detail_like_a_thumbnail_does() {
    // Given an empty library's welcome
    let mut h = headless(library("menu"));
    // When the samples come by Help ▸ Add Sample Photos (or the welcome's button, or an agent)
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    h.step();
    // Then the first of the welcome's samples is open in Detail
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    let first = lightcraft_engine::demo::welcome_scenes().first().map(|(id, _)| *id);
    assert_eq!(active_scene(&h), first);
}

#[test]
fn samples_added_to_a_library_with_photos_leave_the_view_alone() {
    // (only the welcome's empty library is taken somewhere: a library in use stays as it is)
    // a photo of the person's own keeps the library from being empty
    let dir = std::env::temp_dir().join(format!("lc-ui-landing-photo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[90, 120, 160, 255]; 64] };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(dir.join("own.png"), png).unwrap();
    let mut s = library("own");
    s.execute("library.import", &json!({"paths": [dir.join("own.png").to_string_lossy()]})).unwrap();
    let mut h = headless(s);
    assert!(!crate::panels::empty_library::is_empty(&h.app.session));
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_banner_invites_the_first_edit_until_one_is_made() {
    let mut h = headless(library("invite"));
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    h.step();
    // in Detail on a sample, nothing changed yet: an invitation to edit, with the compare key
    let text = samples::message(&h.app, true).unwrap_or_default();
    assert!(text.starts_with("Drag any slider"), "{text}");
    assert!(text.contains("Y"), "the key Compare Before and After is bound to: {text}");
    // after a first change, the banner says what it says elsewhere
    let r = h.request(
        "engine.execute",
        json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 0.5}}),
        Duration::from_secs(20),
    );
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let text = samples::message(&h.app, true).unwrap_or_default();
    assert!(text.starts_with("These are sample photos"), "{text}");
}

#[test]
fn the_grid_of_samples_keeps_the_usual_message() {
    let mut h = headless(library("grid"));
    h.app.run("library.addSamples", json!({})).unwrap();
    h.app.ui.view = ViewMode::PhotoGrid;
    h.step();
    let text = samples::message(&h.app, true).unwrap_or_default();
    assert!(text.starts_with("These are sample photos"), "{text}");
}

#[test]
fn landing_opens_the_edit_panel_with_its_sliders() {
    // even when the panel put aside for the welcome was another, or none
    let mut h = headless(library("panel"));
    h.app.ui.right_before_welcome = Some(crate::state::RightPanel::Info);
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Edit);
}

#[test]
fn samples_added_while_a_local_folder_is_shown_leave_the_view_alone() {
    // the library has no photos of its own, but the welcome isn't what is on screen
    let mut h = headless(library("folder"));
    h.app.session.source = lightcraft_engine::LibrarySource::Folder;
    h.step();
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    assert_eq!(h.app.ui.view, ViewMode::PhotoGrid);
}
