//! Pixel-accurate zoom, pane clipping and anchored horizontal scrub navigation.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The demo library in a 1400×900 window at 1 px per point, the first photo open in Detail.
fn detail() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn edit(h: &mut Headless, f: impl FnOnce(&mut lightcraft_develop::DevelopSettings)) {
    let id = h.app.session.active().expect("a photo is open");
    let mut s = (*h.app.session.develop_of(id).expect("settings")).clone();
    f(&mut s);
    h.app.session.set_develop(id, s, "Test").expect("edit");
}

fn at_100(h: &mut Headless) -> (f32, f32) {
    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    h.settle(SETTLE);
    let r = h.app.image_rect.expect("the loupe drew the photo");
    (r.width(), r.height())
}

fn size_of(h: &Headless) -> (f32, f32) {
    let id = h.app.session.active().expect("a photo is open");
    let p = h.app.session.catalog.photo(id).expect("photo");
    (p.width as f32, p.height as f32)
}

// Given an uncropped photo, 100 % is its pixel size
#[test]
fn an_uncropped_photo_at_100_is_its_pixel_size() {
    let mut h = detail();
    let (nw, nh) = size_of(&h);
    let (w, hh) = at_100(&mut h);
    assert!((w - nw).abs() < 1.5 && (hh - nh).abs() < 1.5, "{w}×{hh} for {nw}×{nh}");
}

// Given a crop to half the width and a third of the height, 100 % is the cropped pixels
#[test]
fn a_cropped_photo_at_100_is_the_crops_pixel_size() {
    let mut h = detail();
    let (nw, nh) = size_of(&h);
    edit(&mut h, |s| s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.25, y0: 0.2, x1: 0.75, y1: 0.2 + 1.0 / 3.0 });
    let (w, hh) = at_100(&mut h);
    assert!((w - nw / 2.0).abs() < 1.5 && (hh - nh / 3.0).abs() < 1.5, "{w}×{hh} for a crop of {}×{}", nw / 2.0, nh / 3.0);
}

// Given a photo rotated a quarter turn, 100 % is its pixel size with the axes swapped
#[test]
fn a_rotated_photo_at_100_is_its_pixel_size_swapped() {
    let mut h = detail();
    let (nw, nh) = size_of(&h);
    edit(&mut h, |s| s.orientation = lightcraft_geom::Orientation::Rotate90);
    let (w, hh) = at_100(&mut h);
    assert!((w - nh).abs() < 1.5 && (hh - nw).abs() < 1.5, "{w}×{hh} for {nh}×{nw}");
}

/// Zoomed in, side by side and stacked Before / After views each stay in their pane: the margins
/// around the panes and the gap between them show the background, as at Fit. The whole-frame
/// renders used to be drawn unclipped, so the After's (magnified, blurred) covered the Before
/// pane's margins.
#[test]
fn zoomed_before_after_views_stay_in_their_panes() {
    use crate::state::{BeforeAfter, Zoom};
    let mut h = detail();
    for mode in [BeforeAfter::SideBySide, BeforeAfter::TopBottom] {
        let mut shots = Vec::new();
        for zoom in [Zoom::Fit, Zoom::Percent(100.0)] {
            h.app.ui.before_after = mode;
            h.app.ui.zoom = zoom;
            h.app.ui.pan = (0.3, 0.4);
            h.settle(SETTLE);
            let img = h.app.image_rect.expect("After view on screen");
            let canvas = h.app.canvas_rect.expect("canvas");
            shots.push((h.paint(), img, canvas));
        }
        let (zoomed, img, canvas) = &shots[1];
        assert!(!canvas.contains_rect(*img), "{mode:?}: 100 % reaches past the pane ({img:?})");
        // outside both panes: the canvas' edges and the gap between the panes
        let c = canvas.center();
        let points = [
            egui::pos2(canvas.left() + 4.0, c.y),
            egui::pos2(c.x, canvas.top() + 4.0),
            egui::pos2(c.x, canvas.bottom() - 4.0),
            match mode {
                BeforeAfter::SideBySide => c,
                _ => egui::pos2(canvas.left() + 200.0, c.y),
            },
        ];
        for q in points {
            let px = |shot: &egui::ColorImage| shot[(q.x as usize, q.y as usize)];
            assert_eq!(px(zoomed), px(&shots[0].0), "{mode:?}: background at {q:?}, zoomed in as at Fit");
        }
    }
}

