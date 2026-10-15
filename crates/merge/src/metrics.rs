//! How sharp a merge is: measures for tests, benchmarks and tuning.
//!
//! - [`detail`]: local high-pass energy of a picture's √luminance (√ so shadows count, as the eye
//!   sees them): the squared difference from a σ = 1 px Gaussian blur, averaged over σ = 2 px.
//! - [`detail_ratio`]: a result's detail against a reference's (an HDR merge against its
//!   reference frame or the scene's truth) over chosen pixels. Below 1 the merge lost detail.
//! - [`MaxDetail`] / [`TextureKept`]: for a focus stack, the result's detail against the most any
//!   aligned frame has at each pixel (what "sharp throughout" could at best keep), split in thirds
//!   by that maximum, so low-contrast texture lost to averaging shows on its own.
//! - [`detail_gain`]: the result's fine detail projected onto a known truth's: blur lowers it, noise
//!   doesn't count.
//! - [`residual`]: how far two aligned frames still are apart, per tile, in pixels (patches
//!   matched as in [`crate::refine`]).

use lightcraft_geom::Homography;
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

use crate::refine;

/// Local high-pass energy of `img`'s √luminance (see the module docs).
pub fn detail(img: &Rgb32f) -> Plane {
    let y = img.map(|c| (0.25 * c[0] + 0.6 * c[1] + 0.15 * c[2]).max(0.0).sqrt());
    detail_of(&y)
}

/// [`detail`] of a plane that is already perceptually encoded.
pub fn detail_of(y: &Plane) -> Plane {
    let b = gaussian(y, 1.0);
    let mut e = y.clone();
    e.data.par_iter_mut().zip(b.data.par_iter()).for_each(|(v, b)| {
        let d = *v - b;
        *v = if d.is_finite() { d * d } else { 0.0 };
    });
    gaussian(&e, 2.0)
}

/// Σ detail(result) / Σ detail(reference) over the pixels `keep` accepts (`x`, `y`); `None`
/// when the sizes differ or the reference has no detail there.
pub fn detail_ratio(result: &Plane, reference: &Plane, keep: impl Fn(usize, usize) -> bool + Sync) -> Option<f64> {
    if result.width != reference.width || result.height != reference.height || result.width == 0 {
        return None;
    }
    let w = result.width;
    let (a, b) = result
        .data
        .par_chunks(w)
        .zip(reference.data.par_chunks(w))
        .enumerate()
        .map(|(y, (ra, rb))| {
            let mut s = (0f64, 0f64);
            for x in 0..w {
                if keep(x, y) {
                    s.0 += ra[x] as f64;
                    s.1 += rb[x] as f64;
                }
            }
            s
        })
        .reduce(|| (0.0, 0.0), |p, q| (p.0 + q.0, p.1 + q.1));
    (b > 0.0).then(|| a / b)
}

/// How much of `truth`'s fine detail `result` carries: the result's high-pass (√luminance minus a
/// σ = 1 px blur) projected onto the truth's, Σ hp(result)·hp(truth) / Σ hp(truth)², over the pixels
/// `keep` accepts. 1 keeps the texture at full contrast; noise, uncorrelated with the truth, doesn't
/// count (unlike [`detail_ratio`]). `None` when the sizes differ or the truth has no detail there.
pub fn detail_gain(result: &Rgb32f, truth: &Rgb32f, keep: impl Fn(usize, usize) -> bool + Sync) -> Option<f64> {
    if result.width != truth.width || result.height != truth.height || result.width == 0 {
        return None;
    }
    let hp = |img: &Rgb32f| {
        let y = img.map(|c| (0.25 * c[0] + 0.6 * c[1] + 0.15 * c[2]).max(0.0).sqrt());
        let b = gaussian(&y, 1.0);
        let mut d = y;
        d.data.par_iter_mut().zip(b.data.par_iter()).for_each(|(v, b)| *v -= b);
        d
    };
    let (r, t) = (hp(result), hp(truth));
    let w = r.width;
    let (rt, tt) = r
        .data
        .par_chunks(w)
        .zip(t.data.par_chunks(w))
        .enumerate()
        .map(|(y, (rr, tr))| {
            let mut s = (0f64, 0f64);
            for x in 0..w {
                let (a, b) = (rr[x] as f64, tr[x] as f64);
                if keep(x, y) && a.is_finite() && b.is_finite() {
                    s.0 += a * b;
                    s.1 += b * b;
                }
            }
            s
        })
        .reduce(|| (0.0, 0.0), |p, q| (p.0 + q.0, p.1 + q.1));
    (tt > 0.0).then(|| rt / tt)
}

/// Detail kept by a merge against the most of it any frame has, overall and in thirds of that
/// maximum (`low`: the third of the pixels with the least, i.e. subtle texture).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureKept {
    pub all: f64,
    pub low: f64,
    pub mid: f64,
    pub high: f64,
}

/// The per-pixel maximum of the frames' [`detail`], added one frame at a time (frames already in
/// the result's geometry; pixels a frame doesn't cover passed as NaN don't count).
#[derive(Clone, Debug)]
pub struct MaxDetail {
    pub max: Plane,
}

impl MaxDetail {
    pub fn new(w: usize, h: usize) -> MaxDetail {
        MaxDetail { max: Plane::new(w, h) }
    }

    /// Add a frame's detail plane.
    pub fn add(&mut self, d: &Plane) {
        if d.width != self.max.width || d.height != self.max.height {
            return;
        }
        self.max.data.par_iter_mut().zip(d.data.par_iter()).for_each(|(m, v)| {
            if v.is_finite() && *v > *m {
                *m = *v;
            }
        });
    }

