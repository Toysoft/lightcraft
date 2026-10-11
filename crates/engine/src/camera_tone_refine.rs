//! Higher-resolution Nikon tone fitting. Averaging a textured scene before fitting a
//! nonlinear curve loses its dark/bright tails. Keep the established colour model and
//! use a fixed, finer proxy for tone only. Matched structure validates the change without
//! confusing the JPEG's lens geometry with a colour/tone error.

use super::{CameraLook, CameraTone, CameraTransform, HueSat, RawImage, Rgb32f, ToneMap, displayed, luminance_2020, proxies};

const EDGE: usize = 768;
const MARGIN: usize = 20;

pub(super) fn fit(raw: &RawImage, bytes: &[u8], transform: &CameraTransform, look: &CameraLook) -> Option<CameraTone> {
    let (mut sensor, reference, clipped) = proxies(raw, bytes, transform, EDGE)?;
    let hue = look.hue_sat.as_ref().and_then(HueSat::new);
    sensor.map_in_place(|p| {
        let p = look.matrix.apply_f32(p);
        hue.as_ref().map_or(p, |h| h.apply(p)).map(|v| v.max(0.0))
    });
    fit_images(&sensor, &reference, &clipped, &look.tone)
}

fn gamma(y: f32) -> f32 {
    y.max(0.0).powf(1.0 / 2.2)
}

fn at(values: &[f32], w: usize, x: usize, y: usize) -> Option<f32> {
    values.get(y.checked_mul(w)?.checked_add(x)?).copied().filter(|v| v.is_finite())
}

/// Indices of matching central pixels. The surrounding patch chooses correspondence;
/// the central 3×3 is excluded from that choice and subsequently from tone training.
fn correspondences(scene: &[f32], reference: &[f32], w: usize, h: usize, legacy: &CameraTone) -> Vec<(usize, usize)> {
    let source: Vec<_> = scene.iter().map(|y| gamma(legacy.apply(*y))).collect();
    let reference: Vec<_> = reference.iter().map(|y| gamma(*y)).collect();
    let mut pairs = Vec::new();
    for row in 0..12 {
        for col in 0..16 {
            let x = MARGIN + col * (w - 2 * MARGIN - 1) / 15;
            let y = MARGIN + row * (h - 2 * MARGIN - 1) / 11;
            let mut patch = Vec::with_capacity(112);
            for dy in -5isize..=5 {
                for dx in -5isize..=5 {
                    if dx.abs() <= 1 && dy.abs() <= 1 {
                        continue;
                    }
                    if let Some(v) = x.checked_add_signed(dx).zip(y.checked_add_signed(dy)).and_then(|(x, y)| at(&reference, w, x, y)) {
                        patch.push((dx, dy, v));
                    }
                }
            }
            if patch.len() != 112 {
                continue;
            }
            let n = patch.len() as f64;
            let mean: f64 = patch.iter().map(|p| f64::from(p.2)).sum::<f64>() / n;
            let variance: f64 = patch.iter().map(|p| (f64::from(p.2) - mean).powi(2)).sum();
            if variance / n < 0.0001 {
                continue;
            }
            let mut best = (0.95, 0isize, 0isize);
            for oy in -14isize..=14 {
                for ox in -14isize..=14 {
                    let score = || -> Option<f64> {
                        let (mut sum, mut square, mut dot) = (0.0, 0.0, 0.0);
                        for &(dx, dy, target) in &patch {
                            let z = f64::from(at(&source, w, x.checked_add_signed(ox + dx)?, y.checked_add_signed(oy + dy)?)?);
                            sum += z;
                            square += z * z;
                            dot += z * (f64::from(target) - mean);
                        }
                        let v = square - sum * sum / n;
                        (v > 1e-10).then(|| dot / (v * variance).sqrt())
                    };
                    if let Some(score) = score().filter(|v| *v > best.0) {
                        best = (score, ox, oy);
                    }
                }
            }
            if best.0 <= 0.95 {
                continue;
            }
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let indices = || -> Option<(usize, usize)> {
                        let src = y.checked_add_signed(best.2 + dy)?.checked_mul(w)?.checked_add(x.checked_add_signed(best.1 + dx)?)?;
                        let dst = y.checked_add_signed(dy)?.checked_mul(w)?.checked_add(x.checked_add_signed(dx)?)?;
                        Some((src, dst))
                    };
                    if let Some(p) = indices() {
                        pairs.push(p);
                    }
                }
            }
        }
    }
    pairs
}

