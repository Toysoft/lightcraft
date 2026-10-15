//! Match a browser clipboard event to its earlier key without swallowing text-field input.
#[derive(Clone, Default)]
pub struct ClipboardGesture {
    pending: Vec<egui::Key>,
}
impl ClipboardGesture {
    pub fn normalize(&mut self, raw: &mut egui::RawInput, typing: bool) {
        use egui::{Event, Key};
        if typing {
            self.pending.clear();
            return;
        }
        raw.events.retain(|event| {
            match event {
                Event::WindowFocused(false) => self.pending.clear(),
                Event::Key { key, pressed, repeat, modifiers, .. } if matches!(key, Key::C | Key::X | Key::V) => {
                    if !pressed {
                        self.pending.retain(|k| k != key);
                    } else if modifiers.command {
                        if *repeat {
                            return false;
                        }
                        if !self.pending.contains(key) {
                            self.pending.push(*key);
                        }
                    }
                }
                Event::Copy | Event::Cut | Event::Paste(_) => {
                    let key = match event {
                        Event::Copy => Key::C,
                        Event::Cut => Key::X,
                        _ => Key::V,
                    };
                    if let Some(index) = self.pending.iter().position(|k| *k == key) {
                        self.pending.remove(index);
                        return false;
                    }
                }
                _ => {}
            }
            true
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::COMMAND }
    }
    #[test]
    fn one_gesture_across_frames_and_event_only_paste() {
        for (k, event) in [(egui::Key::C, egui::Event::Copy), (egui::Key::X, egui::Event::Cut), (egui::Key::V, egui::Event::Paste("text".into()))] {
            let mut gestures = ClipboardGesture::default();
            let mut raw = egui::RawInput { events: vec![key(k, true)], ..Default::default() };
            gestures.normalize(&mut raw, false);
            assert_eq!(raw.events.len(), 1);
            raw.events = vec![event.clone()];
            gestures.normalize(&mut raw, false);
            assert!(raw.events.is_empty());
            raw.events = vec![key(k, false), event.clone()];
            gestures.normalize(&mut raw, false);
            assert!(raw.events.contains(&event));
        }
    }
    #[test]
    fn focused_text_receives_payload_and_blur_clears_latch() {
        let mut gestures = ClipboardGesture::default();
        let mut raw = egui::RawInput { events: vec![key(egui::Key::V, true)], ..Default::default() };
        gestures.normalize(&mut raw, false);
        raw.events = vec![egui::Event::Paste("text".into())];
        gestures.normalize(&mut raw, true);
        assert_eq!(raw.events.len(), 1);
        raw.events = vec![egui::Event::WindowFocused(false), egui::Event::Paste("text".into())];
        gestures.normalize(&mut raw, false);
        assert_eq!(raw.events.len(), 2);
    }
}
