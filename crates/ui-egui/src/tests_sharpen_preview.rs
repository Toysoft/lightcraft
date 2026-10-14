//! Option-drag on the sharpening sliders: while the Masking slider is dragged with
//! Option held the loupe shows the sharpening mask, Amount shows the photo in grayscale; releasing
//! Option or ending the drag restores the photo; the develop settings only ever see the slider value.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton};
use lightcraft_pipeline::Overlay;
use serde_json::json;

use crate::headless::Headless;
use crate::panels::detail::view_overlay;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn detail_with_open_detail_section() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "openSections": ["detail"]}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"control": "detail.sharpenAmount", "value": 40}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    h
}

fn slider(h: &Headless, id: &str) -> egui::Rect {
    let key = format!("slider:{id}");
    h.app.widgets.iter().find(|(w, _)| *w == key).map(|(_, r)| *r).unwrap_or_else(|| panic!("{key} not on screen"))
}

fn overlay(h: &Headless) -> Overlay {
    let id = h.app.session.active().expect("active photo");
    view_overlay(&h.app, &h.app.session.develop_of(id).unwrap_or_default())
}

fn push(h: &mut Headless, e: Event) {
    h.app.synthetic.push(e);
    h.step();
}

#[test]
fn option_drag_on_masking_shows_the_mask_until_option_or_the_drag_ends() {
    let mut h = detail_with_open_detail_section();
    let before = {
        let id = h.app.session.active().unwrap();
        (*h.app.session.develop_of(id).unwrap()).clone()
    };
    let r = slider(&h, "detail.sharpenMasking");
    let (y, x0) = (r.center().y, r.left() + 4.0);
    // press with Option held and drag: the mask
    push(&mut h, Event::PointerMoved(egui::pos2(x0, y)));
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0, y), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT });
    for dx in [8.0, 16.0, 24.0] {
        push(&mut h, Event::PointerMoved(egui::pos2(x0 + dx, y)));
    }
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some("detail.sharpenMasking"));
    assert_eq!(overlay(&h), Overlay::SharpenMask, "Option-drag on Masking shows the mask");
    // Option released mid-drag: the photo at once
    push(&mut h, Event::ModifiersChanged(Modifiers::NONE));
    push(&mut h, Event::PointerMoved(egui::pos2(x0 + 30.0, y)));
    assert_eq!(overlay(&h), Overlay::None, "releasing Option restores the photo");
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0 + 30.0, y), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.app.ui.dragging_control.is_none() && h.app.ui.option_preview.is_none());
    assert_eq!(overlay(&h), Overlay::None);
    // the drag changed Masking and nothing else
    let after = {
        let id = h.app.session.active().unwrap();
        (*h.app.session.develop_of(id).unwrap()).clone()
    };
    assert!(after.detail.sharpen_masking > before.detail.sharpen_masking, "{} -> {}", before.detail.sharpen_masking, after.detail.sharpen_masking);
    let mut same = after.clone();
    same.detail.sharpen_masking = before.detail.sharpen_masking;
    assert_eq!(same, before, "only Masking changed");
}

/// The loupe really shows the Masking preview while Option-dragging (a grey mask, not the colour
/// photo), and the photo again after: the overlay reaches the render and its texture.
#[test]
fn the_masking_preview_reaches_the_loupe() {
    let mut h = detail_with_open_detail_section();
    h.app.renderer.keep_pixels = true;
    h.settle(T);
    let main = |h: &Headless| h.app.renderer.textures.get(&crate::render::Slot::Main).and_then(|t| t.pixels.clone());
    // (colour: some pixel whose channels differ)
    let grey = |img: &egui::ColorImage| img.pixels.iter().all(|p| p.r() == p.g() && p.g() == p.b());
    assert!(main(&h).is_some_and(|img| !grey(&img)), "the colour photo first");
    let r = slider(&h, "detail.sharpenMasking");
    let (y, x0) = (r.center().y, r.left() + 4.0);
    push(&mut h, Event::PointerMoved(egui::pos2(x0, y)));
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0, y), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT });
    for dx in [8.0, 16.0, 24.0] {
        push(&mut h, Event::PointerMoved(egui::pos2(x0 + dx, y)));
    }
    assert_eq!(overlay(&h), Overlay::SharpenMask);
    // (the button stays down while the mask renders)
    let shown = h.step_until(T, |h| main(h).is_some_and(|img| grey(&img)));
    assert!(shown, "the mask on the loupe (overlay {:?})", overlay(&h));
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0 + 24.0, y), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::ALT });
    assert!(h.step_until(T, |h| main(h).is_some_and(|img| !grey(&img))), "the photo again");
}

