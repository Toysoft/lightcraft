//! Headless tests of the Relight tool: its button under Edit, its panel, and placing lights on the photo.

use std::time::Duration;

use lightcraft_develop::{Relight, RelightKind};
use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The demo library in Detail with the Relight panel open, as its tool-strip button opens it.
fn relight_tool() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1200.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    click(&mut h, "icon:relight");
    h
}

fn click(h: &mut Headless, widget: &str) {
    let r = h.request("ui.clickWidget", json!({"id": widget}), T);
    assert_eq!(r["ok"], true, "{widget}: {r}");
    h.settle(SETTLE);
}

fn pointer(h: &mut Headless, events: serde_json::Value) {
    let r = h.request("ui.pointer", json!({"events": events}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
}

fn relight(h: &Headless) -> Relight {
    let id = h.app.session.active().expect("active photo");
    h.app.session.develop_of(id).unwrap_or_default().relight.clone()
}

fn rect_of(h: &Headless, widget: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(id, _)| id == widget).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {widget}"))
}

fn has(h: &Headless, widget: &str) -> bool {
    h.app.widgets.iter().any(|(id, _)| id == widget)
}

// Given the tool strip, the Relight button sits right under Edit; it opens the Relight panel, and Return goes back to Edit
#[test]
fn the_relight_button_is_under_edit_and_opens_its_panel() {
    let mut h = relight_tool();
    assert_eq!((h.app.ui.right, h.app.ui.view), (RightPanel::Relight, ViewMode::Detail));
    let (edit, relight, crop) = (rect_of(&h, "icon:edit"), rect_of(&h, "icon:relight"), rect_of(&h, "icon:crop"));
    assert!((relight.left() - edit.left()).abs() < 0.5, "in the same strip");
    assert!(relight.top() >= edit.bottom() && relight.top() - edit.bottom() < 12.0, "right under Edit: {edit:?} then {relight:?}");
    assert!(crop.top() > relight.bottom(), "and above Crop");
    // an empty panel: the switch, Add Light and the scene sliders; no light to edit
    assert!(has(&h, "check:relight.enabled") && has(&h, "button:relightAdd") && has(&h, "check:relight.shadows"));
    assert!(!has(&h, "button:relightRemove") && !has(&h, "button:relightKind-spot"));
    assert_eq!(crate::menubar::checked(&h.app, "panel.relight"), Some(true));
    exec(&mut h, "tool.done", json!({}));
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    // the button again closes nothing of the photo's edit: it only switches panels
    click(&mut h, "icon:relight");
    assert_eq!(h.app.ui.right, RightPanel::Relight);
    assert!(relight_default(&h));
}

fn relight_default(h: &Headless) -> bool {
    relight(h).is_default()
}

// Given the panel, Add Light adds a light in the middle and selects it; its kind, the shadows and Remove work from the panel
#[test]
fn the_panel_adds_edits_and_removes_lights() {
    let mut h = relight_tool();
    click(&mut h, "button:relightAdd");
    let r = relight(&h);
    assert!(r.enabled && r.lights.len() == 1);
    assert_eq!((r.lights[0].position.x, r.lights[0].position.y, r.lights[0].kind), (0.5, 0.5, RelightKind::Point));
    assert!(has(&h, "button:relightLight-0") && has(&h, "button:relightRemove"));
    click(&mut h, "button:relightAdd");
    assert_eq!((relight(&h).lights.len(), h.app.ui.relight_light), (2, 1), "the new light is the selected one");
    click(&mut h, "button:relightKind-spot");
    assert_eq!(relight(&h).lights.iter().map(|l| l.kind).collect::<Vec<_>>(), [RelightKind::Point, RelightKind::Spot]);
    click(&mut h, "button:relightLight-0");
    assert_eq!(h.app.ui.relight_light, 0);
    click(&mut h, "check:relight.shadows");
    assert!(relight(&h).shadows);
    click(&mut h, "check:relight.enabled");
    assert!(!relight(&h).enabled, "switched off, the lights are kept");
    assert_eq!(relight(&h).lights.len(), 2);
    click(&mut h, "button:relightRemove");
    let r = relight(&h);
    assert_eq!((r.lights.len(), r.lights[0].kind), (1, RelightKind::Spot), "the selected light went");
    click(&mut h, "button:relightReset");
    assert!(relight_default(&h));
}

fn drag_marker(h: &mut Headless, light: usize, dx: f32, dy: f32) {
    let r = h.request("ui.dragWidget", json!({"id": format!("relightMarker:{light}"), "dx": dx, "dy": dy, "steps": 6}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

// Given a light, only a drag of its own marker moves it (as one undo step): clicks and drags elsewhere on the
// photo leave it where it is, and add nothing
#[test]
fn a_light_moves_only_when_its_marker_is_dragged() {
    let mut h = relight_tool();
    // a click on a photo without lights adds none
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "up", "x": 0.3, "y": 0.4}]));
    assert!(relight(&h).lights.is_empty());
    exec(&mut h, "relight.addLight", json!({"position": [0.7, 0.6]}));
    let was = relight(&h);
    // a click elsewhere, a drag across the photo that starts off the marker, a drag that ends on it
    pointer(&mut h, json!([{"kind": "down", "x": 0.2, "y": 0.2}, {"kind": "up", "x": 0.2, "y": 0.2}]));
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.2, "y": 0.3}, {"kind": "move", "x": 0.4, "y": 0.5}, {"kind": "move", "x": 0.5, "y": 0.6}, {"kind": "up", "x": 0.5, "y": 0.6}]),
    );
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.3}, {"kind": "move", "x": 0.5, "y": 0.5}, {"kind": "move", "x": 0.7, "y": 0.6}, {"kind": "up", "x": 0.7, "y": 0.6}]),
    );
    assert_eq!(relight(&h), was, "nothing but its marker moves a light");
    assert!(h.app.gesture.is_none());
    // a drag that starts on its marker takes it along
    let marker = rect_of(&h, "relightMarker:0").center();
    let image = h.app.image_rect.expect("the photo is on screen");
    drag_marker(&mut h, 0, -0.3 * image.width(), -0.2 * image.height());
    let dragged = relight(&h).lights[0].position;
    assert!((dragged.x - 0.4).abs() < 0.02 && (dragged.y - 0.4).abs() < 0.02, "{dragged:?}");
    assert!(rect_of(&h, "relightMarker:0").center().distance(marker) > 50.0, "the marker went with it");
    assert!(h.app.gesture.is_none());
    // … as one step: undo puts it back where the drag began
    exec(&mut h, "edit.undo", json!({}));
    let back = relight(&h).lights[0].position;
    assert!((back.x - 0.7).abs() < 0.01 && (back.y - 0.6).abs() < 0.01, "{back:?}");
}

