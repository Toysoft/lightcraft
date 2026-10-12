//! Loading the checkpoint and running the whole model.
//!
//! Modified work (MIT License): written by the LightCraft contributors in 2026 after the
//! Python/PyTorch MoGe code (`moge/model/v2.py`, `modules/dinov2_encoder.py`,
//! `utils/geometry_torch.py`), Copyright (c) Microsoft Corporation; translated to Rust on candle
//! and reduced to inference of the `moge-2-vits-normal` checkpoint. See NOTICE.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, VarBuilder};

use crate::stack::{DIMS, Stack};
use crate::vit::{HIDDEN, PATCH, Vit};
use crate::{Error, Geometry, MAX_INPUT_EDGE, Result, TOKENS, WEIGHTS_FILE};

/// The backbone blocks whose tokens the encoder reads.
const TAKE: [usize; 2] = [5, 11];
/// ImageNet statistics the backbone's input is normalized with.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
/// Largest exponent of the depth head taken at face value (e^30 is far beyond any scene).
const MAX_LOG_DEPTH: f32 = 30.0;

/// The best device here: Metal on macOS (when a GPU is available), else the CPU.
pub fn best_device() -> Device {
    #[cfg(target_os = "macos")]
    if let Ok(d) = Device::new_metal(0) {
        return d;
    }
    Device::Cpu
}

/// A loaded model.
pub struct Moge {
    device: Device,
    backbone: Vit,
    projections: Vec<Conv2d>,
    neck: Stack,
    normal: Stack,
    points: Stack,
    mask: Stack,
}

impl Moge {
    /// Load the checkpoint in `dir` onto the best device.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_on(dir, best_device())
    }

    pub fn load_on(dir: &Path, device: Device) -> Result<Self> {
        let path = dir.join(WEIGHTS_FILE);
        if !path.is_file() {
            return Err(Error::Missing(dir.to_path_buf()));
        }
        let tensors: HashMap<String, Tensor> = candle_core::pickle::read_all_with_key(&path, Some("model"))?.into_iter().collect();
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
        let projections = (0..TAKE.len())
            .map(|i| Ok(candle_nn::conv2d(HIDDEN, HIDDEN, 1, Conv2dConfig::default(), vb.pp(format!("encoder.output_projections.{i}")))?))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            backbone: Vit::new(vb.pp("encoder.backbone"))?,
            projections,
            // the neck's first level reads the encoder's features with the view-plane coordinates, the others only those
            neck: Stack::new([HIDDEN + 2, 2, 2, 2, 2], None, vb.pp("neck"))?,
            normal: Stack::new(DIMS, Some(3), vb.pp("normal_head"))?,
            points: Stack::new(DIMS, Some(3), vb.pp("points_head"))?,
            mask: Stack::new(DIMS, Some(1), vb.pp("mask_head"))?,
            device,
        })
    }

    /// Analyze an image of `width × height` pixels (`rgb`: sRGB-encoded, 0..1, row-major) with
    /// about `tokens` ViT tokens (clamped to [`TOKENS`]).
    pub fn infer(&self, rgb: &[[f32; 3]], width: usize, height: usize, tokens: usize) -> Result<Geometry> {
        if width < 8 || height < 8 || width > MAX_INPUT_EDGE || height > MAX_INPUT_EDGE || rgb.len() != width * height {
            return Err(Error::Size(width, height));
        }
        let tokens = tokens.clamp(TOKENS.0, TOKENS.1) as f64;
        let aspect = width as f64 / height as f64;
        let rows = ((tokens / aspect).sqrt().round() as usize).max(2);
        let cols = ((tokens * aspect).sqrt().round() as usize).max(2);
        let (in_w, in_h) = (cols * PATCH, rows * PATCH);
        let resized = resize(rgb, width, height, in_w, in_h);
        // planar, normalized
        let mut planes = vec![0f32; 3 * in_w * in_h];
        for (c, plane) in planes.chunks_mut(in_w * in_h).enumerate() {
            let (mean, std) = (MEAN.get(c).copied().unwrap_or(0.5), STD.get(c).copied().unwrap_or(0.25));
            for (v, px) in plane.iter_mut().zip(&resized) {
                let value = px.get(c).copied().filter(|x| x.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
                *v = (value - mean) / std;
            }
        }
        let image = Tensor::from_vec(planes, (1, 3, in_h, in_w), &self.device)?;

        let features = self.backbone.features(&image, rows, cols, &TAKE)?;
        let mut encoded: Option<Tensor> = None;
        for (projection, feature) in self.projections.iter().zip(&features) {
            let projected = projection.forward(feature)?;
            encoded = Some(match encoded {
                Some(sum) => (sum + projected)?,
                None => projected,
            });
        }
        let encoded = encoded.ok_or_else(|| Error::Model("the encoder produced no features".into()))?;

        let mut levels = Vec::with_capacity(5);
        for level in 0..5 {
            let uv = self.view_plane(cols << level, rows << level, aspect)?;
            levels.push(if level == 0 { Tensor::cat(&[&encoded, &uv], 1)? } else { uv });
        }
        let shared = self.neck.forward(&levels)?;
        let last = |stack: &Stack| -> Result<Vec<f32>> {
            let out = stack.forward(&shared)?.pop().ok_or_else(|| Error::Model("a head produced nothing".into()))?;
            Ok(out.flatten_all()?.to_vec1()?)
        };
        let (out_w, out_h) = (cols << 4, rows << 4);
        let n = out_w * out_h;
        let (normal, points, mask) = (last(&self.normal)?, last(&self.points)?, last(&self.mask)?);
        if normal.len() != 3 * n || points.len() != 3 * n || mask.len() != n {
            return Err(Error::Model("a head's output has the wrong size".into()));
        }
        let plane = |data: &[f32], c: usize, i: usize| data.get(c * n + i).copied().unwrap_or(0.0);
        let mut geometry =
            Geometry { width: out_w, height: out_h, normal: Vec::with_capacity(n), depth: Vec::with_capacity(n), surface: Vec::with_capacity(n) };
        for i in 0..n {
            let v = [plane(&normal, 0, i), plane(&normal, 1, i), plane(&normal, 2, i)];
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            geometry.normal.push(if len.is_finite() && len > 1e-6 { v.map(|c| c / len) } else { [0.0, 0.0, -1.0] });
            // the points head predicts log depth ("exp" remapping)
            let log_depth = plane(&points, 2, i);
            geometry.depth.push(if log_depth.is_finite() { log_depth.clamp(-MAX_LOG_DEPTH, MAX_LOG_DEPTH).exp() } else { 1.0 });
            let logit = plane(&mask, 0, i);
            geometry.surface.push(if logit.is_finite() { 1.0 / (1.0 + (-logit).exp()) } else { 0.0 });
        }
        Ok(geometry)
    }

    /// The view-plane coordinates of a `w × h` grid, `[1, 2, h, w]`: u and v run from the
    /// top-left corner at −(aspect, 1) / √(1 + aspect²) to the opposite point at the bottom right,
    /// at pixel centres. They tell the convolutions where in the frame (and of what shape) they are.
    fn view_plane(&self, w: usize, h: usize, aspect: f64) -> Result<Tensor> {
        let diagonal = (1.0 + aspect * aspect).sqrt();
        let (span_x, span_y) = (aspect / diagonal, 1.0 / diagonal);
        let at = |i: usize, n: usize, span: f64| {
            let edge = span * (n as f64 - 1.0) / n as f64;
            if n > 1 { (-edge + 2.0 * edge * i as f64 / (n as f64 - 1.0)) as f32 } else { 0.0 }
        };
        let mut data = Vec::with_capacity(2 * w * h);
        for _ in 0..h {
            data.extend((0..w).map(|x| at(x, w, span_x)));
        }
        for y in 0..h {
            data.extend(std::iter::repeat_n(at(y, h, span_y), w));
        }
        Ok(Tensor::from_vec(data, (1, 2, h, w), &self.device)?)
    }
}

