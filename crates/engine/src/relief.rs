//! Scene geometry for Relight: analyzing a photo with the MoGe-2 model (`lightcraft-relief`) and
//! keeping what it saw (surface normals and depth) in the photo's settings, and the model's
//! download.
//!
//! **The model is optional.** It is not part of LightCraft, and nothing requires it: without it
//! Relight estimates the relief from the photo's brightness, a photo that was analyzed keeps its
//! geometry in its settings (so it renders and exports anywhere, the web included), and no render
//! ever waits for the model. The analysis runs on a thread of its own, shown in the activity stack.

use std::path::PathBuf;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{ReliefSample, RelightGeometry};

use crate::Session;

/// Size of the model file (134 MB).
pub const MODEL_BYTES: u64 = 140_550_416;
pub const LICENSE_NAME: &str = "MIT License (Microsoft)";
pub const LICENSE_URL: &str = "https://huggingface.co/Ruicheng/moge-2-vits-normal";
/// Start of the error every command reports while the model is missing.
pub const NOT_INSTALLED: &str = "The scene geometry model is not installed";

/// Long edge of the render of the photo the model looks at.
#[cfg_attr(not(feature = "relief"), allow(dead_code))]
const INPUT_EDGE: usize = 1024;
/// ViT tokens of an analysis: the low end of what the model was trained on, since lighting is smooth.
#[cfg_attr(not(feature = "relief"), allow(dead_code))]
const TOKENS: usize = 1200;
/// A surface the model is less sure of than this is treated as none (sky).
const SURFACE: f32 = 0.5;

/// What the session sees of the model and its work.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// This build can analyze photos at all.
    pub available: bool,
    pub installed: bool,
    pub downloading: bool,
    pub done: u64,
    pub total: u64,
    /// The last download's error (cleared when a new one starts).
    pub error: Option<String>,
    /// The photo being analyzed, if any.
    pub analyzing: Option<u64>,
}

/// An analysis on its thread.
struct Running {
    photo: PhotoId,
    result: std::sync::mpsc::Receiver<Result<RelightGeometry, String>>,
}

/// The model's folder, its download, and the analysis in flight.
#[derive(Default)]
pub struct Relief {
    /// Where `model.pt` lives (set by the app; `None`: photos can't be analyzed).
    pub dir: Option<PathBuf>,
    /// The user's list of download mirrors (one base URL per line), if any.
    pub mirrors_file: Option<PathBuf>,
    /// Analyze in the background and apply the result in [`Session::relief_poll`] (the desktop
    /// app); otherwise the command waits for it (CLI, MCP, tests).
    pub background: bool,
    running: Option<Running>,
    #[cfg(feature = "relief")]
    download: crate::segment::download::Downloader,
}

impl Relief {
    /// Whether this build can analyze photos at all.
    pub const AVAILABLE: bool = cfg!(feature = "relief");

    pub fn installed(&self) -> bool {
        #[cfg(feature = "relief")]
        return self.dir.as_deref().is_some_and(lightcraft_relief::is_model_dir);
        #[cfg(not(feature = "relief"))]
        false
    }

    /// The photo being analyzed, if any.
    pub fn analyzing(&self) -> Option<PhotoId> {
        self.running.as_ref().map(|r| r.photo)
    }

    pub fn status(&self) -> Status {
        #[cfg_attr(not(feature = "relief"), allow(unused_mut))]
        let mut status =
            Status { available: Self::AVAILABLE, installed: self.installed(), analyzing: self.analyzing().map(|id| id.0), ..Default::default() };
        #[cfg(feature = "relief")]
        {
            let d = self.download.status();
            status.downloading = d.running;
            status.done = d.done;
            status.total = if d.total > 0 { d.total } else { MODEL_BYTES };
            status.error = d.error;
        }
        status
    }

    /// The model folder, if the model is there; otherwise an error that says what to do.
    pub fn model_dir(&self) -> Result<PathBuf, String> {
        if !Self::AVAILABLE {
            return Err("Analyzing a photo's scene is not available in this build".into());
        }
        let dir = self.dir.clone().ok_or("no folder is set for the scene geometry model")?;
        if self.installed() {
            return Ok(dir);
        }
        let status = self.status();
        if status.downloading {
            let pct = status.done.saturating_mul(100).checked_div(status.total).unwrap_or(0);
            return Err(format!("{NOT_INSTALLED} yet: it is downloading ({pct} %)."));
        }
        Err(format!(
            "{NOT_INSTALLED}. Download it (about {} MB, {LICENSE_NAME}) with `relight.model.download`, or put model.pt in {}.",
            MODEL_BYTES / 1_000_000,
            dir.display()
        ))
    }

