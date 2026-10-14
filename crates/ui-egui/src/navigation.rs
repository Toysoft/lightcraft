//! Configurable photo wheel zoom and pointer panning.
use egui::{Pos2, Rect};

pub fn wheel(ctx: &egui::Context, area: Rect, binding: crate::state::WheelModifier) -> Option<(Pos2, f32)> {
    ctx.input(|i| {
        let q = i.pointer.hover_pos().filter(|q| area.contains(*q))?;
        let delta: f32 = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, unit, modifiers, .. } if binding.matches(*modifiers) => {
                    let scale = match unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => 40.0,
                        egui::MouseWheelUnit::Page => 300.0,
                    };
                    Some(delta.y * scale)
                }
                _ => None,
            })
            .sum();
        (delta.is_finite() && delta != 0.0).then_some((q, (delta / 300.0).clamp(-1.0, 1.0).exp()))
    })
}

/// Navigation takes precedence over editing only for an explicit navigation gesture.
pub fn pan_requested(ctx: &egui::Context, resp: &egui::Response, settings: &crate::state::NavigationSettings, tool: bool) -> bool {
    // Native touch gestures also emit pointer drags; let pinch/translation handle them.
    if ctx.input(|i| i.multi_touch().is_some()) {
        return false;
    }
    let button = settings.pan_button.button();
    let held = ctx.input(|i| settings.pan_modifier.matches(i.modifiers));
    if resp.dragged_by(egui::PointerButton::Middle) || resp.drag_started_by(egui::PointerButton::Middle) {
        return true;
    }
    let gesture = resp.dragged_by(button) || resp.drag_started_by(button);
    gesture && (!tool || button == egui::PointerButton::Middle || held)
}
