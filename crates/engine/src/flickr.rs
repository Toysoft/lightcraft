//! Flickr: sign in once, then upload chosen photos (rendered with their edits, like an export)
//! to the account. The protocol is `lightcraft-flickr`; this module keeps the account and turns
//! catalog photos into uploads.
//!
//! - **The account** (API key, signed-in token) lives in the per-user config folder
//!   (`<config>/flickr.json`, [`Session::with_default_flickr`]), not in a library: it belongs to
//!   the person at this computer. A session without that folder (tests, `--memory`) keeps it in
//!   memory only.
//! - **The API key**: Flickr gives one to every app (<https://www.flickr.com/services/apps/create/noncommercial/>).
//!   A build may carry one (`LIGHTCRAFT_FLICKR_API_KEY` and `LIGHTCRAFT_FLICKR_API_SECRET` set when
//!   compiling); otherwise, or to use their own, the user enters theirs in Settings → Flickr.
//! - **Uploads** ([`prepare_upload`] on the session, then [`UploadJob::run`] anywhere) render each
//!   photo as a JPEG through the export pipeline, then send it with its title, caption and
//!   keywords. Location is left out unless asked for: a showcase upload is usually public.
//!
//! Commands: `flickr.status`, `flickr.setApiKey`, `flickr.signIn`, `flickr.completeSignIn`,
//! `flickr.signOut`, `flickr.upload` (see `cmd/flickr.rs`). They run synchronously, which suits
//! the CLI and MCP; the desktop app runs the network steps on a worker thread instead.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use lightcraft_catalog::PhotoId;
use lightcraft_flickr::account::{self, Account};
pub use lightcraft_flickr::{Album, Endpoints, RequestToken, Token};
use lightcraft_flickr::{ApiKey, Client, PhotoInfo, Privacy};
use serde_json::{Value, json};

use crate::Session;
use crate::export::{ExportOptions, PreparedExport, Resize, SharpenFor};

/// Where to get an API key.
pub const API_KEY_PAGE: &str = "https://www.flickr.com/services/apps/create/noncommercial/";

/// The API key built into this binary, if the build had one.
fn builtin_key() -> Option<ApiKey> {
    let k = ApiKey {
        key: option_env!("LIGHTCRAFT_FLICKR_API_KEY").unwrap_or("").to_string(),
        secret: option_env!("LIGHTCRAFT_FLICKR_API_SECRET").unwrap_or("").to_string(),
    };
    k.is_set().then_some(k)
}

/// The session's Flickr account and a sign-in in progress.
#[derive(Default)]
pub struct State {
    /// The account file (`None`: kept in memory only).
    pub(crate) path: Option<PathBuf>,
    pub(crate) account: Account,
    /// Sign-in step 1 done: waiting for the code Flickr shows.
    pub(crate) pending: Option<RequestToken>,
    /// Flickr's addresses (a local stand-in in tests).
    pub endpoints: Endpoints,
    /// Why the account file could not be read (it is left alone; the session starts signed out).
    pub(crate) problem: Option<String>,
    /// Uploads in this session: the most recent results (`flickr.status` → `lastUpload`).
    pub(crate) last_upload: Option<Value>,
}

impl Session {
    /// Keep the Flickr account in the per-user config folder ([`crate::config::config_dir`]).
    pub fn with_default_flickr(mut self) -> Self {
        if let Some(dir) = crate::config::config_dir() {
            self.set_flickr_account_file(dir.join("flickr.json"));
        }
        self
    }

    /// Read the Flickr account from `path` and keep it there from now on. A file that can't be
    /// read is left alone and reported by `flickr.status` (`problem`); the session starts signed out.
    pub fn set_flickr_account_file(&mut self, path: PathBuf) {
        match account::load(&path) {
            Ok(a) => {
                self.flickr.account = a;
                self.flickr.problem = None;
            }
            Err(e) => {
                log::warn!("flickr: {e}");
                self.flickr.account = Account::default();
                self.flickr.problem = Some(e);
            }
        }
        self.flickr.path = Some(path);
    }

    fn save_flickr(&mut self) -> Result<(), String> {
        let Some(path) = &self.flickr.path else { return Ok(()) };
        account::save(path, &self.flickr.account)?;
        self.flickr.problem = None;
        Ok(())
    }