/// A crop of no size (or a hair wide), from an agent or a sidecar, gave the loupe a NaN aspect:
/// the photo was laid out at a NaN size, which egui's debug checks stop on (a panic in a debug
/// build) and nothing could be drawn or clicked. It now lays out at a finite size, fitted and at
/// 100 %.
#[test]
fn a_crop_of_no_size_lays_out_at_a_finite_size() {
    let mut h = detail();
    for rect in [[0.5, 0.5, 0.5, 0.5], [0.0, 0.0, 1e-9, 1.0], [0.0, 0.3, 1.0, 0.3]] {
        let r = h.request("engine.execute", json!({"command": "crop.set", "params": {"rect": rect}}), T);
        assert_eq!(r["ok"], true, "{r}");
        for zoom in ["view.zoomFit", "view.zoom100"] {
            let r = h.request("engine.execute", json!({"command": zoom}), T);
            assert_eq!(r["ok"], true, "{zoom}: {r}");
            h.settle(SETTLE);
            let img = h.app.image_rect.expect("the loupe laid the photo out");
            assert!(img.min.is_finite() && img.max.is_finite(), "{rect:?} at {zoom}: {img:?}");
            let r = h.request(
                "ui.pointer",
                json!({"events": [{"kind": "down", "x": 0.5, "y": 0.5}, {"kind": "drag", "x": 0.6, "y": 0.4}, {"kind": "up", "x": 0.6, "y": 0.4}]}),
                T,
            );
            assert_eq!(r["ok"], true, "{r}");
            h.step();
        }
    }
}

