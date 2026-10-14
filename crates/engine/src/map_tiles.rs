//! Online map tiles for the Map view, used only when the user switched them on (they are off by default: the
//! map then shows the offline basemap of `lightcraft-map` and nothing is requested from anyone).
//!
//! With tiles on, the tiles covering the view are fetched from OpenStreetMap's tile server, which thereby
//! learns which part of the map is being looked at (not the photos, nor their exact positions). They are kept
//! on disk in `<settings>/map-tiles/z/x/y.png` and reused for [`FRESH`], so a place looked at before costs no
//! request. At most [`WORKERS`] requests run at once and nothing is fetched ahead, as the server's usage
//! policy asks; requests carry LightCraft's `User-Agent` (see `lightcraft-fetch`).
//!
//! The work runs on background threads that hold one activity row ("Loading map tiles") while any tile is
//! being fetched. On the web there is no fetching: [`MapTiles::available`] is false and tiles stay off.
//!
//! Sources: OpenStreetMap tile usage policy and slippy-map tile names (prose); own design.
// the web build keeps the switch and the status, but none of the fetching
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crate::activity::{Activity, TaskGuard};

pub const OSM_TILES: &str = "https://tile.openstreetmap.org";
pub const OSM_ATTRIBUTION: &str = "© OpenStreetMap contributors";
/// Requests in flight at once.
pub const WORKERS: usize = 2;
/// How long a tile on disk is used without asking for it again.
pub const FRESH: Duration = Duration::from_secs(30 * 24 * 3600);
/// A tile that failed is not asked for again for this long.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Tiles kept in memory (a 4K view is about 150).
const MEMORY_TILES: usize = 512;
/// The largest tile accepted (real ones are 5–60 kB).
const MAX_TILE_BYTES: u64 = 2 * 1024 * 1024;
const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A tile's address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileKey {
    /// `None` for an address that names no tile (zoom beyond 19, x or y outside the zoom level's grid).
    pub fn new(z: u8, x: u32, y: u32) -> Option<TileKey> {
        (f64::from(z) <= lightcraft_map::MAX_ZOOM && x < (1u32 << z) && y < (1u32 << z)).then_some(TileKey { z, x, y })
    }
}

#[derive(Default)]
struct Shared {
    enabled: bool,
    /// The server (no trailing slash) and the cache folder; `None`: the defaults.
    base: Option<String>,
    dir: Option<PathBuf>,
    /// Tiles wanted and not yet fetched, the most wanted first.
    queue: VecDeque<TileKey>,
    fetching: HashSet<TileKey>,
    ready: HashMap<TileKey, Arc<Vec<u8>>>,
    /// Arrival order of `ready`, for dropping the oldest.
    order: VecDeque<TileKey>,
    failed: HashMap<TileKey, Instant>,
    last_error: Option<String>,
    workers: usize,
    /// The activity row of the tiles being fetched, shared by the workers.
    task: Weak<TaskGuard>,
}

/// The session's map tiles.
#[derive(Default, Clone)]
pub struct MapTiles {
    shared: Arc<Mutex<Shared>>,
}

/// What [`MapTiles::status`] reports.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileStatus {
    pub available: bool,
    pub enabled: bool,
    /// Tiles wanted that have not arrived.
    pub pending: usize,
    pub in_memory: usize,
    /// Why the last tile could not be loaded, while that is still the case.
    pub last_error: Option<String>,
    pub source: String,
    pub attribution: &'static str,
}

impl MapTiles {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether this build can fetch tiles at all (not on the web).
    pub fn available() -> bool {
        cfg!(not(target_arch = "wasm32"))
    }

    pub fn enabled(&self) -> bool {
        Self::available() && self.lock().enabled
    }

    /// Switch online tiles on or off. Off also forgets what was waiting to be fetched.
    pub fn set_enabled(&self, on: bool) {
        let mut sh = self.lock();
        sh.enabled = on && Self::available();
        if !sh.enabled {
            sh.queue.clear();
            sh.failed.clear();
            sh.last_error = None;
        }
    }

    /// Use another server and cache folder (tests; a server other than the default must be https or this
    /// machine's loopback address).
    pub fn set_source(&self, base: &str, dir: &Path) -> Result<(), String> {
        let base = base.trim_end_matches('/');
        let loopback = base.strip_prefix("http://127.0.0.1").is_some_and(|rest| rest.is_empty() || rest.starts_with(':'));
        if !(base.starts_with("https://") || loopback) {
            return Err("the tile server must be an https:// address".into());
        }
        let mut sh = self.lock();
        sh.base = Some(base.to_string());
        sh.dir = Some(dir.to_path_buf());
        sh.ready.clear();
        sh.order.clear();
        sh.failed.clear();
        Ok(())
    }