    /// A client for the API key in use: the user's own, else the build's.
    pub fn flickr_client(&self) -> Result<Client, String> {
        let key = self.flickr.account.own_key().or_else(builtin_key).ok_or("enter a Flickr API key and its secret first (Settings → Flickr)")?;
        let mut c = Client::new(key);
        c.endpoints = self.flickr.endpoints.clone();
        Ok(c)
    }

    /// The signed-in account, if any.
    pub fn flickr_token(&self) -> Option<&Token> {
        self.flickr.account.token.as_ref()
    }

    /// `flickr.status`.
    pub fn flickr_status(&self) -> Value {
        let a = &self.flickr.account;
        let t = a.token.as_ref();
        json!({
            "available": true,
            "ownKey": a.own_key().is_some(),
            "builtinKey": builtin_key().is_some(),
            "apiKey": a.api_key,
            "hasKey": a.own_key().is_some() || builtin_key().is_some(),
            "signedIn": t.is_some(),
            "username": t.map(|t| t.username.as_str()),
            "fullname": t.map(|t| t.fullname.as_str()),
            "userNsid": t.map(|t| t.user_nsid.as_str()),
            "pending": self.flickr.pending.is_some(),
            "saved": self.flickr.path.is_some(),
            "problem": self.flickr.problem,
            "apiKeyPage": API_KEY_PAGE,
            "lastUpload": self.flickr.last_upload,
        })
    }

    /// Use the API key `key` with `secret` (both empty: back to the build's key). A different key
    /// signs out: a sign-in belongs to the key it was made with.
    pub fn flickr_set_api_key(&mut self, key: &str, secret: &str) -> Result<(), String> {
        let (key, secret) = (key.trim(), secret.trim());
        if key.is_empty() != secret.is_empty() {
            return Err("enter both the API key and its secret".into());
        }
        if [key, secret].iter().any(|s| s.len() > 256 || s.chars().any(|c| c.is_whitespace() || c.is_control())) {
            return Err("an API key and its secret are short codes without spaces".into());
        }
        let a = &mut self.flickr.account;
        if a.api_key != key || a.api_secret != secret {
            a.api_key = key.to_string();
            a.api_secret = secret.to_string();
            a.token = None;
            self.flickr.pending = None;
        }
        self.save_flickr()
    }

    /// Sign-in step 1 is done: remember its temporary token until the code arrives.
    pub fn flickr_begin_sign_in(&mut self, request: RequestToken) {
        self.flickr.pending = Some(request);
    }

    /// The temporary token of the sign-in in progress.
    pub fn flickr_pending(&self) -> Option<&RequestToken> {
        self.flickr.pending.as_ref()
    }

    /// Sign-in finished: keep the account's token (saved at once).
    pub fn flickr_finish_sign_in(&mut self, token: Token) -> Result<(), String> {
        self.flickr.pending = None;
        self.flickr.account.token = Some(token);
        self.save_flickr()
    }

    /// Drop a sign-in in progress (nothing on disk changes).
    pub fn flickr_cancel_sign_in(&mut self) {
        self.flickr.pending = None;
    }

    /// Forget the sign-in (the API key stays).
    pub fn flickr_sign_out(&mut self) -> Result<(), String> {
        self.flickr.pending = None;
        self.flickr.account.token = None;
        self.save_flickr()
    }

    /// Remember the results of an upload (`flickr.status` → `lastUpload`); a refused sign-in
    /// signs out, so the next upload asks for a new one.
    pub fn flickr_record_upload(&mut self, result: &Value) {
        if result["signInNeeded"] == true {
            let _ = self.flickr_sign_out();
        }
        self.flickr.last_upload = Some(result.clone());
    }
}

/// How photos are uploaded (`flickr.upload` params).
#[derive(Clone, Debug, PartialEq)]
pub struct UploadOptions {
    /// Long edge in pixels; `None`: full size.
    pub long_edge: Option<u32>,
    /// JPEG quality 1–100.
    pub quality: u8,
    pub privacy: Privacy,
    pub hidden: bool,
    /// The photo's keywords as Flickr tags.
    pub tags: bool,
    /// The photo's caption as the description.
    pub description: bool,
    /// Leave GPS out of the file (Flickr would put the photo on its map).
    pub remove_location: bool,
    /// The id of the account's album to put the photos in (`None`: no album).
    pub album: Option<String>,
}

