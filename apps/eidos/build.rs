//! Windows only: embed the app icon and version info (VERSIONINFO) into `eidos.exe`, so
//! Explorer, the taskbar, the Start menu and Alt-Tab show the lynx.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `EIDOS_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/eidos.ico");
    println!("cargo:rerun-if-env-changed=EIDOS_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/eidos.ico")
        .set("ProductName", "Eidos")
        .set("FileDescription", "Eidos photo library and raw developer")
        .set("LegalCopyright", "Copyright (c) 2026 ArtCraft Team and contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "eidos.exe")
        .set("InternalName", "eidos");
    if let Err(e) = res.compile() {
        if std::env::var_os("EIDOS_REQUIRE_WINRES").is_some() {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=eidos.exe built without icon/version resources: {e}");
    }
}
