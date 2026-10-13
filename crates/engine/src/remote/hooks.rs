//! The engine's file hooks for `nextcloud://` paths, installed by [`crate::Session::with_fs`] around the
//! filesystem ones ([`route`]). A remote photo loads from its local original ([`Remote::local_original`]:
//! downloaded for interactive work only); background work makes a non-raw image's thumbnail from the
//! server's preview instead, and otherwise fails (a smart preview stands in). Embedded previews are the
//! server's (a raw's own, from the file's first MiB, when the server has none), probes read the file's
//! first bytes, and `content_hash` stays `None` (keys stay `file:{path}:{size}`).

use std::path::Path;
use std::sync::Arc;

use craft_nextcloud::RemotePath;
use lightcraft_catalog::{MediaKind, is_remote};

use super::originals::CACHE_IN_USE;
use super::{NOT_DOWNLOADED, Remote};
use crate::files::{decode_upright_srgb, embedded_preview_srgb, load_bytes, probe_bytes};
use crate::media::{ProbeInfo, SourceLevel};

/// Bytes read from the start of a remote file to probe it (see the NC5 commit for the measurement).
pub(crate) const HEADER_WINDOW: u64 = 256 << 10;
/// A file up to this size is read whole when [`HEADER_WINDOW`] didn't describe it.
pub(crate) const HEADER_WINDOW_RETRY: u64 = 4 << 20;
/// The largest server preview asked for.
const MAX_SERVER_PREVIEW: usize = 4096;
/// Bytes read from the start of a raw the server has no preview of (no preview provider for its
/// format), for the raw's own embedded preview. Measured on real Canon R8 CR3s: their `PRVW` JPEG
/// (1620×1080) ends at 435 and 524 KB; 512 KiB missed one.
const RAW_PREVIEW_WINDOW: u64 = 1 << 20;
/// Images (not raws) a background load decodes from the server's preview when the original isn't here.
const PREVIEWED: [&str; 13] = ["jpg", "jpeg", "png", "tif", "tiff", "webp", "gif", "bmp", "heic", "heif", "avif", "jxl", "psd"];

pub(crate) fn previewed(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| PREVIEWED.iter().any(|p| e.eq_ignore_ascii_case(p)))
}

/// Whether a background load of `max_edge` may stand in the server's preview for the original: for a
/// thumbnail, or for work that only looks (the face scan, Find Similar). A larger one would be kept as
/// the photo's preview-size source (a loupe prefetch's result is accepted into the cache, and so is a
/// UI command's, e.g. Auto Straighten's), and Develop would then edit the server's JPEG instead of
/// downloading the original.
fn stand_in_allowed(max_edge: usize) -> bool {
    max_edge <= SourceLevel::Thumb.max_edge() || (crate::memory::is_without_downloads() && !crate::memory::is_ui_thread())
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
                // (no original here: not downloaded, or another process has the download cache)
                Err(e) if (e == NOT_DOWNLOADED || e == CACHE_IN_USE) && previewed(path) && stand_in_allowed(max_edge) => {
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
            let rp = RemotePath::parse(path).ok()?;
            let edge = u32::try_from(max_edge.min(MAX_SERVER_PREVIEW)).unwrap_or(1).max(1);
            match r.server_preview(&rp, edge).ok()? {
                Some(bytes) => decode_upright_srgb(&bytes, max_edge),
                // (none when its start doesn't hold the whole preview: a JPEG the window cuts off,
                // with no EOI in it, would decode half grey)
                None if !previewed(path) => {
                    let head = r.read_head(&rp, RAW_PREVIEW_WINDOW).ok()?;
                    lightcraft_raw::embedded_preview(&head).filter(|j| j.ends_with(&[0xff, 0xd9]))?;
                    embedded_preview_srgb(&head, max_edge)
                }
                None => None,
            }
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
            // (the copy's own length, not the server's word for it)
            let len = std::fs::metadata(&local).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
            let _permit = crate::memory::work_gate().acquire(len);
            probe_bytes(name, &std::fs::read(&local).map_err(|e| format!("{}: {e}", local.display()))?)?
        }
        None => {
            let head = remote.read_head(&rp, HEADER_WINDOW)?;
            match describe(name, &head, size) {
                // a small file is read whole; the start of a big raw never describes it (see `describe`)
                Err(_) if (head.len() as u64) < size && size <= HEADER_WINDOW_RETRY => {
                    describe(name, &remote.read_head(&rp, HEADER_WINDOW_RETRY)?, size)?
                }
                described => described?,
            }
        }
    };
    info.file_size = size;
    info.content_hash = None;
    Ok(info)
}

/// [`probe_bytes`] of a file's first bytes `head` (of `size`). From part of a file, only an image's
/// description counts: a raw's data lies past the window, and what the start says about it is wrong
/// (measured on real ARW and CR3 files: preview-only at the preview's size, or another as-shot white
/// balance). A failure there is the cut's, unless the format can't be opened anyway (unknown, or not
/// in this build).
// ponytail: a probe of the raw's header alone in lightcraft_raw (and a JPEG header read that doesn't
// need the end-of-image marker) would describe big files from 64-512 KiB, which hold all their metadata.
fn describe(name: &str, head: &[u8], size: u64) -> Result<ProbeInfo, String> {
    let described = probe_bytes(name, head);
    if head.len() as u64 >= size {
        return described;
    }
    let opens = lightcraft_raw::probe(head).is_some() || lightcraft_codecs::sniff(head).is_some_and(|f| f.not_decodable().is_none());
    match described {
        Ok(info) if info.kind != MediaKind::Raw => Ok(info),
        Err(e) if !opens => Err(e),
        _ => Err(format!("The start of {name} on the Nextcloud server doesn't describe it; it is read whole once downloaded")),
    }
}
