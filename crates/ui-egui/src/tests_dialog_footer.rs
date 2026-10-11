//! Dialogs that only show information close with one button (#782): System Info and What's New
//! must not offer a Cancel that duplicates OK, the way About shows a single Close.

use std::time::Duration;

use lightcraft_engine::Session;
use serde_json::json;

use crate::{LightcraftApp, Services, headless::Headless};

const T: Duration = Duration::from_secs(5);

#[test]
fn information_dialogs_offer_one_close_button() {
    let dir = std::env::temp_dir().join(format!("lc-dialog-footer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("library"), true).unwrap();
    let mut h = Headless::new(LightcraftApp::new(s, Services::default()), [1600.0, 1000.0], 1.0);
    for command in ["app.systemInfo", "app.whatsNew"] {
        let r = h.request("ui.menu.invoke", json!({"id": command}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
        h.step();
        h.step();
        assert!(h.app.ui.dialog.is_some(), "{command} opens its dialog");
        let r = h.request("ui.clickWidget", json!({"id": "button:dialogCancel"}), T);
        assert_eq!(r["ok"], false, "{command} still offers Cancel: {r}");
        let r = h.request("ui.clickWidget", json!({"id": "button:dialogOk"}), T);
        assert_eq!(r["ok"], true, "{command} has no Close: {r}");
        h.step();
        assert_eq!(h.app.ui.dialog, None, "{command} closes from its one button");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
