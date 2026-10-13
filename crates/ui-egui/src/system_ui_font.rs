//! Desktop system UI face for chrome text. Bundled Inter stays as a fallback.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const MAX_FACE: u64 = 16 << 20;

/// Put the platform UI sans first in every proportional family. No-op on the web, or when no
/// system face can be read.
pub fn prepend(fonts: &mut FontDefinitions) {
    #[cfg(not(target_arch = "wasm32"))]
    prepend_native(fonts);
    #[cfg(target_arch = "wasm32")]
    let _ = fonts;
}

#[cfg(not(target_arch = "wasm32"))]
fn prepend_native(fonts: &mut FontDefinitions) {
    let Some(regular) = first_face(&["sans-serif", "system-ui"]) else { return };
    fonts.font_data.insert("system-ui".into(), Arc::new(face(regular)));
    if let Some(bytes) = first_face(&["sans-serif:weight=600", "sans-serif:weight=700"]) {
        fonts.font_data.insert("system-ui-semibold".into(), Arc::new(face(bytes)));
    }
    if let Some(bytes) = first_face(&["sans-serif:weight=500"]) {
        fonts.font_data.insert("system-ui-medium".into(), Arc::new(face(bytes)));
    }
    let families: Vec<FontFamily> = fonts.families.keys().cloned().collect();
    for family in families {
        if family == FontFamily::Monospace {
            continue;
        }
        let primary = match &family {
            FontFamily::Name(n) if n.contains("semi") || n.contains("bold") => {
                if fonts.font_data.contains_key("system-ui-semibold") { "system-ui-semibold" } else { "system-ui" }
            }
            FontFamily::Name(n) if n.contains("medium") => {
                if fonts.font_data.contains_key("system-ui-medium") { "system-ui-medium" } else { "system-ui" }
            }
            _ => "system-ui",
        };
        fonts.families.entry(family).or_default().insert(0, primary.to_owned());
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn face(bytes: Vec<u8>) -> FontData {
    let mut data = FontData::from_owned(bytes);
    // Segoe UI / system sans run larger than Inter at the same point size.
    data.tweak.scale = 0.9;
    data
}

#[cfg(not(target_arch = "wasm32"))]
fn first_face(queries: &[&str]) -> Option<Vec<u8>> {
    for q in queries {
        if let Some(bytes) = fc_match(q) {
            return Some(bytes);
        }
    }
    platform_ui_file()
}

#[cfg(not(target_arch = "wasm32"))]
fn fc_match(query: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", query]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout);
    let path = path.trim();
    read_face(std::path::Path::new(path))
}

#[cfg(not(target_arch = "wasm32"))]
fn platform_ui_file() -> Option<Vec<u8>> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/SFNSText.ttf", "/System/Library/Fonts/Helvetica.ttc"]
    } else if cfg!(windows) {
        &[r"C:\Windows\Fonts\segoeui.ttf"]
    } else {
        &["/usr/share/fonts/adwaita-sans/AdwaitaSans-Regular.ttf", "/usr/share/fonts/cantarell/Cantarell-VF.otf", "/usr/share/fonts/liberation/LiberationSans-Regular.ttf"]
    };
    candidates.iter().find_map(|p| read_face(std::path::Path::new(p)))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_face(path: &std::path::Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FACE {
        return None;
    }
    std::fs::read(path).ok()
}