/// A scrub keeps the pressed image point fixed, reverses without drift and doesn't click on release.
#[test]
fn scrub_zoom_tracks_horizontal_travel_and_preserves_the_anchor() {
    use crate::panels::detail::Gesture;
    use crate::state::Zoom;
    let mut h = detail();
    assert_eq!(h.request("engine.execute", json!({"command": "tool.zoom"}), T)["ok"], true);
    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    h.step();
    let image = h.app.image_rect.unwrap();
    let at = image.center() + egui::vec2(35.0, 20.0);
    let point = (at - image.min) / image.size();
    let modifiers = egui::Modifiers::default();
    h.app.synthetic.push(egui::Event::PointerMoved(at));
    h.step();
    h.app.synthetic.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    let to = at + egui::vec2(200.0, 40.0);
    h.app.synthetic.push(egui::Event::PointerMoved(to));
    h.step();
    assert!(matches!(h.app.gesture, Some(Gesture::ScrubZoom(_))));
    let Zoom::Percent(percent) = h.app.ui.zoom else { panic!("scrub must zoom in") };
    assert!((percent - 200.0).abs() < 0.01, "{percent}");
    let image = h.app.image_rect.unwrap();
    assert!((image.min + image.size() * point).distance(at) < 0.05, "the press position is the zoom anchor");
    h.app.synthetic.push(egui::Event::PointerMoved(at));
    h.step();
    assert!(matches!(h.app.ui.zoom, Zoom::Percent(p) if (p - 100.0).abs() < 0.01));
    let left = at - egui::vec2(200.0, 0.0);
    h.app.synthetic.push(egui::Event::PointerMoved(left));
    h.step();
    assert!(matches!(h.app.ui.zoom, Zoom::Percent(p) if (p - 50.0).abs() < 0.01));
    h.app.synthetic.push(egui::Event::PointerButton { pos: left, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.step();
    assert!(h.app.gesture.is_none());
    assert!(matches!(h.app.ui.zoom, Zoom::Percent(p) if (p - 50.0).abs() < 0.01), "release must not toggle Fit");
}

/// Shift temporarily scrubs, ordinary dragging still pans, and editing tools retain their input.
#[test]
fn shift_scrub_does_not_replace_pan_or_editing_tools() {
    use crate::state::Zoom;
    let mut h = detail();
    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    h.step();
    let at = h.app.image_rect.unwrap().center();
    let drag = |h: &mut Headless, shift| {
        let r = h.request("ui.drag", json!({"x": at.x, "y": at.y, "toX": at.x + 80.0, "toY": at.y, "shift": shift, "steps": 8}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
    };
    let pan = h.app.ui.pan;
    drag(&mut h, false);
    assert_eq!(h.app.ui.zoom, Zoom::Percent(100.0));
    assert!(h.app.ui.pan.0 < pan.0, "an ordinary drag pans the zoomed photo");
    drag(&mut h, true);
    assert!(matches!(h.app.ui.zoom, Zoom::Percent(p) if p > 100.0));
    assert!(h.app.gesture.is_none());
    h.request("engine.execute", json!({"command": "tool.wbPicker"}), T);
    let zoom = h.app.ui.zoom;
    drag(&mut h, true);
    assert_eq!(h.app.ui.zoom, zoom, "a colour picker must not become scrub zoom");
}

#[test]
fn holding_space_pans_with_the_zoom_tool_selected() {
    use crate::state::Zoom;
    let mut h = detail();
    h.request("engine.execute", json!({"command": "tool.zoom"}), T);
    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    h.step();
    let at = h.app.image_rect.unwrap().center();
    let pan = h.app.ui.pan;
    let key = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    h.app.synthetic.push(key(true));
    h.step();
    let r = h.request("ui.drag", json!({"x": at.x, "y": at.y, "toX": at.x + 80.0, "toY": at.y, "steps": 8}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.synthetic.push(key(false));
    h.step();
    assert_eq!(h.app.ui.zoom, Zoom::Percent(100.0), "Space reserves the drag for pan rather than scrub");
    assert!(h.app.ui.pan.0 < pan.0);
    assert_eq!(h.app.ui.tool, "zoom", "Space is a temporary override");
    assert!(h.app.gesture.is_none());
}

#[test]
fn space_keeps_zoom_toggle_and_slideshow_pause_outside_pan_override() {
    use crate::state::Zoom;
    let mut h = detail();
    h.app.run("view.zoomFit", json!({})).expect("Fit");
    h.request("ui.key", json!({"key": "Space"}), T);
    h.step();
    assert_eq!(h.app.ui.zoom, Zoom::Percent(100.0), "Space retains the ordinary zoom toggle");
    h.app.run("view.slideshow", json!({"interval": 120.0})).expect("slideshow");
    h.app.run("tool.zoom", json!({})).expect("magnifier during slideshow");
    assert_eq!(h.app.ui.tool, "zoom");
    let zoom = h.app.ui.zoom;
    assert!(h.app.ui.slideshow.is_some_and(|(_, _, paused)| !paused));
    h.request("ui.key", json!({"key": "Space"}), T);
    h.step();
    assert!(h.app.ui.slideshow.is_some_and(|(_, _, paused)| paused), "Space pauses even with the magnifier selected");
    assert_eq!(h.app.ui.zoom, zoom);
    h.request("ui.key", json!({"key": "Space"}), T);
    h.step();
    assert!(h.app.ui.slideshow.is_some_and(|(_, _, paused)| !paused));
    assert_eq!(h.app.ui.zoom, zoom);
}

#[test]
fn zoom_button_toggles_the_tool_and_command_preserves_comparison_views() {
    use crate::state::{RightPanel, ViewMode};
    let mut h = detail();
    for tool in ["zoom", ""] {
        let r = h.request("ui.clickWidget", json!({"id": "icon:zoomTool"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        assert_eq!(h.app.ui.tool, tool);
    }
    for right in [RightPanel::Crop, RightPanel::Masking, RightPanel::Remove, RightPanel::RedEye] {
        h.app.ui.right = right;
        h.app.run("tool.zoom", json!({})).expect("select zoom");
        assert_eq!(h.app.ui.right, RightPanel::Edit);
        assert_eq!(h.app.ui.tool, "zoom");
    }
    for view in [ViewMode::Compare, ViewMode::Reference] {
        h.app.ui.view = view;
        h.app.run("tool.zoom", json!({})).expect("select zoom");
        assert_eq!(h.app.ui.view, view);
    }
}
