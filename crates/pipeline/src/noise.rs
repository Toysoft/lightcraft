//! V3 luminance noise reduction. Estimate noise on the whole source, then subtract its
//! variance from local variance in sqrt-luminance. Unlike fixed-epsilon log filtering,
//! this leaves high-contrast structure largely alone and adapts to the source's noise.
//! This is an original spatial filter, not a camera-specific noise model or AI inference.

use lightcraft_color::luminance_2020;
use lightcraft_develop::{DevelopSettings, Process};
use lightcraft_raster::{Rgb32f, blur::gaussian, par_join};

/// Whether this rendering needs a source noise estimate.
pub fn enabled(s: &DevelopSettings) -> bool {
    s.process.process() >= Process::V3 && s.section_enabled("detail") && s.detail.nr_luminance > 0.0
}

/// Robust, bounded estimate in sqrt(scene-linear luminance), before exposure. Sampling is
/// tied to the source, not the output window, so panning never changes the noise strength.
/// The lower fifth of local MADs favors smooth patches over texture. This cannot reliably
/// distinguish noise from texture in an image with no smooth regions; Amount remains editable.
pub fn estimate(src: &Rgb32f, wb: Option<[[f32; 3]; 3]>) -> f32 {
    let (w, h) = (src.width, src.height);
    if w < 12 || h < 12 || w.checked_mul(h) != Some(src.data.len()) {
        return 0.0;
    }
    let sample = |x: usize, y: usize| -> Option<f32> {
        let mut rgb = *src.data.get(y.checked_mul(w)?.checked_add(x)?)?;
        if let Some(m) = wb {
            rgb = m.map(|r| (r[0] * rgb[0] + r[1] * rgb[1] + r[2] * rgb[2]).max(0.0));
        }
        let l = luminance_2020(rgb);
        l.is_finite().then(|| l.max(0.0).sqrt())
    };
    let (pw, ph) = ((w - 4).min(32), (h - 4).min(32));
    let mut estimates = Vec::with_capacity(192);
    for ty in 0..12 {
        for tx in 0..16 {
            let x0 = 2 + (w - pw - 4) / 15 * tx;
            let y0 = 2 + (h - ph - 4) / 11 * ty;
            let mut residuals = Vec::with_capacity(64);
            let mut mean = 0.0;
            for y in (y0..y0 + ph).step_by(4) {
                for x in (x0..x0 + pw).step_by(4) {
                    let cross = || -> Option<(f32, f32)> {
                        let c = sample(x, y)?;
                        let m = (sample(x - 2, y)? + sample(x + 2, y)? + sample(x, y - 2)? + sample(x, y + 2)?) * 0.25;
                        Some((c, (c - m).abs()))
                    };
                    if let Some((c, r)) = cross() {
                        residuals.push(r);
                        mean += c * c;
                    }
                }
            }
            if residuals.len() < 4 {
                continue;
            }
            mean /= residuals.len() as f32;
            // Do not use absolute brightness cutoffs: changing exposure or source gain
            // must scale the estimate, not select a different set of patches.
            if !mean.is_finite() || mean <= 0.0 {
                continue;
            }
            residuals.sort_by(f32::total_cmp);
            if let Some(mad) = residuals.get(residuals.len() / 2) {
                // Median absolute Gaussian residual / normal median; a centre minus four
                // equally weighted neighbours has variance 1.25 times independent noise.
                estimates.push(mad / (0.67449 * 1.25f32.sqrt()));
            }
        }
    }
    estimates.sort_by(f32::total_cmp);
    estimates.get(estimates.len() / 5).copied().unwrap_or(0.0).clamp(0.0, 0.25)
}

/// Shared CPU/GPU parameters. Radii follow the full output, never a render window.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub sigma: f32,
    pub variance: f32,
}

pub fn params(s: &DevelopSettings, src_long: usize, out_long: usize, noise: f32) -> Option<Params> {
    if !enabled(s) || !noise.is_finite() || noise <= 0.0 || !s.detail.nr_detail.is_finite() || !s.detail.nr_luminance.is_finite() {
        return None;
    }
    let scale = (out_long as f32 / src_long.max(1) as f32).clamp(0.05, 1.0);
    let amount = (s.detail.nr_luminance / 100.0).clamp(0.0, 1.0) as f32;
    let retain = 1.5 - (s.detail.nr_detail / 100.0).clamp(0.0, 1.0) as f32;
    Some(Params { sigma: (1.4 * scale).max(0.9), variance: (noise * scale * 1.15).powi(2) * 4.0 * amount * retain })
}

