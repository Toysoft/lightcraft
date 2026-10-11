//! #589: saved mode, displayed palette and native inheritance are independent.
use crate::{LightcraftApp, Services, UiState, headless::Headless, state::AppearanceMode, theme};
use egui::{Theme, ThemePreference, ViewportCommand};
use serde_json::json;
use std::{cell::Cell, rc::Rc, time::Duration};

fn frame(app: &mut LightcraftApp, ctx: &egui::Context, system: Option<Theme>) -> Vec<ViewportCommand> {
    let raw = egui::RawInput {
        system_theme: system,
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| app.logic(ui.ctx()));
    output.textures_delta.clear();
    output.viewport_output.get(&egui::ViewportId::ROOT).unwrap().commands.clone()
}

#[test]
fn appearance_startup_transitions_keep_saved_system_and_native_inheritance() {
    for initial in [Some(Theme::Light), Some(Theme::Dark), None] {
        let ctx = egui::Context::default();
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        app.ui.settings.appearance_mode = AppearanceMode::System;
        let commands = frame(&mut app, &ctx, initial);
        assert_eq!(theme::current(&ctx), Some(initial.unwrap_or(Theme::Dark)));
        assert!(commands.iter().any(|c| matches!(c, ViewportCommand::SetTheme(egui::SystemTheme::SystemDefault))), "{commands:?}");
        for system in [Theme::Light, Theme::Dark, Theme::Light] {
            let commands = frame(&mut app, &ctx, Some(system));
            assert!(commands.iter().all(|c| !matches!(c, ViewportCommand::SetTheme(egui::SystemTheme::Light | egui::SystemTheme::Dark))));
            assert_eq!(ctx.options(|o| o.theme_preference), ThemePreference::System);
            assert_eq!(theme::current(&ctx), Some(system));
            let tokens = theme::Tokens::get(&ctx);
            for branch in [Theme::Light, Theme::Dark] {
                assert_eq!(ctx.style_of(branch).visuals.panel_fill, tokens.chrome);
                assert_eq!(ctx.style_of(branch).visuals.dark_mode, system == Theme::Dark);
            }
            let saved = serde_json::to_value(&app.ui).unwrap();
            assert_eq!(saved["settings"]["appearanceMode"], "system");
            let loaded: UiState = serde_json::from_value(saved).unwrap();
            assert_eq!(loaded.settings.appearance_mode, AppearanceMode::System);
        }
    }
}

#[test]
fn appearance_manual_modes_and_legacy_defaults_remain_fixed() {
    let legacy: UiState = serde_json::from_value(json!({"settings": {"filmNames": false}})).unwrap();
    assert_eq!(legacy.settings.appearance_mode, AppearanceMode::Dark);
    assert!(!legacy.settings.film_names);
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    let ctx = egui::Context::default();
    for (id, mode, expected) in [("light", AppearanceMode::Light, Theme::Light), ("dark", AppearanceMode::Dark, Theme::Dark)] {
        app.run("view.appearance", json!({"mode": id})).unwrap();
        for system in [Some(Theme::Light), Some(Theme::Dark), None] {
            frame(&mut app, &ctx, system);
            assert_eq!(theme::current(&ctx), Some(expected));
            assert_eq!(ctx.options(|o| o.theme_preference), ThemePreference::System);
            assert_eq!(app.ui.settings.appearance_mode, mode);
        }
    }
    let before = app.ui.settings.clone();
    for params in [json!({}), json!({"mode": "unknown"}), json!({"mode": false})] {
        assert!(app.run("view.appearance", params).is_err());
        assert_eq!(app.ui.settings, before);
    }
}