/// The taps of a triangle (antialiased bilinear) resampling of `from` samples to `to`.
fn taps(from: usize, to: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = from as f64 / to.max(1) as f64;
    let support = scale.max(1.0);
    (0..to)
        .map(|i| {
            let centre = (i as f64 + 0.5) * scale;
            let (lo, hi) = ((centre - support).floor().max(0.0) as usize, ((centre + support).ceil() as usize).min(from));
            let mut row: Vec<(usize, f32)> =
                (lo..hi).map(|j| (j, (1.0 - ((j as f64 + 0.5 - centre) / support).abs()).max(0.0) as f32)).filter(|(_, w)| *w > 0.0).collect();
            let total: f32 = row.iter().map(|(_, w)| w).sum();
            if total > 0.0 {
                row.iter_mut().for_each(|(_, w)| *w /= total);
            } else {
                row = vec![((centre as usize).min(from.saturating_sub(1)), 1.0)];
            }
            row
        })
        .collect()
}

/// Resample `src` (`w × h`) to `to_w × to_h`, averaging when shrinking.
fn resize(src: &[[f32; 3]], w: usize, h: usize, to_w: usize, to_h: usize) -> Vec<[f32; 3]> {
    let (xs, ys) = (taps(w, to_w), taps(h, to_h));
    // rows first, then columns
    let mut wide = vec![[0f32; 3]; to_w * h];
    for (row_in, row_out) in src.chunks(w.max(1)).zip(wide.chunks_mut(to_w.max(1))) {
        for (out, x_taps) in row_out.iter_mut().zip(&xs) {
            for (x, weight) in x_taps {
                if let Some(px) = row_in.get(*x) {
                    for (o, c) in out.iter_mut().zip(px) {
                        *o += weight * c;
                    }
                }
            }
        }
    }
    let mut out = vec![[0f32; 3]; to_w * to_h];
    for (row_out, y_taps) in out.chunks_mut(to_w.max(1)).zip(&ys) {
        for (y, weight) in y_taps {
            if let Some(row_in) = wide.get(y * to_w..(y + 1) * to_w) {
                for (o, px) in row_out.iter_mut().zip(row_in) {
                    for (oc, c) in o.iter_mut().zip(px) {
                        *oc += weight * c;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_keeps_a_flat_image_flat_and_averages_when_shrinking() {
        let flat = vec![[0.25f32, 0.5, 0.75]; 40 * 30];
        for (w, h) in [(14, 14), (80, 60), (7, 90)] {
            assert!(resize(&flat, 40, 30, w, h).iter().all(|p| (p[0] - 0.25).abs() < 1e-5 && (p[2] - 0.75).abs() < 1e-5), "{w}×{h}");
        }
        // black and white columns shrink to grey, not to either
        let stripes: Vec<[f32; 3]> = (0..64 * 8).map(|i| if (i % 64) % 2 == 0 { [0.0; 3] } else { [1.0; 3] }).collect();
        assert!(resize(&stripes, 64, 8, 8, 8).iter().all(|p| (p[0] - 0.5).abs() < 0.05));
    }

    #[test]
    fn a_missing_checkpoint_is_an_error_that_names_the_folder() {
        let dir = std::env::temp_dir().join("lightcraft-relief-no-model");
        let error = Moge::load_on(&dir, Device::Cpu).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(error.contains("model.pt") && error.contains("lightcraft-relief-no-model"), "{error}");
    }
}