impl Default for UploadOptions {
    fn default() -> Self {
        UploadOptions {
            long_edge: Some(2048),
            quality: 90,
            privacy: Privacy::Public,
            hidden: false,
            tags: true,
            description: true,
            remove_location: true,
            album: None,
        }
    }
}

impl UploadOptions {
    pub fn from_json(p: &Value) -> Result<UploadOptions, String> {
        let mut o = UploadOptions::default();
        if let Some(v) = p.get("longEdge") {
            let n = v.as_u64().ok_or("`longEdge` must be a number of pixels (0 = full size)")?;
            o.long_edge = match n {
                0 => None,
                64..=20000 => Some(n as u32),
                _ => return Err("`longEdge` must be 0 (full size) or 64–20000 pixels".into()),
            };
        }
        if let Some(v) = p.get("quality") {
            o.quality = v.as_u64().filter(|q| (1..=100).contains(q)).ok_or("`quality` must be 1–100")? as u8;
        }
        if let Some(v) = p.get("privacy") {
            o.privacy = v.as_str().and_then(Privacy::parse).ok_or("`privacy` must be public, friends, family, friendsAndFamily or private")?;
        }
        let flag = |k: &str, d: bool| -> Result<bool, String> {
            match p.get(k) {
                None => Ok(d),
                Some(v) => v.as_bool().ok_or_else(|| format!("`{k}` must be true or false")),
            }
        };
        o.hidden = flag("hidden", o.hidden)?;
        o.tags = flag("tags", o.tags)?;
        o.description = flag("description", o.description)?;
        o.remove_location = flag("removeLocation", o.remove_location)?;
        o.album = match p.get("album") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.trim().is_empty() => None,
            Some(Value::String(s)) if s.len() <= 64 && s.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                Some(s.trim().to_string())
            }
            Some(_) => return Err("`album` must be an album id from flickr.albums (or null: no album)".into()),
        };
        Ok(o)
    }

    /// The export settings the photos are rendered with: JPEG, sRGB, sharpened for screen.
    pub fn export_options(&self) -> ExportOptions {
        ExportOptions {
            quality: self.quality,
            resize: self.long_edge.map(Resize::long_edge),
            sharpen: SharpenFor::Screen,
            remove_location: self.remove_location,
            ..ExportOptions::default()
        }
    }
}

/// One photo of an upload: how it renders and what Flickr is told about it.
struct Item {
    export: PreparedExport,
    info: PhotoInfo,
}

/// An upload set up from the session ([`prepare_upload`]); [`UploadJob::run`] needs no session,
/// so it can run on a worker thread.
pub struct UploadJob {
    items: Vec<Item>,
    client: Client,
    token: Token,
    album: Option<String>,
}

/// Set up the upload of the photos `p` names (`ids`; default: the selection) with the options in
/// `p`. Fails when nothing is signed in or no photo can be rendered.
pub fn prepare_upload(s: &mut Session, p: &Value) -> Result<UploadJob, String> {
    let opts = UploadOptions::from_json(p)?;
    let client = s.flickr_client()?;
    let token = s.flickr_token().cloned().ok_or("sign in to Flickr first (Settings → Flickr)")?;
    let ids: Vec<PhotoId> = s.targets(p);
    if ids.is_empty() {
        return Err("no photos selected".into());
    }
    if ids.len() > MAX_PHOTOS {
        return Err(format!("choose at most {MAX_PHOTOS} photos to upload at once"));
    }
    let eo = opts.export_options();
    let infos: Vec<PhotoInfo> =
        ids.iter().map(|id| s.catalog.photo(*id).map(|ph| photo_info(ph, &opts)).ok_or(format!("no photo {}", id.0))).collect::<Result<_, _>>()?;
    let exports = crate::export::prepare_batch(s, &ids, &eo)?;
    let items = exports.into_iter().zip(infos).map(|(export, info)| Item { export, info }).collect();
    Ok(UploadJob { items, client, token, album: opts.album })
}