    /// Start downloading the model on a background thread. `Ok(false)` when it is installed or
    /// downloading already.
    pub fn start_download(&self, activity: &crate::activity::Activity) -> Result<bool, String> {
        if !Self::AVAILABLE {
            return Err("Analyzing a photo's scene is not available in this build".into());
        }
        let dir = self.dir.clone().ok_or("no folder is set for the scene geometry model")?;
        if self.installed() {
            return Ok(false);
        }
        #[cfg(feature = "relief")]
        {
            let env = std::env::var(lightcraft_relief::fetch::MIRRORS_ENV).ok();
            let mirrors = lightcraft_relief::fetch::mirrors(env.as_deref(), self.mirrors_file.as_deref());
            self.download.start(
                lightcraft_relief::fetch::FILES,
                mirrors,
                dir,
                lightcraft_relief::fetch::Options::default(),
                activity,
                "Scene geometry (MoGe-2)",
            )
        }
        #[cfg(not(feature = "relief"))]
        {
            let _ = (dir, activity);
            Ok(false)
        }
    }

    /// Stop a running download (its partial file stays, to resume). False when none runs.
    pub fn cancel_download(&self) -> bool {
        #[cfg(feature = "relief")]
        return self.download.cancel();
        #[cfg(not(feature = "relief"))]
        false
    }
}

/// What a model saw, as a photo keeps it: `normal` (camera space: x right, y down, z away from the
/// camera), `depth` (positive, larger = farther, any scale) and `surface` (0..1) on a
/// `width × height` grid, reduced to [`lightcraft_develop::relief::STORE_EDGE`] on its long edge.
/// The depth becomes a height, 0 (the farthest surface, or none) to 1 (the nearest), on a
/// logarithmic scale between the 1st and 99th percentile of the surfaces, so one far or near
/// stray sample does not flatten the rest.
pub fn stored_geometry(width: usize, height: usize, normal: &[[f32; 3]], depth: &[f32], surface: &[f32]) -> Option<RelightGeometry> {
    let n = width.checked_mul(height).filter(|n| *n > 0)?;
    if normal.len() != n || depth.len() != n || surface.len() != n {
        return None;
    }
    let is_surface = |i: usize| surface.get(i).is_some_and(|s| *s >= SURFACE) && depth.get(i).is_some_and(|d| d.is_finite() && *d > 0.0);
    let mut logs: Vec<f32> = (0..n).filter(|i| is_surface(*i)).filter_map(|i| depth.get(i).map(|d| d.ln())).collect();
    logs.sort_by(f32::total_cmp);
    let percentile = |p: f64| logs.get(((logs.len().saturating_sub(1)) as f64 * p) as usize).copied();
    let (near, far) = (percentile(0.01).unwrap_or(0.0), percentile(0.99).unwrap_or(1.0));
    let span = (far - near).max(1e-6);
    let sample = |i: usize| -> ReliefSample {
        if !is_surface(i) {
            return ReliefSample { nx: 0.0, ny: 0.0, height: 0.0 };
        }
        let d = depth.get(i).map_or(far, |d| d.ln());
        let v = normal.get(i).copied().unwrap_or([0.0, 0.0, -1.0]);
        ReliefSample { nx: v[0], ny: v[1], height: (1.0 - (d - near) / span).clamp(0.0, 1.0) }
    };
    // box-average down to the stored grid
    let edge = lightcraft_develop::relief::STORE_EDGE;
    let scale = (edge as f64 / width.max(height) as f64).min(1.0);
    let (sw, sh) = (((width as f64 * scale).round() as usize).max(1), ((height as f64 * scale).round() as usize).max(1));
    let mut out = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        let (y0, y1) = (y * height / sh, ((y + 1) * height / sh).max(y * height / sh + 1).min(height));
        for x in 0..sw {
            let (x0, x1) = (x * width / sw, ((x + 1) * width / sw).max(x * width / sw + 1).min(width));
            let (mut sum, mut count) = ([0f32; 3], 0f32);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let s = sample(yy * width + xx);
                    if s.nx.is_finite() && s.ny.is_finite() && s.height.is_finite() {
                        sum = [sum[0] + s.nx, sum[1] + s.ny, sum[2] + s.height];
                        count += 1.0;
                    }
                }
            }
            let k = if count > 0.0 { 1.0 / count } else { 0.0 };
            out.push(ReliefSample { nx: sum[0] * k, ny: sum[1] * k, height: sum[2] * k });
        }
    }
    RelightGeometry::encode(sw, sh, &out)
}