/// Two short Gaussian passes reduce isolated coefficient variations without large halos.
pub(crate) fn reduce(img: &mut Rgb32f, p: Params) {
    let l = img.map(|rgb| luminance_2020(rgb).max(1e-7).sqrt());
    let blur = |v: &lightcraft_raster::Plane, sigma| gaussian(&gaussian(v, sigma), sigma);
    let (mean, corr) = par_join(|| blur(&l, p.sigma), || blur(&l.map(|v| v * v), p.sigma));
    let a = corr.zip_map(&mean, |c, m| {
        let variance = (c - m * m).max(0.0);
        (1.0 - p.variance / variance.max(1e-12)).max(0.0)
    });
    let b = mean.zip_map(&a, |m, a| m * (1.0 - a));
    let (a, b) = par_join(|| blur(&a, 0.7), || blur(&b, 0.7));
    for (((rgb, l), a), b) in img.data.iter_mut().zip(&l.data).zip(&a.data).zip(&b.data) {
        let f = a * l + b;
        let gain = (f * f / (l * l).max(1e-7)).clamp(0.05, 20.0);
        *rgb = rgb.map(|v| v * gain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::ProcessVersion;

    // Deterministic Gaussian noise added in the same sqrt-luminance domain as the signal.
    fn normal(seed: &mut u32) -> f32 {
        let mut uniform = || {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            (*seed as f64 + 0.5) / (u32::MAX as f64 + 1.0)
        };
        ((-2.0 * uniform().ln()).sqrt() * (std::f64::consts::TAU * uniform()).cos()) as f32
    }

    fn scene(sigma: f32) -> (Rgb32f, Rgb32f) {
        let clean = Rgb32f::from_fn(512, 384, |x, y| {
            let base = if x < 256 { 0.28f32 } else { 0.65f32 };
            let texture = if y > 192 && x > 32 && x < 480 { 0.045 * (x as f32 * std::f32::consts::TAU / 8.0).sin() } else { 0.0 };
            [(base + texture).powi(2); 3]
        });
        let mut noisy = clean.clone();
        let mut seed = 13287;
        for px in &mut noisy.data {
            let l = (px[0].sqrt() + sigma * normal(&mut seed)).max(0.0);
            *px = [l * l; 3];
        }
        (clean, noisy)
    }

    #[test]
    fn estimate_tracks_noise_and_ignores_a_clean_edge() {
        for sigma in [0.0, 0.004, 0.012, 0.03] {
            let (_, img) = scene(sigma);
            let estimated = estimate(&img, None);
            assert!((estimated - sigma).abs() < sigma * 0.3 + 1e-6, "sigma {sigma}, estimate {estimated}");
        }
        assert_eq!(estimate(&Rgb32f::new(0, 0), None), 0.0);
        assert_eq!(estimate(&Rgb32f::filled(32, 32, [f32::NAN; 3]), None), 0.0);
        assert_eq!(estimate(&Rgb32f { width: usize::MAX, height: 2, data: vec![] }, None), 0.0);
        let (_, noisy) = scene(0.012);
        let baseline = estimate(&noisy, None);
        for gain in [0.04f32, 4.0] {
            let scaled = noisy.map(|p| p.map(|v| v * gain));
            let measured = estimate(&scaled, None);
            assert!((measured / gain.sqrt() - baseline).abs() < 1e-5, "source-gain consistency");
        }
    }

    #[test]
    fn adaptive_filter_removes_noise_without_erasing_repeating_detail() {
        let (clean, mut noisy) = scene(0.012);
        let original = noisy.clone();
        let settings = DevelopSettings::for_raw(6500.0, 0.0);
        let noise = estimate(&noisy, None);
        reduce(&mut noisy, params(&settings, 512, 512, noise).unwrap());
        let error = |img: &Rgb32f, ys: std::ops::Range<usize>| {
            let mut error = 0.0;
            for y in ys {
                for x in 40..220 {
                    let i = y * 512 + x;
                    error += (img.data[i][0].sqrt() - clean.data[i][0].sqrt()).powi(2);
                }
            }
            error
        };
        assert!(error(&noisy, 32..160) < error(&original, 32..160) * 0.25, "flat-region noise must fall substantially");
        assert!(error(&noisy, 220..350) < error(&original, 220..350) * 0.9, "textured-region error must improve too");
        let amplitude = |img: &Rgb32f| {
            (220..350)
                .flat_map(|y| (40..220).map(move |x| (x, y)))
                .map(|(x, y)| img.data[y * 512 + x][0].sqrt() * (x as f32 * std::f32::consts::TAU / 8.0).sin())
                .sum::<f32>()
        };
        let retained = amplitude(&noisy) / amplitude(&clean);
        assert!(retained > 0.85, "preserve fine repeating structure: retained {retained}");
        for x in [250, 255, 256, 261] {
            let mean = (40..160).map(|y| noisy.data[y * 512 + x][0].sqrt()).sum::<f32>() / 120.0;
            assert!((mean - clean.data[100 * 512 + x][0].sqrt()).abs() < 0.008, "edge/halo at {x}: {mean}");
        }
    }

    #[test]
    fn zero_amount_and_legacy_processes_do_not_use_adaptive_nr() {
        let mut s = DevelopSettings::for_raw(6500.0, 0.0);
        for process in [1, 2] {
            s.process = ProcessVersion(process);
            assert!(params(&s, 512, 512, 0.01).is_none());
        }
        s.process = ProcessVersion(3);
        s.detail.nr_luminance = 0.0;
        assert!(params(&s, 512, 512, 0.01).is_none());
        s.detail.nr_luminance = 25.0;
        s.set_section_enabled("detail", false);
        assert!(params(&s, 512, 512, 0.01).is_none());
    }
}
