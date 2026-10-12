//! The haze map of process V3's Dehaze (issue #1237): a dark-channel-prior estimate of how much of
//! each pixel is haze, computed once per image on a fixed-size proxy of the whole frame.
//!
//! Every render of the photo (any preview size, a zoomed window, the export, the GPU) samples the
//! same map by frame-normalised coordinates, so they all dehaze alike and none needs a margin or a
//! mid-render airlight readback.
//!
//! Built on a proxy of the whole output frame, long edge `P = min(`[`PROXY_EDGE`]`, native)`, after
//! white balance (before exposure), in the gamma encoding `e = lin^(1/2.2)` (scale-invariant: the
//! map doesn't change with Exposure):
//! 1. airlight `A`: the brightest (luminance) pixel among the top [`AIR_TOP`] of the patch dark
//!    channel, pulled [`AIR_NEUTRAL`] of the way toward neutral and floored at [`AIR_FLOOR`]
//!    (`A'`);
//! 2. normalised dark `g = min_c(e_c / A'_c)` (0..[`G_MAX`]), also the guide;
//! 3. patch dark channel `d = minfilter(g, `[`PATCH`]`·P)` ([`lightcraft_raster::morph`]);
//! 4. guided refinement of `d` by `g` ([`GUIDE_RADIUS`]`·P`, [`GUIDE_EPS`]), keeping the
//!    coefficients `a`, `b` at proxy resolution: a full-resolution pixel's haze is
//!    `d̃ = clamp(a·g_px + b, 0, 1)` with its own `g_px`, so edges stay sharp at any size;
//! 5. an exposure-compensation LUT ([`COMP_N`] amounts): how much the recovery darkens the median
//!    at each amount, so Dehaze clears haze without making the photo darker.
//!
//! [`recover`] is the per-pixel V3 Dehaze both renderers apply with it.

use std::sync::{Arc, Mutex};

use lightcraft_color::luminance_2020;
use lightcraft_develop::{DevelopSettings, Process};
use lightcraft_raster::{Plane, Rgb32f};

use crate::SourceInfo;
use crate::geometry::Frame;

/// Long edge (px) of the proxy the map is built on (smaller frames use their own size).
pub const PROXY_EDGE: usize = 1024;
/// Gamma of the encoding the map works in (`e = lin^(1/GAMMA)`).
pub const GAMMA: f32 = 2.2;
/// Share of the patch dark channel (its brightest end) the airlight is picked from.
pub const AIR_TOP: f32 = 0.001;
/// How far the airlight is pulled toward a neutral of its own luminance (0 = as picked).
pub const AIR_NEUTRAL: f32 = 0.5;
/// Lowest airlight channel (encoded): no division by zero, no runaway gain on dark photos.
pub const AIR_FLOOR: f32 = 0.05;
/// Upper clamp of the normalised dark `g`.
pub const G_MAX: f32 = 1.5;
/// Patch (min filter) radius as a fraction of the proxy's long edge.
pub const PATCH: f32 = 0.012;
/// Guided refinement radius as a fraction of the proxy's long edge (Gaussian windows with the
/// spread of a box of this radius: `σ = r/√3`).
pub const GUIDE_RADIUS: f32 = 0.035;
/// Guided refinement epsilon (in units of `g²`).
pub const GUIDE_EPS: f32 = 1e-3;
/// Entries of the exposure-compensation LUT: amounts `0.05, 0.10, …, 1.0`.
pub const COMP_N: usize = 20;
/// Largest exposure compensation before the [`COMP_POWER`] (EV).
pub const COMP_MAX_EV: f32 = 1.4;
/// Share (as a power) of the measured median darkening the compensation gives back.
pub const COMP_POWER: f32 = 0.55;
/// The compensation LUT measures every `COMP_STEP`-th proxy pixel.
pub const COMP_STEP: usize = 7;

/// Haze strength at +100: `ω = OMEGA·k^OMEGA_POWER`.
pub const OMEGA: f32 = 0.95;
pub const OMEGA_POWER: f32 = 0.6;
/// Lowest transmission at +100 (`t ≥ lerp(1, T_FLOOR, k)`): bounds the gain at `1/T_FLOOR`.
pub const T_FLOOR: f32 = 0.25;
/// Sky tolerance: pixels within this of the airlight (encoded, per channel, relative to its brightest
/// channel) are recovered less.
pub const SKY_K: f32 = 0.15;
/// Share of the input's chromaticity blended back after recovery (keeps colours from swinging).
pub const CHROMA_KEEP: f32 = 0.3;
/// Negative Dehaze: veil strength at −100 and the share of it that doesn't depend on the haze.
pub const VEIL: f32 = 0.65;
pub const VEIL_BASE: f32 = 0.4;