/// Render the photo and run the model on it.
#[cfg(feature = "relief")]
fn analyze(render: crate::media::RenderJob, dir: &std::path::Path, missing: Option<String>) -> Result<RelightGeometry, String> {
    let image = match render.run().rendered {
        Ok(r) => r.image,
        Err(e) => {
            return Err(match missing {
                Some(path) => format!(
                    "This photo's file is missing (moved or deleted), so its scene can't be analyzed: {path}. Library ▸ Find Missing Photos can relink it."
                ),
                None => format!("couldn't render the photo to analyze its scene: {e}"),
            });
        }
    };
    let rgb: Vec<[f32; 3]> = image.data.iter().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
    let model = lightcraft_relief::Moge::load(dir).map_err(|e| e.to_string())?;
    let g = model.infer(&rgb, image.width, image.height, TOKENS).map_err(|e| e.to_string())?;
    stored_geometry(g.width, g.height, &g.normal, &g.depth, &g.surface).ok_or_else(|| "the model's result could not be stored".to_string())
}

impl Session {
    /// Keep `geometry` as photo `id`'s scene (one history step).
    pub fn relief_store(&mut self, id: PhotoId, geometry: RelightGeometry) -> Result<(), String> {
        let mut d = (*self.develop_of(id).ok_or("no such photo")?).clone();
        d.relight.geometry = Some(std::sync::Arc::new(geometry));
        self.set_develop(id, d, "Analyze Scene").map_err(|e| e.to_string())
    }