fn fit_images(sensor: &Rgb32f, reference: &Rgb32f, clipped: &[bool], legacy: &CameraTone) -> Option<CameraTone> {
    let (w, h) = (sensor.width, sensor.height);
    if w < 64
        || h < 64
        || w > EDGE
        || h > EDGE
        || w.checked_mul(h)? != sensor.data.len()
        || (w, h) != (reference.width, reference.height)
        || reference.data.len() != sensor.data.len()
        || clipped.len() != sensor.data.len()
    {
        return None;
    }
    let scene: Vec<_> = sensor.data.iter().map(|p| luminance_2020(*p)).collect();
    let target: Vec<_> = reference.data.iter().map(|p| luminance_2020(*p)).collect();
    let pairs = correspondences(&scene, &target, w, h, legacy);
    if pairs.len() < 24 * 9 {
        return None;
    }
    let mut excluded = clipped.to_vec();
    for &(a, b) in &pairs {
        *excluded.get_mut(a)? = true;
        *excluded.get_mut(b)? = true;
    }
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for ((x, y), skip) in scene.iter().zip(&target).zip(&excluded) {
        if !skip && x.is_finite() && *x > 0.0 && y.is_finite() && (0.0..1.0).contains(y) {
            xs.push(*x);
            ys.push(*y);
        }
    }
    if xs.len() < 1024 {
        return None;
    }
    xs.sort_by(f32::total_cmp);
    ys.sort_by(f32::total_cmp);
    let mut knots = [[0.0; 2]; 32];
    for (i, knot) in knots.iter_mut().enumerate() {
        // More knots near the tails, which the 96 px fit poorly resolves.
        let q = 1.0 / (1.0 + (-(i as f32 / 31.0 * 2.0 - 1.0) * 6.0).exp());
        let idx = ((xs.len() - 1) as f32 * q) as usize;
        *knot = [*xs.get(idx)?, *ys.get(idx)?];
    }
    let candidate = CameraTone::new(knots)?.with_chroma(*legacy.chroma())?;
    let old_map = ToneMap::camera(legacy, 0.0, 0.0, 0.0);
    let new_map = ToneMap::camera(&candidate, 0.0, 0.0, 0.0);
    let (mut before, mut after, mut samples) = (0.0, 0.0, 0usize);
    for (a, b) in pairs {
        if *clipped.get(a)? || *clipped.get(b)? {
            continue;
        }
        let src = sensor.data.get(a)?.map(f64::from);
        let target = reference.data.get(b)?;
        if src.iter().any(|v| !v.is_finite()) || target.iter().any(|v| !v.is_finite()) {
            continue;
        }
        let old = displayed(src, &old_map);
        let new = displayed(src, &new_map);
        for ((a, b), t) in old.into_iter().zip(new).zip(target) {
            let t = f64::from(gamma(*t));
            before += (a.max(0.0).powf(1.0 / 2.2) - t).powi(2);
            after += (b.max(0.0).powf(1.0 / 2.2) - t).powi(2);
            samples += 1;
        }
    }
    let accepted = samples >= 24 * 9 * 3 && after.is_finite() && before.is_finite() && after < before * 0.98;
    if lightcraft_pipeline::profiling() {
        let n = samples.max(1) as f64;
        eprintln!(
            "[profile] Nikon refined tone: {} held-out channels RMS {:.5} -> {:.5}, accepted {accepted}",
            samples,
            (before / n).sqrt(),
            (after / n).sqrt()
        );
    }
    accepted.then(|| legacy.with_refined_knots(knots)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(power: f32) -> CameraTone {
        CameraTone::new(std::array::from_fn(|i| {
            let x = 0.0005 * 1.27f32.powi(i as i32);
            [x, x.powf(power)]
        }))
        .unwrap()
    }

    #[test]
    fn refines_tone_without_changing_the_legacy_curve() {
        let sensor = Rgb32f::from_fn(256, 192, |x, y| {
            let v = 0.04 + 0.38 * x as f32 / 256.0 + 0.02 * y as f32 / 192.0 + 0.045 * (x as f32 * 0.18).sin() * (y as f32 * 0.2).sin();
            [v; 3]
        });
        let reference = sensor.map(|p| p.map(|v| v.powf(1.3)));
        let legacy = curve(1.05);
        let refined = fit_images(&sensor, &reference, &vec![false; sensor.len()], &legacy).expect("tone improvement");
        assert_eq!(refined.knots(), legacy.knots());
        for x in [0.03, 0.07, 0.15, 0.3] {
            assert!((refined.refined().apply(x) - x.powf(1.3)).abs() < 0.006);
        }
    }

    #[test]
    fn does_not_refine_flat_unrelated_or_invalid_inputs() {
        let flat = Rgb32f::filled(128, 96, [0.2; 3]);
        let curve = curve(1.05);
        assert!(fit_images(&flat, &flat, &vec![false; flat.len()], &curve).is_none());
        let empty = Rgb32f::new(0, 0);
        assert!(fit_images(&empty, &empty, &[], &curve).is_none());
        let invalid = Rgb32f { width: usize::MAX, height: 64, data: vec![] };
        assert!(fit_images(&invalid, &flat, &[], &curve).is_none());
        let noise = |seed: usize| {
            Rgb32f::from_fn(128, 96, |x, y| {
                let mut n = ((x + y * 128) as u32).wrapping_mul(seed as u32);
                n = (n ^ (n >> 16)).wrapping_mul(0x7feb352d);
                n = (n ^ (n >> 15)).wrapping_mul(0x846ca68b);
                [(n & 65535) as f32 / 131072.0; 3]
            })
        };
        assert!(fit_images(&noise(17), &noise(137), &vec![false; flat.len()], &curve).is_none());
    }
}