#[test]
fn appearance_settings_menu_and_host_service_agree() {
    let system = Rc::new(Cell::new(Some(Theme::Light)));
    let reading = system.clone();
    let services = Services { system_theme: Some(Box::new(move || reading.get())), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::new(), services);
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    let timeout = Duration::from_secs(10);
    h.app.run("app.settings", json!({})).unwrap();
    assert!(h.settle(timeout), "settle the initial settings window before using widget coordinates");
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:settingsTab-interface"}), timeout)["ok"], true);
    assert!(matches!(&h.app.ui.dialog, Some(crate::state::Dialog::Settings {tab}) if tab == "interface"));
    assert_eq!(h.app.ui.language, crate::i18n::Locale::En);
    assert!(h.settle(timeout), "settle the Interface tab before clicking its controls");
    for (index, (mode, label)) in AppearanceMode::ALL.into_iter().enumerate() {
        let result = h.request("ui.clickWidget", json!({"id": format!("button:settingsAppearance-{index}")}), timeout);
        assert_eq!(result["ok"], true, "{result}");
        h.step();
        assert_eq!(h.app.ui.settings.appearance_mode, mode);
        assert!(h.painted_text().iter().any(|text| text == label));
        let tree = crate::menubar::menu_bar(&h.app);
        let (_, view) = tree.iter().find(|(name, _)| name == "View").unwrap();
        let crate::menubar::MenuNode::Submenu { children, .. } =
            view.iter().find(|node| matches!(node, crate::menubar::MenuNode::Submenu {label, ..} if label == "Appearance")).unwrap()
        else {
            panic!("appearance submenu")
        };
        assert_eq!(children.len(), 3);
        for child in children {
            let crate::menubar::MenuNode::Item { params, checked, .. } = child else { panic!("mode item") };
            assert_eq!(*checked, Some(params["mode"] == mode.id()));
        }
    }
    crate::menubar::run_item(&mut h.app, "view.appearance", json!({"mode": "system"})).unwrap();
    h.step();
    assert_eq!(theme::current(&h.view.ctx), Some(Theme::Light));
    system.set(Some(Theme::Dark));
    h.step();
    assert_eq!(theme::current(&h.view.ctx), Some(Theme::Dark));
    system.set(Some(Theme::Light));
    h.step();
    assert_eq!(theme::current(&h.view.ctx), Some(Theme::Light));
    system.set(None);
    h.step();
    assert_eq!(theme::current(&h.view.ctx), Some(Theme::Dark), "unknown appearance has a history-independent fallback");
    assert_eq!(h.app.ui.settings.appearance_mode, AppearanceMode::System);
}

#[test]
fn appearance_palette_preserves_photo_surround_and_semantic_colours() {
    let dark = theme::Tokens::default();
    let fixed = theme::Tokens::for_theme(Theme::Dark);
    let light = theme::Tokens::for_theme(Theme::Light);
    assert_eq!(format!("{dark:?}"), format!("{fixed:?}"), "Dark keeps every original token");
    assert_eq!(light.image_canvas, dark.image_canvas);
    assert_eq!(light.mask_overlay, dark.mask_overlay);
    assert_eq!(light.top_bar_h, dark.top_bar_h);
    assert_eq!(light.film_h, dark.film_h);
    assert_ne!(light.chrome, dark.chrome);
    assert_ne!(light.canvas, dark.canvas);
    assert_ne!(light.grid_bg, dark.grid_bg);
    assert!(light.text.r() < light.canvas.r() / 2);
    assert!(light.text_label.r() < light.chrome.r() / 2);
}

#[test]
fn appearance_window_capture_copies_the_resolved_palette() {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    let ctx = egui::Context::default();
    app.ui.settings.appearance_mode = AppearanceMode::System;
    for system in [Theme::Light, Theme::Dark] {
        frame(&mut app, &ctx, Some(system));
        let image = app.headless_screenshot(&ctx, true).unwrap();
        assert_eq!(image.size, [1200, 900]);
        let shadow = &app.shadow.as_ref().unwrap().ctx;
        assert_eq!(theme::current(shadow), Some(system));
        assert_eq!(theme::Tokens::get(shadow).chrome, theme::Tokens::get(&ctx).chrome);
        assert_eq!(shadow.options(|o| o.theme_preference), ThemePreference::System);
    }
}