// Given two lights, a click on a marker selects that light without moving either, and a drag moves only the one taken
#[test]
fn a_click_on_a_marker_selects_its_light() {
    let mut h = relight_tool();
    exec(&mut h, "relight.addLight", json!({"position": [0.25, 0.3]}));
    exec(&mut h, "relight.addLight", json!({"position": [0.75, 0.7]}));
    h.app.ui.relight_light = 1;
    h.settle(SETTLE);
    let was = relight(&h);
    click(&mut h, "relightMarker:0");
    assert_eq!(h.app.ui.relight_light, 0);
    assert_eq!(relight(&h), was, "selecting moves nothing");
    // dragging the other light's marker selects and moves that one only
    drag_marker(&mut h, 1, -40.0, -30.0);
    let now = relight(&h);
    assert_eq!(h.app.ui.relight_light, 1);
    assert_eq!(now.lights[0], was.lights[0]);
    assert_ne!(now.lights[1].position, was.lights[1].position);
}

// Given a Directional light, dragging its marker around the photo turns it to shine from that side; a click elsewhere does not
#[test]
fn a_directional_light_is_turned_by_its_marker() {
    let mut h = relight_tool();
    exec(&mut h, "relight.addLight", json!({"kind": "directional", "angle": 0}));
    // its marker sits above the middle of the photo
    let image = h.app.image_rect.expect("the photo is on screen");
    let marker = rect_of(&h, "relightMarker:0").center();
    assert!((marker.x - image.center().x).abs() < 2.0 && marker.y < image.center().y - 20.0, "{marker:?} in {image:?}");
    pointer(&mut h, json!([{"kind": "down", "x": 0.95, "y": 0.5}, {"kind": "up", "x": 0.95, "y": 0.5}]));
    assert_eq!(relight(&h).lights[0].angle, 0.0, "a click elsewhere turns nothing");
    // dragged to the right of the middle: it shines from the right
    let up = image.center().y - marker.y;
    drag_marker(&mut h, 0, up, up);
    let l = relight(&h).lights[0];
    assert!((l.angle - 90.0).abs() < 3.0, "from the right: {}", l.angle);
    assert_eq!((l.position.x, l.position.y), (0.5, 0.5), "it has no place to move");
    // and on to below the middle: from below
    drag_marker(&mut h, 0, -up, up);
    assert!((relight(&h).lights[0].angle - 180.0).abs() < 3.0, "from below: {}", relight(&h).lights[0].angle);
}

