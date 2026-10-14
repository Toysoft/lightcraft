//! The banner under the top bar while the library holds the sample photos: it says what they
//! are, and offers the next step. With only samples: Import Photos… or Remove Samples. Once photos
//! of the person's own are in: Remove Samples or Keep Them. Closed for good with Hide / Keep Them
//! (until samples are added again).

use egui::RichText;
use lightcraft_catalog::Source;
use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

enum Click {
    Import,
    Remove,
    Hide,
}

/// The library holds photos of the person's own beside the samples (stops at the first one:
/// samples are a couple of dozen).
fn own_photos(app: &LightcraftApp) -> bool {
    app.session.catalog.photos().any(|p| p.in_library() && !matches!(p.source, Source::Demo { .. }))
}

/// What the banner says now, or `None` while it isn't shown: hidden, no sample in the library
/// (all in Recently Deleted: the grid says the library is empty), or an in-memory demo session (a
/// demo by design). On a sample open in Detail before the first change since the samples came,
/// it invites that change, with the key Compare Before and After is bound to.
pub fn message(app: &LightcraftApp, mac: bool) -> Option<String> {
    if app.ui.samples_banner_hidden || app.session.catalog.sample_photos_in_library() == 0 || app.session.library.is_none() {
        return None;
    }
    if own_photos(app) {
        return Some(crate::i18n::tr("Your photos are in. The sample photos are still here too.").to_string());
    }
    let on_a_sample = app.ui.view == crate::state::ViewMode::Detail
        && app.session.active().and_then(|id| app.session.catalog.photo(id)).is_some_and(|p| matches!(p.source, Source::Demo { .. }));
    if on_a_sample && app.samples_landed == Some(app.session.commits()) {
        let first = crate::i18n::tr("Drag any slider: the original never changes.");
        return Some(match crate::shortcuts::shortcut_of(&app.ui.settings.keymap, "view.beforeAfter") {
            Some(sc) => {
                let key = crate::menubar::shortcut_text(sc, mac);
                format!("{first} {}", crate::i18n::tr_format!("Press {key} to compare before and after.", key = key))
            }
            None => first.to_string(),
        });
    }
    Some(crate::i18n::tr("These are sample photos, to try LightCraft on. Import your own whenever you're ready.").to_string())
}

pub fn banner(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    // cached by the photo set, not by every edit (see `Session::has_samples`)
    let present = app.session.has_samples();
    // samples that newly appear bring the banner back, however they were added
    if present && app.samples_present == Some(false) {
        app.ui.samples_banner_hidden = false;
    }
    app.samples_present = Some(present);
    let Some(text) = message(app, ui.ctx().os() == egui::os::OperatingSystem::Mac) else { return };
    let own = own_photos(app);
    let t = Tokens::get(ui.ctx());
    let mut click = None;
    let resp = egui::Panel::top("samples-banner")
        .exact_size(34.0)
        .frame(egui::Frame::NONE.fill(t.inset).inner_margin(egui::Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            // the buttons first, from the right edge, so a long translation shortens the message
            // (whole on hover) instead of pushing them out of the window
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, widget) = if own { ("Keep Them", "button:samplesKeep") } else { ("Hide", "button:samplesHide") };
                let b = ui.add(egui::Button::new(crate::i18n::tr(label)).frame(false));
                register(ui.ctx(), widget, b.rect);
                if b.clicked() {
                    click = Some(Click::Hide);
                }
                let b = ui.button(crate::i18n::tr("Remove Sample Photos"));
                register(ui.ctx(), "button:samplesRemove", b.rect);
                if b.clicked() {
                    click = Some(Click::Remove);
                }
                if !own {
                    let b = ui.add(egui::Button::new(RichText::new(crate::i18n::tr("Import Photos…")).color(egui::Color32::WHITE)).fill(t.accent));
                    register(ui.ctx(), "button:samplesImport", b.rect);
                    if b.clicked() {
                        click = Some(Click::Import);
                    }
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(text).font(t.font(12.5)).color(t.text)).truncate());
                });
            });
        });
    register(ui.ctx(), "banner:samples", resp.response.rect);
    match click {
        Some(Click::Import) => {
            if let Err(e) = app.run("file.addPhotos", json!({})) {
                app.toast_error(ui.ctx(), e);
            }
        }
        Some(Click::Remove) => match app.run("library.removeSamples", json!({})) {
            Ok(r) => {
                let n = r["removed"].as_u64().unwrap_or(0);
                let text = crate::i18n::tr_format!("Removed {n} sample photos. Edit → Undo brings them back.", n = n);
                app.toast(ui.ctx(), text);
            }
            Err(e) => app.toast_error(ui.ctx(), e),
        },
        Some(Click::Hide) => app.ui.samples_banner_hidden = true,
        None => {}
    }
}
