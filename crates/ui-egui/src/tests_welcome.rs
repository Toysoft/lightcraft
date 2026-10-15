//! The welcome owns the window while the library is empty: the Tools panel steps aside (and comes
//! back as it was once photos arrive), the bottom bar's rating and flags hide, and a row of sample
//! photos under the buttons opens the samples on the one clicked.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// A new library on disk (the welcome's thumbnails are offered over a library, as the samples
/// banner is).
fn library(tag: &str) -> lightcraft_engine::Session {
    let dir = std::env::temp_dir().join(format!("lc-ui-welcome-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.open_library(dir.join("lib"), false).unwrap();
    s
}

fn headless(session: lightcraft_engine::Session, size: [f32; 2]) -> Headless {
    let mut a = LightcraftApp::new(session, Services::default());
    a.ui.view = ViewMode::PhotoGrid;
    a.ui.right = RightPanel::Edit;
    let mut h = Headless::new(a, size, 1.0);
    h.step();
    h.step();
    h
}

/// Steps until a sample thumbnail has arrived from the worker that draws them (the tiles are
/// there from the first frame, as placeholders).
fn thumbnails_shown(h: &mut Headless) -> bool {
    let arrived = |h: &Headless| h.app.welcome_samples.as_ref().is_some_and(|s| s.has_thumbnail());
    h.step_until(T, |h| arrived(h));
    arrived(h) && h.app.widgets.iter().any(|(w, _)| w.starts_with("button:sample:"))
}

#[test]
fn an_empty_library_puts_the_tools_panel_aside_until_photos_arrive() {
    // Given an empty library opened with the Edit panel out
    let mut h = headless(library("aside"), [1300.0, 900.0]);
    // Then the welcome has the window: the panel is put aside, remembered
    assert_eq!(h.app.ui.right, RightPanel::None);
    assert_eq!(h.app.ui.right_before_welcome, Some(RightPanel::Edit));
    // When photos arrive, it is back as it was
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    assert_eq!(h.app.ui.right_before_welcome, None);
}

#[test]
fn a_panel_opened_while_the_library_is_empty_stays_open() {
    let mut h = headless(library("open"), [1300.0, 900.0]);
    assert_eq!(h.app.ui.right, RightPanel::None);
    h.app.run("panel.info", json!({})).unwrap();
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::Info, "only becoming empty puts it aside");
}

#[test]
fn the_panel_comes_back_after_a_restart_while_the_library_was_empty() {
    // quit while empty: ui.json held the panel put aside; the library has photos by the next launch
    let mut a = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    a.ui.right = RightPanel::None;
    a.ui.right_before_welcome = Some(RightPanel::Info);
    let mut h = Headless::new(a, [1300.0, 900.0], 1.0);
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::Info);
}

#[test]
fn an_editing_tool_put_aside_comes_back_as_the_edit_panel() {
    // the photo it was used on is gone: Masking on a newly arrived photo would be a surprise
    let mut a = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    a.ui.right = RightPanel::None;
    a.ui.right_before_welcome = Some(RightPanel::Masking);
    let mut h = Headless::new(a, [1300.0, 900.0], 1.0);
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::Edit);
}

#[test]
fn a_panel_opened_and_closed_during_the_welcome_stays_closed() {
    let mut h = headless(library("closed"), [1300.0, 900.0]);
    h.app.run("panel.info", json!({})).unwrap();
    h.step();
    h.step();
    h.app.ui.right = RightPanel::None;
    h.step();
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::None, "closing it was the person's choice");
}

#[test]
fn the_thumbnails_go_with_the_welcome_and_the_window_settles() {
    // photos arriving while the thumbnails are still drawing: nothing keeps waiting for them
    let mut h = headless(library("settle"), [1600.0, 1000.0]);
    assert!(h.app.welcome_samples.is_some());
    h.app.run("library.addSamples", json!({})).unwrap();
    h.step();
    h.step();
    assert!(h.app.welcome_samples.is_none());
    assert!(h.settle(T), "nothing left busy");
}

#[test]
fn the_thumbnails_go_when_another_view_replaces_the_welcome() {
    // the library is still empty, but People (or Compare, or a filter's message) is on screen
    let mut h = headless(library("people"), [1600.0, 1000.0]);
    assert!(h.app.welcome_samples.is_some());
    h.app.ui.view = ViewMode::People;
    h.step();
    h.step();
    assert!(h.app.welcome_samples.is_none());
    assert!(h.settle(T), "nothing left busy");
}

