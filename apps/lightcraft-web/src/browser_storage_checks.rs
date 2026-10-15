#![cfg(target_arch = "wasm32")]
//! Browser-only regression probe. Build with --features browser-storage-tests.
//! Call run_checks() from the generated wasm-bindgen module on a disposable test origin.
use super::browser_store;
use wasm_bindgen::prelude::*;
#[wasm_bindgen]
pub async fn run_checks() -> Result<String, JsValue> {
    async fn checks() -> Result<String, String> {
        let db = browser_store::open().await?;
        let id = format!("browser-test-{}", js_sys::Date::now());
        let other = format!("{id}-other");
        if browser_store::load(&db, &id).await?.is_some() {
            return Err("new id unexpectedly exists".into());
        }
        browser_store::write(&db, &id, b"first", None).await?;
        if browser_store::load(&db, &id).await?.as_deref() != Some(b"first") {
            return Err("round trip failed".into());
        }
        browser_store::write(&db, &id, b"second", Some(b"first")).await?;
        if browser_store::load(&db, &format!("{id}:previous")).await?.as_deref() != Some(b"first") {
            return Err("previous snapshot failed".into());
        }
        browser_store::write(&db, &other, b"other-tab", None).await?;
        if browser_store::load(&db, &id).await?.as_deref() != Some(b"second") {
            return Err("independent writer was overwritten".into());
        }
        if browser_store::read(&db, "latest").await?.as_string().as_deref() != Some(other.as_str()) {
            return Err("latest pointer failed".into());
        }
        db.close();
        if browser_store::write(&db, &id, b"lost", None).await.is_ok() {
            return Err("closed database unexpectedly wrote".into());
        }
        let reopened = browser_store::open().await?;
        if browser_store::load(&reopened, &id).await?.as_deref() != Some(b"second") {
            return Err("failed write damaged committed copy".into());
        }
        reopened.close();
        Ok("PASS: missing record, binary round trip, previous snapshot, independent writers, latest pointer, closed-database failure, retained committed copy".into())
    }
    checks().await.map_err(|e| e.into())
}