    /// How much of it `result` (a [`detail`] plane) keeps, over the pixels `keep` accepts.
    pub fn kept(&self, result: &Plane, keep: impl Fn(usize, usize) -> bool + Sync) -> Option<TextureKept> {
        if result.width != self.max.width || result.height != self.max.height || result.width == 0 {
            return None;
        }
        let w = result.width;
        let mut px: Vec<(f32, f32)> = (0..result.data.len())
            .filter(|&i| keep(i % w, i / w))
            .filter_map(|i| Some((*self.max.data.get(i)?, *result.data.get(i)?)))
            .filter(|(m, r)| m.is_finite() && r.is_finite())
            .collect();
        if px.len() < 3 {
            return None;
        }
        px.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ratio = |s: &[(f32, f32)]| {
            let (m, r) = s.iter().fold((0f64, 0f64), |(m, r), p| (m + p.0 as f64, r + p.1 as f64));
            if m > 0.0 { r / m } else { 0.0 }
        };
        let t = px.len() / 3;
        Some(TextureKept { all: ratio(&px), low: ratio(&px[..t]), mid: ratio(&px[t..2 * t]), high: ratio(&px[2 * t..]) })
    }
}

/// How far apart two aligned frames still are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Residual {
    /// Median and 95th percentile of the tiles' shifts (pixels).
    pub median: f64,
    pub p95: f64,
    /// Tiles that could be matched.
    pub tiles: usize,
}

/// The shift between `reference` and `frame` (same geometry; planes from [`refine::luma`]) at each
/// textured tile, within ±3 px; `None` when no tile matches.
pub fn residual(reference: &Plane, frame: &Plane) -> Option<Residual> {
    let sites = refine::sites(reference, 5);
    let mut d: Vec<f64> = sites
        .par_iter()
        .filter_map(|&s| refine::match_patch(reference, s, frame, &Homography::IDENTITY, 3))
        .map(|m| (m.offset.x * m.offset.x + m.offset.y * m.offset.y).sqrt())
        .collect();
    if d.is_empty() {
        return None;
    }
    d.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| d.get(((d.len() - 1) as f64 * q).round() as usize).copied().unwrap_or(0.0);
    Some(Residual { median: at(0.5), p95: at(0.95), tiles: d.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(w: usize, h: usize, dx: f32, blur: f32) -> Rgb32f {
        let img = Rgb32f::from_fn(w, h, |x, y| {
            let (x, y) = (x as f32 + dx, y as f32);
            [0.2 + 0.1 * (x * 0.9).sin() * (y * 0.7).cos() + 0.05 * (x * 0.31 + y * 0.43).sin(); 3]
        });
        if blur > 0.0 { gaussian(&img, blur) } else { img }
    }

    #[test]
    fn blur_lowers_the_detail_ratio() {
        let sharp = tex(200, 150, 0.0, 0.0);
        let soft = tex(200, 150, 0.0, 1.0);
        let r = detail_ratio(&detail(&soft), &detail(&sharp), |_, _| true).unwrap();
        assert!(r < 0.5, "{r}");
        assert!((detail_ratio(&detail(&sharp), &detail(&sharp), |_, _| true).unwrap() - 1.0).abs() < 1e-9);
        assert!(detail_ratio(&detail(&sharp), &Plane::new(3, 3), |_, _| true).is_none());
    }

    #[test]
    fn detail_gain_ignores_noise_and_sees_blur() {
        let sharp = tex(200, 150, 0.0, 0.0);
        let mut noisy = sharp.clone();
        let mut seed = 7u64;
        for p in noisy.data.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let n = ((seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.05;
            *p = p.map(|v| v + n);
        }
        let g = detail_gain(&noisy, &sharp, |_, _| true).unwrap();
        assert!((g - 1.0).abs() < 0.05, "noise: {g}");
        assert!(detail_gain(&tex(200, 150, 0.0, 1.0), &sharp, |_, _| true).unwrap() < 0.7, "an amplitude: blur that halves the energy leaves √½");
        assert!(detail_gain(&sharp, &Rgb32f::new(2, 2), |_, _| true).is_none());
    }

    #[test]
    fn texture_kept_against_the_sharpest_frame() {
        let sharp = detail(&tex(120, 90, 0.0, 0.0));
        let soft = detail(&tex(120, 90, 0.0, 1.5));
        let mut m = MaxDetail::new(120, 90);
        m.add(&soft);
        m.add(&sharp);
        m.add(&Plane::new(5, 5));
        let all = m.kept(&sharp, |_, _| true).unwrap();
        assert!((all.all - 1.0).abs() < 1e-9 && (all.low - 1.0).abs() < 1e-9);
        assert!(m.kept(&soft, |_, _| true).unwrap().all < 0.5);
        assert!(m.kept(&soft, |_, _| false).is_none());
    }

    #[test]
    fn residual_finds_a_known_shift() {
        let a = refine::luma(
            &Rgb32f::from_fn(600, 400, |x, y| [0.1 + 0.05 * ((x as f32 * 0.37).sin() + (y as f32 * 0.23 + x as f32 * 0.05).cos()); 3]),
            1.0,
            1.0,
        );
        let b = refine::luma(
            &Rgb32f::from_fn(600, 400, |x, y| {
                let x = x as f32 + 1.25;
                [0.1 + 0.05 * ((x * 0.37).sin() + (y as f32 * 0.23 + x * 0.05).cos()); 3]
            }),
            1.0,
            1.0,
        );
        let r = residual(&a, &b).unwrap();
        assert!((r.median - 1.25).abs() < 0.1, "{r:?}");
        assert!(residual(&a, &a).unwrap().p95 < 0.05);
        assert!(residual(&Plane::new(10, 10), &Plane::new(10, 10)).is_none());
    }
}