#[test]
fn a_row_out_of_room_for_a_moment_keeps_its_thumbnails() {
    // a resize past the threshold and back: no new worker, no placeholders flashing back
    let mut h = headless(library("resize"), [1600.0, 1000.0]);
    assert!(thumbnails_shown(&mut h));
    assert_eq!(h.request("ui.resize", json!({"width": 1600, "height": 440}), T)["ok"], true);
    h.step();
    h.step();
    assert!(h.app.welcome_samples.as_ref().is_some_and(|s| s.has_thumbnail()), "kept while the row is out of room");
    assert_eq!(h.request("ui.resize", json!({"width": 1600, "height": 1000}), T)["ok"], true);
    h.step();
    assert!(h.app.welcome_samples.as_ref().is_some_and(|s| s.has_thumbnail()));
}

#[test]
fn a_short_view_offers_the_button_instead_of_the_row() {
    // a wide but short window: the welcome's view is too low for the row under the message
    let mut h = headless(library("short"), [1300.0, 380.0]);
    h.step();
    assert!(has(&h, "button:emptySamples"));
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with("button:sample:")));
}

#[test]
fn the_bottom_bar_offers_no_rating_or_flags_with_nothing_to_rate() {
    let h = headless(library("bottom"), [1300.0, 900.0]);
    assert!(!has(&h, "star:bottom:1"));
    assert!(!has(&h, "flag:pick"));
    let h = headless(lightcraft_engine::Session::with_demo(), [1300.0, 900.0]);
    assert!(has(&h, "star:bottom:1"));
}

#[test]
fn the_welcome_shows_sample_photos_that_open_the_one_clicked() {
    // Given the welcome of a new library
    let mut h = headless(library("strip"), [1600.0, 1000.0]);
    assert!(thumbnails_shown(&mut h), "the sample thumbnails render and show");
    assert!(!has(&h, "button:emptySamples"), "the row replaces the button");
    let id = h.app.widgets.iter().find(|(w, _)| w.starts_with("button:sample:")).map(|(w, _)| w.clone()).unwrap();
    let scene: u32 = id.trim_start_matches("button:sample:").parse().unwrap();
    // When one is clicked
    assert_eq!(h.request("ui.clickWidget", json!({"id": id}), T)["ok"], true);
    h.step();
    h.step();
    // Then the samples are in, and that photo is open in Detail, ready to edit
    assert!(h.app.session.has_samples());
    let active = h.app.session.active().and_then(|id| h.app.session.catalog.photo(id).cloned()).unwrap();
    assert!(matches!(active.source, lightcraft_catalog::Source::Demo { scene: s } if s == scene));
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert_eq!(h.app.ui.right, RightPanel::Edit, "the panel put aside is back to edit with");
}

#[test]
fn a_column_too_narrow_for_the_row_offers_the_button() {
    // a small window with the sidebar out, as an empty library opens: under three thumbnails wide
    let mut a = LightcraftApp::new(library("narrow"), Services::default());
    a.ui.view = ViewMode::PhotoGrid;
    a.ui.left_panel = true;
    let mut h = Headless::new(a, [560.0, 600.0], 1.0);
    h.step();
    h.step();
    h.step();
    assert!(has(&h, "button:emptySamples"));
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with("button:sample:")));
}

#[test]
fn a_temporary_session_offers_no_samples() {
    // nothing done there is saved: samples would only be lost
    let mut a = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    a.ui.view = ViewMode::PhotoGrid;
    a.library_problem = Some(crate::panels::library_problem::LibraryProblem {
        dismissed: true,
        ..crate::panels::library_problem::LibraryProblem::new("/nowhere", "test")
    });
    let mut h = Headless::new(a, [1300.0, 900.0], 1.0);
    h.step();
    h.step();
    h.step();
    assert!(has(&h, "button:emptyImportPhotos"));
    assert!(!has(&h, "button:emptySamples"));
    assert!(!h.app.widgets.iter().any(|(w, _)| w.starts_with("button:sample:")));
}

#[test]
fn the_welcome_keeps_clear_of_the_grid_header() {
    // a short window: the welcome sits below "All Photos", never over it
    for size in [[1300.0, 440.0], [1300.0, 900.0]] {
        let h = headless(library(&format!("header-{}", size[1])), size);
        let title = h.app.widgets.iter().find(|(w, _)| w == "text:emptyTitle").map(|(_, r)| *r).unwrap();
        // the header is 44 points under the 42-point top bar
        assert!(title.top() >= 42.0 + 44.0, "{size:?}: the title starts at {}", title.top());
    }
}