#[test]
fn option_drag_on_amount_shows_grayscale_and_a_plain_drag_shows_nothing() {
    let mut h = detail_with_open_detail_section();
    let r = slider(&h, "detail.sharpenAmount");
    let (y, x0) = (r.center().y, r.center().x);
    // a plain drag: no diagnostic view
    push(&mut h, Event::PointerMoved(egui::pos2(x0, y)));
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0, y), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    push(&mut h, Event::PointerMoved(egui::pos2(x0 + 10.0, y)));
    assert_eq!(overlay(&h), Overlay::None);
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0 + 10.0, y), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    // with Option: grayscale while it lasts
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0 + 10.0, y), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT });
    push(&mut h, Event::PointerMoved(egui::pos2(x0 + 20.0, y)));
    push(&mut h, Event::PointerMoved(egui::pos2(x0 + 26.0, y)));
    assert_eq!(overlay(&h), Overlay::Luminance);
    push(&mut h, Event::PointerButton { pos: egui::pos2(x0 + 26.0, y), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::ALT });
    h.step();
    h.step();
    assert_eq!(overlay(&h), Overlay::None, "the drag ended");
}

/// Option-dragging Radius or Detail shows the sharpening band in grey (the edges those sliders
/// pick) until the drag ends.
#[test]
fn option_drag_on_radius_or_detail_shows_the_sharpening_detail() {
    for id in ["detail.sharpenRadius", "detail.sharpenDetail"] {
        let mut h = detail_with_open_detail_section();
        let r = slider(&h, id);
        let (y, x0) = (r.center().y, r.center().x);
        push(&mut h, Event::PointerMoved(egui::pos2(x0, y)));
        push(&mut h, Event::PointerButton { pos: egui::pos2(x0, y), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT });
        push(&mut h, Event::PointerMoved(egui::pos2(x0 + 8.0, y)));
        push(&mut h, Event::PointerMoved(egui::pos2(x0 + 14.0, y)));
        assert_eq!(overlay(&h), Overlay::SharpenDetail, "{id}");
        push(
            &mut h,
            Event::PointerButton { pos: egui::pos2(x0 + 14.0, y), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::ALT },
        );
        h.step();
        h.step();
        assert_eq!(overlay(&h), Overlay::None, "{id}: the drag ended");
    }
}

/// A drag on the photo (any slider or wheel) leaves its filmstrip thumbnail alone until the drag
/// ends: one thumbnail render after it instead of one per tick (each decoding the raw again, as
/// superseded thumbnail renders threw their decoded source away).
#[test]
fn a_drag_refreshes_the_photos_thumbnail_once_when_it_ends() {
    let mut h = detail_with_open_detail_section();
    h.settle(Duration::from_secs(120));
    let built = h.app.renderer.thumb_jobs_built;
    let r = h.request("engine.execute", json!({"command": "develop.beginInteraction", "params": {"label": "test"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    for v in [10, 20, 30, 40, 50] {
        let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": v as f64 / 50.0}}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
    }
    assert_eq!(h.app.renderer.thumb_jobs_built, built, "no thumbnail render while dragging");
    let r = h.request("engine.execute", json!({"command": "develop.endInteraction", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.renderer.thumb_jobs_built > built, "the thumbnail catches up when the drag ends");
}
