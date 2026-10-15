//! IndexedDB through web-sys only. A failed/blocked request always completes with an error.
use js_sys::{Promise, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbRequest, IdbTransaction, IdbTransactionMode};

pub const LIMIT: usize = 512 * 1024 * 1024;
const DB: &str = "lightcraft-rust-sessions-v1";
const STORE: &str = "sessions";
fn error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("Browser storage error: {value:?}"))
}

async fn request(request: &IdbRequest) -> Result<JsValue, String> {
    let mut success = None;
    let mut failure = None;
    let mut timeout = None;
    let mut timer = None;
    let promise = Promise::new(&mut |resolve, reject| {
        let req = request.clone();
        success = Some(Closure::<dyn FnMut()>::new(move || {
            let _ = resolve.call1(&JsValue::NULL, &req.result().unwrap_or(JsValue::UNDEFINED));
        }));
        let reject_error = reject.clone();
        failure = Some(Closure::<dyn FnMut()>::new(move || {
            let _ = reject_error.call1(&JsValue::NULL, &"Browser storage request failed".into());
        }));
        timeout = Some(Closure::<dyn FnMut()>::new(move || {
            let _ = reject.call1(&JsValue::NULL, &"Browser storage timed out; editing remains available".into());
        }));
        if let Some(callback) = &success {
            request.set_onsuccess(Some(callback.as_ref().unchecked_ref()));
        }
        if let Some(callback) = &failure {
            request.set_onerror(Some(callback.as_ref().unchecked_ref()));
        }
        if let Some(callback) = &timeout {
            timer = web_sys::window()
                .and_then(|w| w.set_timeout_with_callback_and_timeout_and_arguments_0(callback.as_ref().unchecked_ref(), 10_000).ok());
        }
    });
    let result = JsFuture::from(promise).await.map_err(error);
    request.set_onsuccess(None);
    request.set_onerror(None);
    if let (Some(window), Some(timer)) = (web_sys::window(), timer) {
        window.clear_timeout_with_handle(timer);
    }
    result
}

pub async fn open() -> Result<IdbDatabase, String> {
    let factory = web_sys::window().ok_or("No browser window")?.indexed_db().map_err(error)?.ok_or("IndexedDB is unavailable")?;
    let req = factory.open_with_u32(DB, 1).map_err(error)?;
    let upgrade_req = req.clone();
    let upgrade = Closure::<dyn FnMut()>::new(move || {
        if let Ok(value) = upgrade_req.result()
            && let Ok(db) = value.dyn_into::<IdbDatabase>()
        {
            let _ = db.create_object_store(STORE);
        }
    });
    req.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
    let result = request(req.as_ref()).await;
    req.set_onupgradeneeded(None);
    result?.dyn_into().map_err(|_| "Invalid session database".into())
}

pub async fn read(db: &IdbDatabase, key: &str) -> Result<JsValue, String> {
    let tx = db.transaction_with_str(STORE).map_err(error)?;
    let req = tx.object_store(STORE).map_err(error)?.get(&key.into()).map_err(error)?;
    request(&req).await
}

pub async fn load(db: &IdbDatabase, id: &str) -> Result<Option<Vec<u8>>, String> {
    let value = read(db, id).await?;
    if value.is_undefined() {
        return Ok(None);
    }
    let bytes = value.dyn_into::<Uint8Array>().map_err(|_| "Invalid recovery record")?;
    if bytes.length() as usize > LIMIT {
        return Err("Recovery copy exceeds 512 MiB".into());
    }
    Ok(Some(bytes.to_vec()))
}

// Install completion handlers before issuing writes. A request's success is NOT a commit.
struct Commit {
    promise: Promise,
    tx: IdbTransaction,
    _complete: Closure<dyn FnMut()>,
    _failed: Closure<dyn FnMut()>,
    _timeout: Closure<dyn FnMut()>,
    timer: Option<i32>,
}
impl Commit {
    fn new(tx: IdbTransaction) -> Self {
        let mut resolve_fn = None;
        let mut reject_fn = None;
        let promise = Promise::new(&mut |resolve, reject| {
            resolve_fn = Some(resolve);
            reject_fn = Some(reject);
        });
        let complete = Closure::<dyn FnMut()>::new(move || {
            if let Some(resolve) = &resolve_fn {
                let _ = resolve.call0(&JsValue::NULL);
            }
        });
        let rejected = reject_fn.clone();
        let failed = Closure::<dyn FnMut()>::new(move || {
            if let Some(reject) = &rejected {
                let _ = reject.call1(&JsValue::NULL, &"Session write failed (storage quota or transaction error)".into());
            }
        });
        let timeout_tx = tx.clone();
        let timeout = Closure::<dyn FnMut()>::new(move || {
            let _ = timeout_tx.abort();
            if let Some(reject) = &reject_fn {
                let _ = reject.call1(&JsValue::NULL, &"Session write timed out".into());
            }
        });
        tx.set_oncomplete(Some(complete.as_ref().unchecked_ref()));
        tx.set_onabort(Some(failed.as_ref().unchecked_ref()));
        tx.set_onerror(Some(failed.as_ref().unchecked_ref()));
        let timer =
            web_sys::window().and_then(|w| w.set_timeout_with_callback_and_timeout_and_arguments_0(timeout.as_ref().unchecked_ref(), 10_000).ok());
        Self { promise, tx, _complete: complete, _failed: failed, _timeout: timeout, timer }
    }
    async fn finish(self) -> Result<(), String> {
        JsFuture::from(self.promise.clone()).await.map(|_| ()).map_err(error)
    }
}
impl Drop for Commit {
    fn drop(&mut self) {
        self.tx.set_oncomplete(None);
        self.tx.set_onabort(None);
        self.tx.set_onerror(None);
        if let (Some(w), Some(timer)) = (web_sys::window(), self.timer) {
            w.clear_timeout_with_handle(timer);
        }
    }
}

pub async fn write(db: &IdbDatabase, id: &str, bytes: &[u8], previous: Option<&[u8]>) -> Result<(), String> {
    if bytes.len() > LIMIT {
        return Err("Session exceeds 512 MiB; save documents separately".into());
    }
    let tx = db.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite).map_err(error)?;
    let commit = Commit::new(tx.clone());
    let result = (|| {
        let store = tx.object_store(STORE).map_err(error)?;
        store.put_with_key(&Uint8Array::from(bytes), &id.into()).map_err(error)?;
        if let Some(previous) = previous {
            store.put_with_key(&Uint8Array::from(previous), &format!("{id}:previous").into()).map_err(error)?;
        }
        store.put_with_key(&id.into(), &"latest".into()).map_err(error)?;
        Ok::<(), String>(())
    })();
    if result.is_err() {
        let _ = tx.abort();
    }
    let committed = commit.finish().await;
    result.and(committed)
}
