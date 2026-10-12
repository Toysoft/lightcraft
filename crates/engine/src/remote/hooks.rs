//! The engine's file hooks for `nextcloud://` paths, installed by [`crate::Session::with_fs`] around the
//! filesystem ones ([`route`]). A remote photo loads from its local original ([`Remote::local_original`]:
//! downloaded for interactive work only); background work decodes the server's preview of a non-raw
//! image instead, and a raw fails (its smart preview stands in). Embedded previews are the server's,
//! probes read the file's first bytes, and `content_hash` stays `None` (keys stay `file:{path}:{size}`).

use std::path::Path;
use std::sync::Arc;

use craft_nextcloud::RemotePath;
use lightcraft_catalog::is_remote;

use super::{NOT_DOWNLOADED, Remote};
use crate::files::{decode_upright_srgb, load_bytes, probe_bytes};
use crate::media::ProbeInfo;

/// Bytes read from the start of a remote file to probe it (see the NC5 commit for the measurement).
pub(crate) const HEADER_WINDOW: u64 = 256 << 10;
/// Read once more when [`HEADER_WINDOW`] didn't describe the file.
pub(crate) const HEADER_WINDOW_RETRY: u64 = 4 << 20;
/// The largest server preview asked for.
const MAX_SERVER_PREVIEW: usize = 4096;
/// Images (not raws) a background load decodes from the server's preview when the original isn't here.
const PREVIEWED: [&str; 13] = ["jpg", "jpeg", "png", "tif", "tiff", "webp", "gif", "bmp", "heic", "heif", "avif", "jxl", "psd"];

fn previewed(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| PREVIEWED.iter().any(|p| e.eq_ignore_ascii_case(p)))
}

fn parse(path: &str) -> Result<RemotePath, String> {
    RemotePath::parse(path).map_err(|e| e.to_string())
}

/// `f` on the local original of `path`, kept from eviction until `f` returns.
fn with_original<R>(remote: &Remote, path: &str, f: impl FnOnce(&str) -> Result<R, String>) -> Result<R, String> {
    let _hold = remote.cache().ok().zip(RemotePath::parse(path).ok()).map(|(cache, rp)| cache.hold(&rp));
    let local = remote.local_original(path)?;
    f(&local.to_string_lossy())
}

/// Wrap `s`'s file hooks (the filesystem's) so `nextcloud://` paths go to `s.remote`; installs the
/// byte reader and the existence check the same way.
pub(crate) fn route(s: &mut crate::Session) {
    let remote = s.remote.clone();
    if let Some(fs) = s.media.file_loader.clone() {
        let r = remote.clone();
        s.media.file_loader = Some(Arc::new(move |path: &str, max_edge: usize| {
            if !is_remote(path) {
                return fs(path, max_edge);
            }
            match with_original(&r, path, |local| fs(local, max_edge)) {
                Err(e) if e == NOT_DOWNLOADED && previewed(path) => {
                    let edge = u32::try_from(max_edge.min(MAX_SERVER_PREVIEW)).unwrap_or(1).max(1);
                    let bytes = r.server_preview(&parse(path)?, edge)?.ok_or(e)?;
                    load_bytes(&bytes, max_edge)
                }
                loaded => loaded,
            }
        }));
    }
    if let Some(fs) = s.media.denoise.loader.clone() {
        let r = remote.clone();
        s.media.denoise.loader =
            Some(Arc::new(
                move |path, max_edge, spec| {
                    if is_remote(path) { with_original(&r, path, |local| fs(local, max_edge, spec)) } else { fs(path, max_edge, spec) }
                },
            ));
    }
    if let Some(fs) = s.media.preview_loader.clone() {
        let r = remote.clone();
        s.media.preview_loader = Some(Arc::new(move |path: &str, max_edge: usize| {
            if !is_remote(path) {
                return fs(path, max_edge);
            }
            let edge = u32::try_from(max_edge.min(MAX_SERVER_PREVIEW)).unwrap_or(1).max(1);
            let bytes = r.server_preview(&RemotePath::parse(path).ok()?, edge).ok()??;
            decode_upright_srgb(&bytes, max_edge)
        }));
    }
    if let Some(fs) = s.media.file_probe.clone() {
        let r = remote.clone();
        s.media.file_probe = Some(Arc::new(move |path: &str| if is_remote(path) { probe(&r, path) } else { fs(path) }));
    }
    let r = remote.clone();
    s.media.file_bytes = Some(Arc::new(move |path: &str| {
        let read = |p: &str| std::fs::read(p).map_err(|e| format!("{p}: {e}"));
        if is_remote(path) { with_original(&r, path, read) } else { read(path) }
    }));
    s.media
        .availability
        .set_probe(Arc::new(move |path: &str| if is_remote(path) { remote.exists(path) } else { crate::availability::fs_exists(path) }));
}

/// Describe a remote file: from its local original when there is one, else from its first bytes
/// ([`HEADER_WINDOW`], then [`HEADER_WINDOW_RETRY`] if those weren't enough). The size is the server's.
fn probe(remote: &Remote, path: &str) -> Result<ProbeInfo, String> {
    let rp = parse(path)?;
    let name = rp.name().unwrap_or_default();
    let (etag, size) = remote.etag_and_size(&rp)?;
    let cache = remote.cache().ok();
    let _hold = cache.as_ref().map(|c| c.hold(&rp));
    let mut info = match cache.and_then(|c| c.cached(&rp, &etag)) {
        Some(local) => {
            let _permit = crate::memory::work_gate().acquire(usize::try_from(size).unwrap_or(usize::MAX));
            probe_bytes(name, &std::fs::read(&local).map_err(|e| format!("{}: {e}", local.display()))?)?
        }
        None => {
            let head = remote.read_head(&rp, HEADER_WINDOW)?;
            match describe(name, &head, size) {
                Err(_) if (head.len() as u64) < size => describe(name, &remote.read_head(&rp, HEADER_WINDOW_RETRY)?, size)?,
                described => described?,
            }
        }
    };
    info.file_size = size;
    info.content_hash = None;
    Ok(info)
}

/// [`probe_bytes`] of a file's first bytes `head` (of `size`). Unless they are the whole file, a
/// description from the embedded preview doesn't count: the raw data lies past the window, and the
/// photo would be marked preview-only at its preview's size (measured on real ARW and CR3 files).
// ponytail: a probe of the raw's header alone in lightcraft_raw (and a JPEG header read that doesn't
// need the end-of-image marker) would describe big files from 64-512 KiB, which hold all their metadata.
fn describe(name: &str, head: &[u8], size: u64) -> Result<ProbeInfo, String> {
    let whole = head.len() as u64 >= size;
    match probe_bytes(name, head) {
        Ok(info) if whole || info.preview_only.is_none() => Ok(info),
        Err(e) if whole => Err(e),
        _ => Err(format!("The start of {name} on the Nextcloud server doesn't describe it; it is read whole once downloaded")),
    }
}