    pub fn status(&self) -> TileStatus {
        let sh = self.lock();
        TileStatus {
            available: Self::available(),
            enabled: sh.enabled && Self::available(),
            pending: sh.queue.len().saturating_add(sh.fetching.len()),
            in_memory: sh.ready.len(),
            last_error: sh.last_error.clone(),
            source: sh.base.clone().unwrap_or_else(|| OSM_TILES.to_string()),
            attribution: OSM_ATTRIBUTION,
        }
    }

    /// A tile's PNG, if it has arrived.
    pub fn get(&self, key: TileKey) -> Option<Arc<Vec<u8>>> {
        self.lock().ready.get(&key).cloned()
    }

    /// The tiles the view needs now, the most important first. Those not in memory are fetched in the
    /// background (from disk when fresh, else from the server); tiles asked for earlier and no longer in
    /// `keys` are dropped from the queue, so panning does not fetch what has left the view. Does nothing
    /// while tiles are off. Returns how many of `keys` are still on their way.
    pub fn want(&self, activity: &Activity, keys: &[TileKey]) -> usize {
        let mut guard = self.lock();
        let sh = &mut *guard;
        if !sh.enabled || !Self::available() {
            return 0;
        }
        let now = Instant::now();
        sh.failed.retain(|_, at| now.duration_since(*at) < RETRY_AFTER);
        let missing: Vec<TileKey> = keys
            .iter()
            .filter(|k| TileKey::new(k.z, k.x, k.y).is_some() && !sh.ready.contains_key(k) && !sh.failed.contains_key(k))
            .copied()
            .take(MEMORY_TILES)
            .collect();
        sh.queue = missing.iter().filter(|k| !sh.fetching.contains(k)).copied().collect();
        let pending = missing.len();
        let start = sh.queue.len().min(WORKERS.saturating_sub(sh.workers));
        drop(guard);
        for _ in 0..start {
            self.spawn(activity);
        }
        pending
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn(&self, _: &Activity) {}

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(&self, activity: &Activity) {
        let task = {
            let mut sh = self.lock();
            if sh.workers >= WORKERS {
                return;
            }
            sh.workers += 1;
            match sh.task.upgrade() {
                Some(t) => t,
                None => {
                    let t = Arc::new(activity.start("download", "Loading map tiles", crate::activity::Cancel::Yes));
                    t.detail("OpenStreetMap");
                    sh.task = Arc::downgrade(&t);
                    t
                }
            }
        };
        let store = self.clone();
        let spawned = std::thread::Builder::new().name("map-tiles".into()).spawn(move || {
            // a panic in a fetch must not leave the worker count stuck: the map would never load again
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native::work(&store, &task)));
            let mut sh = store.lock();
            sh.workers = sh.workers.saturating_sub(1);
            if task.is_cancelled() {
                sh.queue.clear();
            }
        });
        if let Err(e) = spawned {
            let mut sh = self.lock();
            sh.workers = sh.workers.saturating_sub(1);
            sh.last_error = Some(format!("could not start loading map tiles: {e}"));
        }
    }

    fn finish(&self, key: TileKey, result: Result<Vec<u8>, String>) {
        let mut sh = self.lock();
        sh.fetching.remove(&key);
        match result {
            Ok(bytes) => {
                if sh.ready.insert(key, Arc::new(bytes)).is_none() {
                    sh.order.push_back(key);
                }
                while sh.order.len() > MEMORY_TILES {
                    if let Some(old) = sh.order.pop_front() {
                        sh.ready.remove(&old);
                    }
                }
                sh.last_error = None;
            }
            Err(e) => {
                log::debug!("map tile {}/{}/{}: {e}", key.z, key.x, key.y);
                sh.failed.insert(key, Instant::now());
                sh.last_error = Some(e);
            }
        }
    }
}