// Given the Relight tool, Delete removes the selected light, never the photo
#[test]
fn delete_removes_the_selected_light_not_the_photo() {
    let mut h = relight_tool();
    exec(&mut h, "relight.addLight", json!({}));
    let photos = h.app.session.visible_cloned().len();
    let r = h.request("ui.key", json!({"key": "Delete"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(relight(&h).lights.is_empty());
    assert_eq!(h.app.session.visible_cloned().len(), photos);
    // with no light left it does nothing
    let r = h.request("ui.key", json!({"key": "Delete"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.session.visible_cloned().len(), photos);
}

// Given the panel, the Scene Analysis block offers the model's download while it is missing, and shows and removes
// what an analysis stored (in builds that can analyze at all)
#[test]
fn the_panel_offers_the_scene_analysis() {
    use lightcraft_engine::relief::{Relief, stored_geometry};
    let mut h = relight_tool();
    h.app.session.relief.dir = Some(std::env::temp_dir().join("lightcraft-ui-tests-no-relief-model"));
    h.settle(SETTLE);
    assert_eq!(has(&h, "button:relightModelDownload"), Relief::AVAILABLE, "the download is offered where photos can be analyzed");
    assert!(!has(&h, "button:relightAnalyze") && !has(&h, "button:relightClearGeometry"));
    // a scene stored on the photo: said so, and removable
    let id = h.app.session.active().unwrap();
    let n = 16 * 16;
    let geometry = stored_geometry(16, 16, &vec![[0.3, 0.0, -0.9]; n], &vec![2.0; n], &vec![1.0; n]).unwrap();
    h.app.session.relief_store(id, geometry).unwrap();
    h.settle(SETTLE);
    assert!(relight(&h).geometry.is_some());
    assert_eq!(has(&h, "button:relightClearGeometry"), Relief::AVAILABLE);
    if Relief::AVAILABLE {
        click(&mut h, "button:relightClearGeometry");
        assert!(relight(&h).geometry.is_none());
    }
}

// Given a light, its colour is a temperature slider, a colour picker and a hex field: what is typed there is taken
// when the edit ends, and something that is no colour changes nothing
#[test]
fn a_lights_colour_is_typed_as_hex() {
    let mut h = relight_tool();
    exec(&mut h, "relight.addLight", json!({}));
    assert!(has(&h, "slider:relightLight.temp"), "the temperature slider");
    assert!(has(&h, "relightColorPicker") && has(&h, "field:relightColor"));
    assert_eq!(relight(&h).lights[0].color, [255, 255, 255]);
    let type_color = |h: &mut Headless, text: &str| {
        click(h, "field:relightColor");
        let r = h.request("ui.text", json!({"text": text}), T);
        assert_eq!(r["ok"], true, "{r}");
        let r = h.request("ui.key", json!({"key": "Enter"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    };
    assert!(has(&h, "relightColorSource:temp") && !relight(&h).lights[0].use_color, "a new light takes its colour from its temperature");
    type_color(&mut h, "#ff8800");
    assert_eq!(relight(&h).lights[0].color, [255, 136, 0]);
    assert!(has(&h, "relightColorSource:color") && relight(&h).lights[0].use_color, "a colour that is set is the one in use");
    type_color(&mut h, "0AF");
    assert_eq!(relight(&h).lights[0].color, [0, 170, 255]);
    type_color(&mut h, "sky blue");
    assert_eq!(relight(&h).lights[0].color, [0, 170, 255], "not a colour: the light keeps its own");
    // the temperature goes through its slider's control
    exec(&mut h, "develop.set", json!({"control": "relightLight.0.temp", "value": 3000}));
    let l = relight(&h).lights[0];
    assert_eq!((l.temp, l.use_color, l.color), (3000.0, false, [0, 170, 255]), "the temperature takes over; the colour is kept, unused");
    assert!(has(&h, "relightColorSource:temp"));
    // typing the same colour again puts it back in use
    type_color(&mut h, "#00aaff");
    assert!(relight(&h).lights[0].use_color && relight(&h).lights[0].temp == 3000.0);
}