/// The haze map of one photo (see the module docs). Frame-normalised: [`Self::coeffs`] takes
/// coordinates in 0..1 over the whole output frame.
#[derive(Clone, Debug, PartialEq)]
pub struct HazeMap {
    /// Guided-filter coefficients at proxy resolution: `d̃ = clamp(a·g + b, 0, 1)`.
    pub a: Plane,
    pub b: Plane,
    /// Airlight `A'` in the encoding (`lin^(1/GAMMA)`), every channel ≥ [`AIR_FLOOR`].
    pub air: [f32; 3],
    /// [`Self::air`] in linear light (pre-exposure).
    pub air_lin: [f32; 3],
    /// Exposure compensation (a gain ≥ 1) at amounts `0.05·(i+1)`.
    pub comp: [f32; COMP_N],
}

/// `lin^(1/GAMMA)`; non-finite and negative values encode as 0.
#[inline]
pub fn encode(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v.powf(1.0 / GAMMA) } else { 0.0 }
}

/// Inverse of [`encode`].
#[inline]
pub fn decode(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v.powf(GAMMA) } else { 0.0 }
}

fn encode3(c: [f32; 3]) -> [f32; 3] {
    c.map(encode)
}

/// Normalised dark of an encoded pixel `e` against airlight `air`: `min_c(e_c / air_c)` clamped to
/// 0..[`G_MAX`] (0 for NaN).
#[inline]
pub fn norm_dark(e: [f32; 3], air: [f32; 3]) -> f32 {
    let g = (e[0] / air[0].max(AIR_FLOOR)).min(e[1] / air[1].max(AIR_FLOOR)).min(e[2] / air[2].max(AIR_FLOOR));
    if g.is_finite() { g.clamp(0.0, G_MAX) } else { 0.0 }
}

impl HazeMap {
    /// The map of `img`: the white-balanced (linear, pre-exposure) proxy of the whole frame.
    pub fn build(img: &Rgb32f) -> HazeMap {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 || img.data.len() != w.saturating_mul(h) {
            return HazeMap::empty();
        }
        let long = w.max(h) as f32;
        let r = ((PATCH * long).round() as usize).max(1);
        let e = img.map(encode3);
        // 1. airlight
        let dark = crate::timed("haze dark", || lightcraft_raster::morph::min_filter(&e.map(crate::local::dark_of), r));
        let picked = crate::timed("haze air", || brightest_of_haziest(&e, &dark)).unwrap_or([1.0; 3]);
        let y = luminance_2020(picked);
        let air = picked.map(|c| {
            let v = c + (y - c) * AIR_NEUTRAL;
            if v.is_finite() { v.max(AIR_FLOOR) } else { 1.0 }
        });
        // 2.-4. normalised dark, patch minimum, guided refinement
        let g = e.map(|p| norm_dark(p, air));
        let d = lightcraft_raster::morph::min_filter(&g, r);
        let sigma = (GUIDE_RADIUS * long / 3f32.sqrt()).max(1.0);
        let (a, b) = crate::timed("haze guided", || crate::masks::guided_cross_coeffs(&g, &d, sigma, GUIDE_EPS));
        let sane = |p: Plane| p.map(|v| if v.is_finite() { v } else { 0.0 });
        let mut map = HazeMap { a: sane(a), b: sane(b), air, air_lin: air.map(decode), comp: [1.0; COMP_N] };
        // 5. exposure compensation
        map.comp = crate::timed("haze comp", || map.measure_comp(img, &e, &g));
        map
    }

    /// A map that finds no haze anywhere (for an empty image).
    fn empty() -> HazeMap {
        HazeMap { a: Plane::new(1, 1), b: Plane::new(1, 1), air: [1.0; 3], air_lin: [1.0; 3], comp: [1.0; COMP_N] }
    }

    /// Proxy size (px).
    pub fn size(&self) -> (usize, usize) {
        (self.a.width, self.a.height)
    }