/// Whether `bytes` is a PNG small enough to be a tile.
fn is_tile(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_MAGIC) && bytes.len() as u64 <= MAX_TILE_BYTES
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::atomic::AtomicBool;
    use std::time::SystemTime;

    use super::*;

    fn default_dir() -> Option<PathBuf> {
        crate::config::config_dir().map(|d| d.join("map-tiles"))
    }

    /// The tile on disk, and whether it is still fresh.
    fn cached(file: &Path) -> Option<(Vec<u8>, bool)> {
        let meta = std::fs::metadata(file).ok()?;
        if meta.len() > MAX_TILE_BYTES {
            return None;
        }
        let bytes = std::fs::read(file).ok().filter(|b| is_tile(b))?;
        let age = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok());
        Some((bytes, age.is_some_and(|a| a < FRESH)))
    }

    fn fetch(key: TileKey, base: &str, dir: Option<&Path>, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        let dir = dir.ok_or("there is no settings folder to keep map tiles in")?;
        let folder = dir.join(key.z.to_string()).join(key.x.to_string());
        let name = format!("{}.png", key.y);
        let file = folder.join(&name);
        let old = cached(&file);
        if let Some((bytes, true)) = old {
            return Ok(bytes);
        }
        let fetched = (|| {
            std::fs::create_dir_all(&folder).map_err(|e| format!("could not create {}: {e}", folder.display()))?;
            // into a folder of its own, so a failed or wrong download never replaces a good tile
            let staging = folder.join(format!(".{}-{}", key.y, std::process::id()));
            std::fs::create_dir_all(&staging).map_err(|e| format!("could not create {}: {e}", staging.display()))?;
            let spec = lightcraft_fetch::FileSpec { name: &name, size: None, sha256: None, max: MAX_TILE_BYTES };
            let opts = lightcraft_fetch::Options { attempts: 1, ..Default::default() };
            let url = format!("{base}/{}/{}/{}.png", key.z, key.x, key.y);
            let got = lightcraft_fetch::download_url(&spec, &url, &staging, &opts, cancel, &mut |_| {})
                .map_err(|e| e.to_string())
                .and_then(|()| std::fs::read(staging.join(&name)).map_err(|e| e.to_string()))
                .and_then(|b| if is_tile(&b) { Ok(b) } else { Err("the map server did not send a picture".to_string()) });
            if let Ok(bytes) = &got {
                // a tile that can't be kept is still shown
                if let Err(e) = std::fs::write(&file, bytes) {
                    log::debug!("map tile cache: {}: {e}", file.display());
                }
            }
            let _ = std::fs::remove_dir_all(&staging);
            got
        })();
        // an old tile is better than none when the server can't be reached
        fetched.or_else(|e| old.map(|(bytes, _)| bytes).ok_or(e))
    }

    /// Fetch queued tiles until the queue is empty, tiles are switched off or the task is cancelled.
    pub(super) fn work(store: &MapTiles, task: &TaskGuard) {
        let never = AtomicBool::new(false);
        loop {
            let (key, base, dir) = {
                let mut sh = store.lock();
                if !sh.enabled || task.is_cancelled() {
                    return;
                }
                let Some(key) = sh.queue.pop_front() else { return };
                sh.fetching.insert(key);
                (key, sh.base.clone().unwrap_or_else(|| OSM_TILES.to_string()), sh.dir.clone().or_else(default_dir))
            };
            let flag = task.cancel_flag();
            let result = fetch(key, &base, dir.as_deref(), flag.as_deref().unwrap_or(&never));
            store.finish(key, result);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn png(tag: u8) -> Vec<u8> {
        let mut b = PNG_MAGIC.to_vec();
        b.extend_from_slice(&[tag; 16]);
        b
    }

    /// A tile server on this machine: `/z/x/y.png` is a PNG whose filler byte is `x`; zoom 7 is missing, zoom 8
    /// sends text. Counts the requests it gets.
    fn serve() -> (String, Arc<AtomicUsize>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(mut c) = conn else { continue };
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                let _ = r.read_line(&mut line);
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let path = line.split(' ').nth(1).unwrap_or("").to_string();
                let parts: Vec<&str> = path.trim_start_matches('/').trim_end_matches(".png").split('/').collect();
                let (status, body): (&str, Vec<u8>) = match parts.as_slice() {
                    ["7", ..] => ("404 Not Found", b"no such tile".to_vec()),
                    ["8", ..] => ("200 OK", b"<html>blocked</html>".to_vec()),
                    [_, x, _] => ("200 OK", png(x.parse().unwrap_or(0))),
                    _ => ("400 Bad Request", Vec::new()),
                };
                let _ = write!(c, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = c.write_all(&body);
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    fn cache(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightcraft-map-tiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn key(z: u8, x: u32, y: u32) -> TileKey {
        TileKey::new(z, x, y).unwrap()
    }

    /// Ask for `keys` until none is pending (or 10 s pass).
    fn settle(tiles: &MapTiles, activity: &Activity, keys: &[TileKey]) {
        let end = Instant::now() + Duration::from_secs(10);
        while tiles.want(activity, keys) > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        while tiles.lock().workers > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nothing_is_fetched_until_tiles_are_switched_on() {
        let (base, hits) = serve();
        let (tiles, activity) = (MapTiles::default(), Activity::default());
        tiles.set_source(&base, &cache("off")).unwrap();
        assert!(!tiles.enabled());
        assert_eq!(tiles.want(&activity, &[key(3, 1, 2)]), 0);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(tiles.get(key(3, 1, 2)).is_none() && activity.list().is_empty());
        assert_eq!(tiles.status().pending, 0);
    }

    #[test]
    fn tiles_are_fetched_kept_on_disk_and_not_asked_for_twice() {
        let (base, hits) = serve();
        let dir = cache("fetch");
        let (tiles, activity) = (MapTiles::default(), Activity::default());
        tiles.set_source(&base, &dir).unwrap();
        tiles.set_enabled(true);
        let keys = [key(5, 17, 11), key(5, 18, 11), key(5, 19, 11)];
        assert_eq!(tiles.want(&activity, &keys), 3);
        // the fetch shows in the activity stack while it runs
        assert!(activity.list().iter().any(|t| t.label == "Loading map tiles") || tiles.status().pending == 0);
        settle(&tiles, &activity, &keys);
        for k in keys {
            assert_eq!(tiles.get(k).as_deref(), Some(&png(k.x as u8)), "{k:?}");
            assert!(dir.join("5").join(k.x.to_string()).join("11.png").is_file());
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert!(activity.list().is_empty(), "the row goes when the tiles are in");
        assert_eq!(tiles.status().last_error, None);
        // again: from memory. A new session with the same folder: from disk. Neither asks the server
        settle(&tiles, &activity, &keys);
        let again = MapTiles::default();
        again.set_source(&base, &dir).unwrap();
        again.set_enabled(true);
        settle(&again, &activity, &keys);
        assert_eq!(again.get(keys[1]).as_deref(), Some(&png(18)));
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        // nothing is left in the tile folders but the tiles
        let left: Vec<_> = std::fs::read_dir(dir.join("5").join("17")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("11.png")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_tile_or_a_page_instead_of_a_picture_is_an_error_to_read_not_a_tile() {
        let (base, hits) = serve();
        let dir = cache("bad");
        let (tiles, activity) = (MapTiles::default(), Activity::default());
        tiles.set_source(&base, &dir).unwrap();
        tiles.set_enabled(true);
        let keys = [key(7, 1, 1), key(8, 1, 1)];
        settle(&tiles, &activity, &keys);
        assert!(tiles.get(keys[0]).is_none() && tiles.get(keys[1]).is_none());
        assert!(tiles.status().last_error.is_some());
        assert!(!dir.join("8").join("1").join("1.png").exists(), "the page is not kept as a tile");
        // a failed tile is left alone for a while instead of being asked for every frame
        let before = hits.load(Ordering::SeqCst);
        assert_eq!(tiles.want(&activity, &keys), 0);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(hits.load(Ordering::SeqCst), before);
        // a good tile clears the error; switching tiles off forgets the failures
        settle(&tiles, &activity, &[key(5, 3, 3)]);
        assert_eq!(tiles.status().last_error, None);
        tiles.set_enabled(false);
        assert!(!tiles.enabled() && tiles.lock().failed.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreachable_server_falls_back_to_the_old_tile_on_disk() {
        let dir = cache("stale");
        let file = dir.join("4").join("2");
        std::fs::create_dir_all(&file).unwrap();
        let tile = file.join("3.png");
        std::fs::write(&tile, png(9)).unwrap();
        // older than FRESH
        let old = std::time::SystemTime::now() - FRESH - Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&tile).unwrap().set_modified(old).unwrap();
        // a port nothing listens on
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (tiles, activity) = (MapTiles::default(), Activity::default());
        tiles.set_source(&format!("http://127.0.0.1:{port}"), &dir).unwrap();
        tiles.set_enabled(true);
        settle(&tiles, &activity, &[key(4, 2, 3), key(4, 2, 4)]);
        assert_eq!(tiles.get(key(4, 2, 3)).as_deref(), Some(&png(9)));
        assert!(tiles.get(key(4, 2, 4)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_real_tile_addresses_and_safe_servers_are_accepted() {
        assert!(TileKey::new(0, 0, 0).is_some() && TileKey::new(19, 524_287, 524_287).is_some());
        assert!(
            TileKey::new(0, 1, 0).is_none()
                && TileKey::new(3, 8, 0).is_none()
                && TileKey::new(20, 0, 0).is_none()
                && TileKey::new(255, 0, 0).is_none()
        );
        let tiles = MapTiles::default();
        let dir = cache("source");
        for bad in ["http://example.com", "ftp://x", "", "http://127.0.0.1.example.com", "file:///etc"] {
            assert!(tiles.set_source(bad, &dir).is_err(), "{bad}");
        }
        assert!(tiles.set_source("https://tiles.example.org/", &dir).is_ok());
        assert_eq!(tiles.status().source, "https://tiles.example.org");
        // addresses that name no tile are ignored, not fetched
        tiles.set_enabled(true);
        let activity = Activity::default();
        assert_eq!(tiles.want(&activity, &[TileKey { z: 2, x: 9, y: 0 }, TileKey { z: 40, x: 0, y: 0 }]), 0);
    }
}
