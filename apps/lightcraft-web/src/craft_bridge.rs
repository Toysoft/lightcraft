//! The browser coordinator owns lifecycle, consent, atomic writes and visible storage errors.
use wasm_bindgen::prelude::*;
#[wasm_bindgen(
    inline_js = "export function cs_ready(){return globalThis.CraftSession?.ready() || false;} export function cs_take(){return globalThis.CraftSession?.take() || new Uint8Array();} export function cs_restored(ok,e){globalThis.CraftSession?.restored(ok,e);} export function cs_due(s){return globalThis.CraftSession?.shouldSave(s) || false;} export function cs_save(b){globalThis.CraftSession?.save(b);} export function cs_fail(e){globalThis.CraftSession?.failed(e);} export function cs_wake(f){globalThis.addEventListener('craft:session-wake',f);}"
)]
extern "C" {
    fn cs_ready() -> bool;
    fn cs_take() -> js_sys::Uint8Array;
    fn cs_restored(ok: bool, error: &str);
    fn cs_due(signature: &str) -> bool;
    fn cs_save(bytes: &[u8]);
    fn cs_fail(error: &str);
    fn cs_wake(callback: &js_sys::Function);
}
pub fn wake(ctx: egui::Context) {
    let callback = Closure::<dyn FnMut()>::new(move || ctx.request_repaint());
    cs_wake(callback.as_ref().unchecked_ref());
    callback.forget();
}
pub fn restore(mut apply: impl FnMut(&[u8]) -> Result<(), String>) -> bool {
    let bytes = cs_take();
    if bytes.length() > 0 {
        match apply(&bytes.to_vec()) {
            Ok(()) => cs_restored(true, ""),
            Err(e) => cs_restored(false, &e),
        }
    }
    cs_ready()
}
pub fn save(signature: Result<String, String>, capture: impl FnOnce() -> Result<Vec<u8>, String>, ctx: &egui::Context) {
    if ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    match signature {
        Ok(s) if cs_due(&s) => match capture() {
            Ok(bytes) => cs_save(&bytes),
            Err(e) => cs_fail(&e),
        },
        Err(e) => cs_fail(&e),
        _ => {}
    }
}