    /// The coefficients `(a, b)` at frame-normalised point `(nx, ny)` (0..1 over the whole output
    /// frame; outside, the edge's), bilinear over proxy pixel centres.
    pub fn coeffs(&self, nx: f32, ny: f32) -> (f32, f32) {
        let (w, h) = self.size();
        if w == 0 || h == 0 {
            return (0.0, 0.0);
        }
        let axis = |n: f32, len: usize| {
            let n = if n.is_finite() { n } else { 0.5 };
            let p = (n * len as f32 - 0.5).clamp(0.0, (len - 1) as f32);
            let i0 = p.floor() as usize;
            (i0, (i0 + 1).min(len - 1), p - i0 as f32)
        };
        let (x0, x1, fx) = axis(nx, w);
        let (y0, y1, fy) = axis(ny, h);
        let at = |p: &Plane, x: usize, y: usize| p.data.get(y * w + x).copied().unwrap_or(0.0);
        let bil = |p: &Plane| {
            let top = at(p, x0, y0) + (at(p, x1, y0) - at(p, x0, y0)) * fx;
            let bot = at(p, x0, y1) + (at(p, x1, y1) - at(p, x0, y1)) * fx;
            top + (bot - top) * fy
        };
        (bil(&self.a), bil(&self.b))
    }

    /// Haze `d̃` (0..1) of a pixel with linear (pre-exposure) colour `lin` at frame-normalised
    /// `(nx, ny)`.
    pub fn dark(&self, nx: f32, ny: f32, lin: [f32; 3]) -> f32 {
        let (a, b) = self.coeffs(nx, ny);
        self.dark_with(a, b, lin)
    }

    /// [`Self::dark`] with the coefficients already sampled.
    pub fn dark_with(&self, a: f32, b: f32, lin: [f32; 3]) -> f32 {
        let d = a * norm_dark(encode3(lin), self.air) + b;
        if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 }
    }

    /// Exposure compensation (gain ≥ 1) at amount `k` (linear between the LUT's entries; 1 at 0 and
    /// for negative amounts).
    pub fn comp(&self, k: f32) -> f32 {
        if k.is_nan() || k <= 0.0 {
            return 1.0;
        }
        let f = (k.min(1.0) * COMP_N as f32).clamp(0.0, COMP_N as f32);
        let i = f.floor() as usize;
        let at = |j: usize| if j == 0 { 1.0 } else { self.comp.get(j - 1).copied().unwrap_or(1.0) };
        let (lo, hi) = (at(i), at((i + 1).min(COMP_N)));
        lo + (hi - lo) * (f - i as f32)
    }

    /// The compensation LUT: for each amount, the ratio of the median luminance before and after
    /// the recovery (every [`COMP_STEP`]-th proxy pixel), clamped to `1..2^COMP_MAX_EV` and raised
    /// to [`COMP_POWER`].
    fn measure_comp(&self, img: &Rgb32f, e: &Rgb32f, g: &Plane) -> [f32; COMP_N] {
        let samples: Vec<([f32; 3], f32, f32)> = img
            .data
            .iter()
            .zip(&e.data)
            .zip(g.data.iter().zip(self.a.data.iter().zip(&self.b.data)))
            .step_by(COMP_STEP)
            .map(|((c, ev), (gv, (a, b)))| {
                (c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 }), (a * gv + b).clamp(0.0, 1.0), sky_distance(self, *ev))
            })
            .collect();
        let before = median(samples.iter().map(|(c, ..)| luminance_2020(*c)).collect());
        let max = 2f32.powf(COMP_MAX_EV);
        let mut lut = [1.0f32; COMP_N];
        // (one amount per task: each is a pass over the samples and a median)
        lightcraft_raster::par_rows(&mut lut, 1, |i, out| {
            let k = (i + 1) as f32 / COMP_N as f32;
            let after = median(samples.iter().map(|(c, d, m)| luminance_2020(recover_with(self, *c, 1.0, *d, k, || *m))).collect());
            let ratio = before / after;
            let ratio = if ratio.is_finite() {
                ratio
            } else if before > 0.0 {
                max
            } else {
                1.0
            };
            for o in out.iter_mut() {
                *o = ratio.clamp(1.0, max).powf(COMP_POWER);
            }
        });
        lut
    }
}