/// What [`list_albums`] needs, taken from the session: the client and the signed-in account.
pub fn albums_request(s: &Session) -> Result<(Client, Token), String> {
    let client = s.flickr_client()?;
    let token = s.flickr_token().cloned().ok_or("sign in to Flickr first (Settings → Flickr)")?;
    Ok((client, token))
}

/// The signed-in account's albums (runs without the session: a worker thread may call it).
pub fn list_albums(client: &Client, token: &Token, cancel: &AtomicBool) -> Result<Vec<Album>, lightcraft_flickr::Error> {
    client.list_albums(token, cancel)
}

/// Most photos one upload takes: it is for chosen photos, not a whole library.
pub const MAX_PHOTOS: usize = 500;

/// What Flickr is told about photo `p`: its title (else its file name without the extension),
/// caption and keywords as the options say.
pub fn photo_info(p: &lightcraft_catalog::Photo, o: &UploadOptions) -> PhotoInfo {
    let title = if p.meta.title.trim().is_empty() {
        std::path::Path::new(&p.file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        p.meta.title.trim().to_string()
    };
    PhotoInfo {
        title,
        description: if o.description { p.meta.caption.trim().to_string() } else { String::new() },
        tags: if o.tags { p.meta.keywords.clone() } else { Vec::new() },
        privacy: o.privacy,
        hidden: o.hidden,
    }
}

impl UploadJob {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Render and upload each photo in order, then put it in the album, if one was chosen.
    /// `progress(done, file)` is called before each one; it, or `cancel`, stops the rest. A refused
    /// sign-in stops the rest too (they would all fail). A photo that went up but couldn't join
    /// the album counts as uploaded, with `albumError`. → `{uploaded, failed, albumFailed,
    /// cancelled, signInNeeded, album, photos: [{photo, file, photoId, url, albumError?} | {photo,
    /// file, error}]}`
    pub fn run(self, progress: &mut dyn FnMut(usize, &str) -> bool, cancel: &AtomicBool) -> Value {
        let total = self.items.len();
        let (mut photos, mut uploaded, mut failed, mut album_failed) = (Vec::new(), 0usize, 0usize, 0usize);
        let (mut cancelled, mut sign_in) = (false, false);
        for (i, item) in self.items.into_iter().enumerate() {
            let name = item.export.file_name.clone();
            if !progress(i, &name) || cancel.load(std::sync::atomic::Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let photo = item.export.photo.0;
            let what = format!("rendering {name} for Flickr");
            let rendered = crate::guard::catch(&what, || item.export.run()).and_then(|r| r);
            let result = rendered.and_then(|e| {
                self.client.upload(&self.token, &item.info, &e.file_name, &e.bytes, cancel).map_err(|err| {
                    if err.needs_sign_in() {
                        sign_in = true;
                    }
                    if err.is_cancelled() {
                        cancelled = true;
                    }
                    err.to_string()
                })
            });
            match result {
                Ok(id) => {
                    uploaded += 1;
                    let mut entry =
                        json!({"photo": photo, "file": name, "photoId": id, "url": lightcraft_flickr::photo_url(&self.token.user_nsid, &id)});
                    if let Some(album) = &self.album
                        && let Err(e) = self.client.add_to_album(&self.token, album, &id, cancel)
                    {
                        if e.is_cancelled() {
                            cancelled = true;
                        }
                        album_failed += 1;
                        log::warn!("flickr: {name}: not added to album {album}: {e}");
                        entry["albumError"] = json!(e.to_string());
                    }
                    photos.push(entry);
                    if cancelled {
                        break;
                    }
                }
                Err(e) => {
                    if cancelled {
                        break;
                    }
                    failed += 1;
                    log::warn!("flickr: {name}: {e}");
                    photos.push(json!({"photo": photo, "file": name, "error": e}));
                    if sign_in {
                        break;
                    }
                }
            }
        }
        progress(total.min(uploaded + failed), "");
        json!({
            "total": total,
            "uploaded": uploaded,
            "failed": failed,
            "albumFailed": album_failed,
            "cancelled": cancelled,
            "signInNeeded": sign_in,
            "album": self.album,
            "photos": photos,
        })
    }
}