    /// Analyze photo `id`'s scene with the model. In background mode this returns `Ok(false)` at
    /// once and the result is applied by [`Session::relief_poll`]; otherwise it waits, stores the
    /// geometry and returns `Ok(true)`.
    pub fn relief_analyze(&mut self, id: PhotoId) -> Result<bool, String> {
        let dir = self.relief.model_dir()?;
        if let Some(busy) = self.relief.analyzing() {
            return Err(if busy == id {
                "This photo is being analyzed already".to_string()
            } else {
                "Another photo is being analyzed: wait for it to finish".into()
            });
        }
        #[cfg(feature = "relief")]
        {
            let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
            // the photo as it looks, whole and unflipped, without what Relight or masks add
            let mut d = (*p.develop).clone();
            d.masks.clear();
            d.relight = Default::default();
            d.crop.flip_h = false;
            d.crop.flip_v = false;
            d.geometry.constrain_crop = false;
            let render = self.preview_job(id, INPUT_EDGE, INPUT_EDGE, false, &d).ok_or("no such photo")?;
            let missing = match &p.source {
                lightcraft_catalog::Source::File { path } if !std::path::Path::new(path).exists() => Some(path.clone()),
                _ => None,
            };
            let work = move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyze(render, &dir, missing)));
            let unexpected = || Err("analyzing the scene failed unexpectedly".to_string());
            if !self.relief.background {
                let geometry = work().unwrap_or_else(|_| unexpected())?;
                self.relief_store(id, geometry)?;
                return Ok(true);
            }
            let task = self.activity.start("relight", "Analyzing scene", crate::activity::Cancel::No);
            task.detail(&p.file_name);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("relief".into())
                .spawn(move || {
                    let _task = task;
                    // (the session may be gone by now: then nobody wants the result)
                    let _ = tx.send(work().unwrap_or_else(|_| unexpected()));
                })
                .map_err(|e| format!("could not start the analysis: {e}"))?;
            self.relief.running = Some(Running { photo: id, result: rx });
            Ok(false)
        }
        #[cfg(not(feature = "relief"))]
        {
            let _ = dir;
            Err("Analyzing a photo's scene is not available in this build".into())
        }
    }

    /// Apply a finished background analysis: `Some(Ok(photo))` when its geometry was stored,
    /// `Some(Err(why))` when it failed, `None` while there is nothing new.
    pub fn relief_poll(&mut self) -> Option<Result<PhotoId, String>> {
        let running = self.relief.running.as_ref()?;
        let outcome = match running.result.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("analyzing the scene stopped unexpectedly".to_string()),
        };
        let photo = self.relief.running.take()?.photo;
        Some(outcome.and_then(|geometry| self.relief_store(photo, geometry)).map(|()| photo))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Given what a model saw of a ball in front of a wall, the stored geometry puts the ball near and the wall far
    #[test]
    fn depth_becomes_a_height_between_the_nearest_and_farthest_surface() {
        let (w, h) = (64, 48);
        let mut normal = vec![[0.0, 0.0, -1.0]; w * h];
        let mut depth = vec![4.0f32; w * h];
        let mut surface = vec![1.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if (20..44).contains(&x) && (12..36).contains(&y) {
                    depth[i] = 1.0;
                    normal[i] = [0.6, 0.0, -0.8];
                }
                if y < 6 {
                    // sky: no surface, and a depth that means nothing
                    surface[i] = 0.02;
                    depth[i] = 1e9;
                }
            }
        }
        let stored = stored_geometry(w, h, &normal, &depth, &surface).unwrap();
        assert_eq!(stored.size(), Some((w, h)), "already smaller than the stored grid: kept as it is");
        let samples = stored.decode().unwrap();
        let at = |x: usize, y: usize| samples[y * w + x];
        assert!(at(32, 24).height > 0.95 && (at(32, 24).nx - 0.6).abs() < 0.02, "{:?}", at(32, 24));
        assert!(at(4, 40).height < 0.05 && at(4, 40).nx.abs() < 0.02, "{:?}", at(4, 40));
        assert_eq!(at(30, 2), ReliefSample { nx: 0.0, ny: 0.0, height: 0.0 }, "sky is flat and far");
    }

    // Given a large grid, it is reduced to the stored size keeping its shape
    #[test]
    fn a_large_grid_is_reduced_to_the_stored_edge() {
        let (w, h) = (800, 500);
        let stored = stored_geometry(w, h, &vec![[0.0, 0.0, -1.0]; w * h], &vec![2.0; w * h], &vec![1.0; w * h]).unwrap();
        assert_eq!(stored.size(), Some((256, 160)));
        assert!(stored.data.len() < 4000, "a flat scene packs to almost nothing: {}", stored.data.len());
    }

    // Given numbers no model should produce, or sizes that don't match, nothing panics
    #[test]
    fn hostile_model_output_is_stored_flat_or_refused() {
        let n = 16 * 16;
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            let stored = stored_geometry(16, 16, &vec![[bad; 3]; n], &vec![bad; n], &vec![bad; n]);
            assert!(stored.is_some_and(|g| g.decode().is_some_and(|s| s.iter().all(|s| s.height.is_finite() && s.nx.is_finite()))), "{bad}");
        }
        assert!(stored_geometry(16, 16, &[], &[], &[]).is_none());
        assert!(stored_geometry(0, 16, &[], &[], &[]).is_none());
        assert!(stored_geometry(usize::MAX, 2, &[], &[], &[]).is_none());
    }

    // Given no model, the status and the errors say so, and nothing is analyzed
    #[test]
    fn without_the_model_analyzing_says_what_to_do() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        s.relief.dir = Some(std::env::temp_dir().join("lightcraft-no-relief-model"));
        let status = s.relief.status();
        assert!(!status.installed && !status.downloading && status.analyzing.is_none());
        let e = s.relief_analyze(id).unwrap_err();
        if Relief::AVAILABLE {
            assert!(e.starts_with(NOT_INSTALLED) && e.contains("relight.model.download") && e.contains("lightcraft-no-relief-model"), "{e}");
        } else {
            assert!(e.contains("not available in this build"), "{e}");
        }
        assert!(s.relief_poll().is_none());
        assert!(s.develop_of(id).unwrap().relight.geometry.is_none());
    }

    // Given geometry stored on a photo, it is one history step that undo takes away
    #[test]
    fn stored_geometry_is_one_undo_step() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        let n = 8 * 8;
        let geometry = stored_geometry(8, 8, &vec![[0.0, 0.0, -1.0]; n], &vec![1.0; n], &vec![1.0; n]).unwrap();
        s.relief_store(id, geometry.clone()).unwrap();
        assert_eq!(s.develop_of(id).unwrap().relight.geometry.as_deref(), Some(&geometry));
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert!(s.develop_of(id).unwrap().relight.geometry.is_none());
    }
}