/// The colour of the brightest pixel (by luminance of `e`) among those whose patch dark channel
/// is in its top [`AIR_TOP`]. Ties go to the first. `None` for an empty image.
fn brightest_of_haziest(e: &Rgb32f, dark: &Plane) -> Option<[f32; 3]> {
    let mut v: Vec<f32> = dark.data.iter().map(|x| if x.is_finite() { *x } else { 0.0 }).collect();
    if v.is_empty() {
        return None;
    }
    let k = (((v.len() as f32) * (1.0 - AIR_TOP)) as usize).min(v.len() - 1);
    let (_, thr, _) = v.select_nth_unstable_by(k, f32::total_cmp);
    let thr = *thr;
    let mut best: Option<(f32, [f32; 3])> = None;
    for (p, d) in e.data.iter().zip(&dark.data) {
        if *d >= thr {
            let y = luminance_2020(*p);
            if best.is_none_or(|(by, _)| y > by) {
                best = Some((y, *p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Median of `v` (0 when empty).
fn median(mut v: Vec<f32>) -> f32 {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return 0.0;
    }
    let k = v.len() / 2;
    *v.select_nth_unstable_by(k, f32::total_cmp).1
}

/// The V3 Dehaze of one pixel: `lin` is its white-balanced, pre-exposure colour, `gain` the
/// exposure gain, `d` its haze ([`HazeMap::dark`]) and `k` the amount (−1..1, global plus masks).
/// Returns the exposed, dehazed colour. A non-finite result falls back to the exposed input.
pub fn recover(map: &HazeMap, lin: [f32; 3], gain: f32, d: f32, k: f32) -> [f32; 3] {
    let k = if k.is_finite() { k.clamp(-1.0, 1.0) } else { 0.0 };
    let j = recover_haze(map, lin, gain, d, k);
    finish_recovery(map, lin, gain, k, j)
}

/// The V3 Dehaze of the pixel at frame-normalised `(nx, ny)` (0..1 over the whole output frame,
/// so a window dehazes exactly like the whole render): `lin` is its white-balanced, pre-exposure
/// colour, `gain` the exposure gain and `k` the amount (global plus masks; clamped to −1..1).
/// [`HazeMap::dark`] and [`recover`] in one, encoding the pixel once. Returns the exposed colour;
/// a non-finite result falls back to the exposed input.
pub fn dehaze_at(map: &HazeMap, nx: f32, ny: f32, lin: [f32; 3], gain: f32, k: f32) -> [f32; 3] {
    let k = if k.is_finite() { k.clamp(-1.0, 1.0) } else { 0.0 };
    if k == 0.0 {
        return lin.map(|v| v * gain);
    }
    let e = encode3(lin);
    let (a, b) = map.coeffs(nx, ny);
    let d = a * norm_dark(e, map.air) + b;
    let d = if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 };
    let j = recover_with(map, lin, gain, d, k, || sky_distance(map, e));
    finish_recovery(map, lin, gain, k, j)
}

/// [`recover`]'s exposure compensation and chroma blend (positive amounts) of the recovered `j`,
/// and its fallback to the exposed input when anything is not finite.
fn finish_recovery(map: &HazeMap, lin: [f32; 3], gain: f32, k: f32, mut j: [f32; 3]) -> [f32; 3] {
    if k > 0.0 {
        let comp = map.comp(k);
        j = j.map(|v| v * comp);
        // keep J's luminance, blend some of the input's chromaticity back
        let c = lin.map(|v| v * gain);
        let (yj, yc) = (luminance_2020(j), luminance_2020(c));
        if yj > 1e-9 && yc > 1e-9 {
            let s = yj / yc;
            j = std::array::from_fn(|i| j[i] + (c[i] * s - j[i]) * CHROMA_KEEP);
        }
    }
    let c = lin.map(|v| v * gain);
    if j.iter().all(|v| v.is_finite()) { j } else { c }
}

/// The recovery of [`recover`] before its exposure compensation and chroma blend (what the
/// compensation LUT measures). `k` is already clamped to −1..1.
fn recover_haze(map: &HazeMap, lin: [f32; 3], gain: f32, d: f32, k: f32) -> [f32; 3] {
    recover_with(map, lin, gain, d, k, || sky_distance(map, encode3(lin)))
}

/// How far an encoded pixel `e` is from the airlight: its largest channel difference, relative to
/// the airlight's brightest channel (so the same photo exposed brighter or darker is judged alike).
fn sky_distance(map: &HazeMap, e: [f32; 3]) -> f32 {
    let top = map.air.iter().fold(AIR_FLOOR, |m, v| m.max(*v));
    (0..3).map(|i| (e[i] - map.air[i]).abs()).fold(0.0f32, f32::max) / top
}

/// [`recover_haze`] with the pixel's [`sky_distance`] given (computed only when needed).
fn recover_with(map: &HazeMap, lin: [f32; 3], gain: f32, d: f32, k: f32, sky: impl FnOnce() -> f32) -> [f32; 3] {
    let c = lin.map(|v| v * gain);
    let air = map.air_lin.map(|v| v * gain);
    let d = if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 };
    if k > 0.0 {
        let omega = OMEGA * k.powf(OMEGA_POWER);
        let tmin = 1.0 + (T_FLOOR - 1.0) * k;
        // the map measures haze in the encoding; the recovery is linear, so is its haze share
        // (`min(e/A)` of an encoded pixel is `min(c/A)^(1/GAMMA)` of the linear one)
        let mut t = (1.0 - omega * decode(d)).max(tmin);
        // sky tolerance: pixels close to the airlight colour are left nearly alone
        let m = sky();
        if m < SKY_K {
            t = if m > 1e-6 { (t * SKY_K / m).min(1.0) } else { 1.0 };
        }
        let t = t.max(T_FLOOR);
        std::array::from_fn(|i| (air[i] + (c[i] - air[i]) / t).max(0.0))
    } else if k < 0.0 {
        let t = 1.0 - VEIL * -k * (VEIL_BASE + (1.0 - VEIL_BASE) * d);
        std::array::from_fn(|i| c[i] * t + air[i] * (1.0 - t))
    } else {
        c
    }
}

/// Whether `s` dehazes with a [`HazeMap`]: process V3 with Dehaze (the Effects slider, its section
/// on, or any mask's) not zero.
pub fn wanted(s: &DevelopSettings) -> bool {
    let v3 = match s.process.process() {
        Process::V1 | Process::V2 => false,
        Process::V3 => true,
    };
    v3 && ((s.section_enabled("effects") && s.effects.dehaze != 0.0) || s.masks.iter().any(|m| m.adjust.dehaze != 0.0))
}

/// Proxy size (px) of the map of `frame` (the whole output frame).
pub fn proxy_size(frame: &Frame) -> (usize, usize) {
    let (nw, nh) = frame.native_size();
    let native = nw.max(nh);
    let edge = if native.is_finite() && native >= 1.0 { (native.round() as usize).min(PROXY_EDGE) } else { PROXY_EDGE };
    frame.fit(edge.max(1), edge.max(1))
}

/// The haze map of `src` developed with `s` through `frame` (the whole output frame, before any
/// window), from a cache of the last few maps (keyed by the source's content fingerprint, the
/// frame, the white balance, the process and the proxy size).
pub fn for_frame(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, frame: &Frame) -> Arc<HazeMap> {
    static CACHE: Mutex<Vec<(u64, Arc<HazeMap>)>> = Mutex::new(Vec::new());
    const KEEP: usize = 4;
    // (an empty source has nothing to sample: no haze)
    if src.width == 0 || src.height == 0 || src.data.is_empty() {
        return Arc::new(HazeMap::empty());
    }
    let (pw, ph) = proxy_size(frame);
    let wb = crate::local::wb_matrix_for(info, s).map(|m| m.map(|r| r.map(f32::to_bits)));
    let key = crate::hash_of((crate::optics::fingerprint(src), format!("{frame:?}"), wb, s.process.process(), pw, ph));
    if let Some(m) = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
        return m.1.clone();
    }
    let map = Arc::new(crate::timed("haze map", || {
        let mut img = crate::timed("haze proxy", || frame.sample(src, pw, ph));
        crate::local::white_balance(&mut img, info, s);
        HazeMap::build(&img)
    }));
    let mut c = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    c.retain(|(k, _)| *k != key);
    c.push((key, map.clone()));
    while c.len() > KEEP {
        c.remove(0);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin_of_encoded(e: [f32; 3]) -> [f32; 3] {
        e.map(decode)
    }

    /// A hazy scene: a dark, contrasty foreground (bottom) under a bright bluish-grey veil (top),
    /// with a small white object in the foreground.
    fn hazy(w: usize, h: usize) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            let fy = y as f32 / h as f32;
            if (w / 2..w / 2 + 3).contains(&x) && (h * 3 / 4..h * 3 / 4 + 3).contains(&y) {
                return [1.0, 1.0, 1.0];
            }
            if fy < 0.4 {
                lin_of_encoded([0.70, 0.74, 0.80])
            } else {
                let t = ((x / 6 + y / 6) % 2) as f32;
                lin_of_encoded([0.10 + 0.25 * t, 0.12 + 0.2 * t, 0.08 + 0.1 * t])
            }
        })
    }

    #[test]
    fn airlight_is_the_haze_not_a_small_white_object() {
        let m = HazeMap::build(&hazy(200, 150));
        // the veil [0.70, 0.74, 0.80] pulled halfway to neutral, not the white object's 1.0
        let y = luminance_2020([0.70, 0.74, 0.80]);
        for (i, c) in [0.70, 0.74, 0.80].into_iter().enumerate() {
            let want = c + (y - c) * AIR_NEUTRAL;
            assert!((m.air[i] - want).abs() < 1e-3, "air {:?} want ch{i} {want}", m.air);
        }
        // haze is high in the veil, low in the foreground
        let (top, bottom) = (m.dark(0.5, 0.1, lin_of_encoded([0.70, 0.74, 0.80])), m.dark(0.3, 0.8, lin_of_encoded([0.10, 0.12, 0.08])));
        assert!(top > 0.8 && bottom < 0.3, "top {top} bottom {bottom}");
    }

    #[test]
    fn a_flat_grey_has_constant_haze_and_is_left_alone() {
        for v in [0.02f32, 0.18, 0.5, 1.0] {
            let img = Rgb32f::filled(64, 48, [v; 3]);
            let m = HazeMap::build(&img);
            assert!(m.air.iter().all(|a| a.is_finite() && *a >= AIR_FLOOR), "{:?}", m.air);
            let d0 = m.dark(0.0, 0.0, [v; 3]);
            for (nx, ny) in [(0.5, 0.5), (1.0, 1.0), (0.2, 0.9)] {
                assert!((m.dark(nx, ny, [v; 3]) - d0).abs() < 1e-4);
            }
            assert!(m.comp.iter().all(|c| (*c - 1.0).abs() < 1e-3), "{:?}", m.comp);
            for k in [-1.0, -0.5, 0.25, 0.5, 1.0] {
                let out = recover(&m, [v; 3], 1.0, d0, k);
                for c in out {
                    assert!((c - v).abs() <= v * 2e-3 + 1e-6, "v {v} k {k} -> {out:?}");
                }
            }
        }
    }

    #[test]
    fn black_white_and_nan_images_never_give_nan() {
        let imgs = [
            Rgb32f::filled(32, 32, [0.0; 3]),
            Rgb32f::filled(32, 32, [1.0; 3]),
            Rgb32f::filled(32, 32, [f32::NAN; 3]),
            Rgb32f::filled(32, 32, [f32::INFINITY, -1.0, f32::NAN]),
            Rgb32f::from_fn(32, 32, |x, _| if x % 3 == 0 { [f32::NAN, 0.5, 0.2] } else { [0.3, 0.2, 0.1] }),
            Rgb32f::new(0, 0),
            Rgb32f::new(5, 0),
            Rgb32f { width: 4, height: 4, data: vec![[0.5; 3]; 3] },
        ];
        for img in &imgs {
            let m = HazeMap::build(img);
            assert!(m.air.iter().all(|a| a.is_finite() && *a >= AIR_FLOOR), "{:?}", m.air);
            assert!(m.air_lin.iter().all(|a| a.is_finite()));
            assert!(m.a.data.iter().chain(&m.b.data).all(|v| v.is_finite()));
            assert!(m.comp.iter().all(|c| c.is_finite() && *c >= 1.0));
            for px in [[0.0; 3], [1.0; 3], [f32::NAN; 3], [f32::INFINITY; 3], [-1.0; 3]] {
                for (nx, ny) in [(0.5, 0.5), (f32::NAN, 2.0), (-3.0, f32::INFINITY)] {
                    let d = m.dark(nx, ny, px);
                    assert!((0.0..=1.0).contains(&d), "{d}");
                    for k in [-1.0, -0.3, 0.0, 0.4, 1.0, f32::NAN, 7.0] {
                        let out = recover(&m, px, 1.0, d, k);
                        let fallback = px.map(|v| v * 1.0);
                        assert!(
                            out.iter().all(|v| v.is_finite()) || out.iter().zip(fallback).all(|(a, b)| a.to_bits() == b.to_bits()),
                            "{px:?} k {k} -> {out:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        let img = hazy(160, 120);
        assert_eq!(HazeMap::build(&img), HazeMap::build(&img));
    }

    #[test]
    fn coefficients_are_frame_normalised() {
        let m = HazeMap::build(&hazy(100, 80));
        // corners land on the corner proxy pixels, and outside is the edge's
        let (w, h) = m.size();
        let at = |p: &Plane, x: usize, y: usize| p.data[y * w + x];
        assert_eq!(m.coeffs(0.0, 0.0), (at(&m.a, 0, 0), at(&m.b, 0, 0)));
        assert_eq!(m.coeffs(1.0, 1.0), (at(&m.a, w - 1, h - 1), at(&m.b, w - 1, h - 1)));
        assert_eq!(m.coeffs(-5.0, 9.0), m.coeffs(0.0, 1.0));
    }

    #[test]
    fn compensation_brightens_more_with_the_amount_and_is_bounded() {
        let m = HazeMap::build(&hazy(200, 150));
        assert_eq!(m.comp(0.0), 1.0);
        assert_eq!(m.comp(-0.5), 1.0);
        let max = 2f32.powf(COMP_MAX_EV).powf(COMP_POWER);
        let mut last = 1.0;
        for i in 0..=40 {
            let c = m.comp(i as f32 / 40.0);
            assert!(c >= last - 1e-6 && c <= max + 1e-6, "{i}: {c} after {last}");
            last = c;
        }
        assert!(m.comp(1.0) > 1.0, "{:?}", m.comp);
        assert_eq!(m.comp(1.0), m.comp[COMP_N - 1]);
        assert_eq!(m.comp(5.0), m.comp(1.0));
    }

    #[test]
    fn dehaze_adds_contrast_and_negative_adds_a_veil() {
        let m = HazeMap::build(&hazy(200, 150));
        let (p1, p2) = (lin_of_encoded([0.55, 0.57, 0.60]), lin_of_encoded([0.45, 0.47, 0.50]));
        let y = |c: [f32; 3]| luminance_2020(c);
        let before = y(p1) - y(p2);
        let after = y(recover(&m, p1, 1.0, 0.5, 0.5)) - y(recover(&m, p2, 1.0, 0.5, 0.5));
        assert!(after > before * 1.1, "contrast {before} -> {after}");
        // without the exposure compensation the hazy tones darken
        assert!(y(recover_haze(&m, p1, 1.0, 0.5, 0.5)) < y(p1));
        let veiled = recover(&m, [0.01; 3], 1.0, 0.5, -1.0);
        assert!(y(veiled) > 0.05, "a veil lifts black: {veiled:?}");
        let less = y(recover(&m, p1, 1.0, 0.5, -1.0)) - y(recover(&m, p2, 1.0, 0.5, -1.0));
        assert!(less < before * 0.8, "negative lowers contrast: {before} -> {less}");
    }

    #[test]
    fn proxy_is_capped_at_the_proxy_edge() {
        let s = DevelopSettings::default();
        let big = Frame::new(4000, 3000, &s, true);
        assert_eq!(proxy_size(&big), (1024, 768));
        let small = Frame::new(300, 200, &s, true);
        assert_eq!(proxy_size(&small), (300, 200));
    }

    #[test]
    fn wanted_only_for_v3_with_dehaze() {
        use lightcraft_develop::ProcessVersion;
        let mut s = DevelopSettings { process: Process::V3.version(), ..DevelopSettings::default() };
        assert!(!wanted(&s));
        s.effects.dehaze = 30.0;
        assert!(wanted(&s));
        s.disabled_sections.push("effects".into());
        assert!(!wanted(&s));
        s.process = ProcessVersion(2);
        s.disabled_sections.clear();
        assert!(!wanted(&s));
    }
}
