//! How much colour noise a render has, so that V3's colour noise reduction takes away noise and not
//! colour detail (`local::nr_color_guided`).
//!
//! The guided filter of chromaticity removes colour noise and, with it, whatever colour detail
//! luminance doesn't explain: the warm and cool strands of hair, fabric, foliage. At one fixed
//! strength it turned low-ISO hair grey, and still left blotches in flat areas of high-ISO
//! photos. So the filter is wide enough for
//! the blotches, and what it takes away is put back where it is clearly more than noise: where its
//! local energy (a Gaussian of `|c − q|²` over [`KEEP_WINDOW`] px) is well above the colour
//! noise's energy at that brightness ([`crate::local::keep_weight`]).
//!
//! The noise's energy is measured on the render: per band of the filter's guide (cube-root
//! luminance), the median of that local energy over the band's pixels (most pixels of a band are
//! smooth), never rising with brightness (colour noise falls as the light rises; a band whose
//! median rises is detail, not noise).
//!
//! A windowed render can't measure the whole frame, and a window full of hair would take hair for
//! noise. So every render, whole or windowed, measures the same [`GRID`] × [`GRID`] patches of the
//! whole frame at its output scale, sampled from the source as a render of them would be, white
//! balanced and luminance-denoised like the render (spot removal, red eye, defringe and the camera's
//! local tone mapping are left out: they change little of a band's noise). The patches and the
//! measurement are cached: a drag of an unrelated slider measures nothing, and the drafts of a
//! white balance or noise reduction drag reuse the last measurement of the same framing.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_raster::Rgb32f;

use crate::local::{self, nr_guide};
use crate::{PixelWindow, Plan, SourceInfo, geometry::Frame, hash_of};

/// Bands of the guide (`local::nr_guide`) the noise is measured in, over guide values 0 to [`G_MAX`].
pub const BANDS: usize = 16;
/// Guide value at the top of the last band (above it, the last band's noise).
pub const G_MAX: f32 = 1.25;
/// Sigma (output px) of the local energy that tells colour detail from noise.
pub const KEEP_WINDOW: f32 = 1.5;
/// Patches per side of the frame.
pub const GRID: usize = 4;
/// Side of a patch's measured interior (output px) and the context around it.
const PATCH: usize = 64;
const MARGIN: usize = 24;
/// Frames up to this many pixels are measured whole.
const WHOLE_MAX: usize = 512 * 512;
/// Fewer samples than this and a band takes its neighbour's noise.
const MIN_SAMPLES: usize = 64;

/// A render's colour noise ([`for_plan`], for V3's colour noise reduction); `stand_in` when a draft
/// took the last measurement of its framing instead of measuring its own settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    pub noise: ChromaNoise,
    pub stand_in: bool,
}

impl Measured {
    /// The key a render caches its noise-reduced image (and the planes from it) under: the plan's,
    /// or, for a stand-in, one no render with its own measurement looks up.
    pub fn lin_key(m: Option<&Measured>, plan_key: u64) -> u64 {
        if m.is_some_and(|m| m.stand_in) { hash_of((plan_key, "colour noise stand-in")) } else { plan_key }
    }
}

/// The colour noise of a render: per band of the guide, the typical local energy of what the
/// guided filter takes away from a smooth area (squared chromaticity units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromaNoise(pub [f32; BANDS]);

impl ChromaNoise {
    /// The band of guide value `g`.
    pub fn band(g: f32) -> usize {
        ((g / G_MAX * BANDS as f32).max(0.0) as usize).min(BANDS - 1)
    }

    /// The noise at guide value `g`: linear between band centres. The GPU's `nrx_apply` mirrors it.
    pub fn at(&self, g: f32) -> f32 {
        let x = (g / G_MAX * BANDS as f32 - 0.5).clamp(0.0, (BANDS - 1) as f32);
        let x = if x.is_nan() { 0.0 } else { x };
        let i = (x as usize).min(BANDS - 2);
        let f = x - i as f32;
        let (a, b) = (self.0.get(i).copied().unwrap_or(0.0), self.0.get(i + 1).copied().unwrap_or(0.0));
        a + (b - a) * f
    }

    /// From `(guide, energy)` samples: each band's median; a band with too few samples takes the
    /// nearest measured band's (the darker on a tie), and none is above a darker one.
    pub fn from_samples(samples: &[(f32, f32)]) -> ChromaNoise {
        let mut bands: Vec<Vec<f32>> = vec![Vec::new(); BANDS];
        for &(g, e) in samples {
            if e.is_finite()
                && let Some(b) = bands.get_mut(Self::band(g))
            {
                b.push(e.max(0.0));
            }
        }
        let median = |v: &mut Vec<f32>| {
            let k = v.len() / 2;
            v.select_nth_unstable_by(k, f32::total_cmp);
            v.get(k).copied()
        };
        let measured: Vec<Option<f32>> = bands.iter_mut().map(|v| if v.len() >= MIN_SAMPLES { median(v) } else { None }).collect();
        let fallback = {
            let mut all: Vec<f32> = bands.into_iter().flatten().collect();
            if all.is_empty() { 0.0 } else { median(&mut all).unwrap_or(0.0) }
        };
        Self::from_bands(&measured, fallback)
    }

    /// From each band's measurement, if any: a band without one takes the nearest measured band's
    /// (the darker on a tie), `fallback` when none is measured, and none is above a darker one.
    fn from_bands(measured: &[Option<f32>], fallback: f32) -> ChromaNoise {
        let mut out = [0.0f32; BANDS];
        for (i, o) in out.iter_mut().enumerate() {
            *o = (0..BANDS)
                .find_map(|d| {
                    let darker = i.checked_sub(d).and_then(|j| measured.get(j).copied().flatten());
                    darker.or_else(|| measured.get(i + d).copied().flatten())
                })
                .unwrap_or(fallback);
        }
        for i in 1..BANDS {
            out[i] = out[i].min(out[i - 1]);
        }
        ChromaNoise(out)
    }
}

/// The patches measured in a `w × h` frame: the window each samples and the interior measured,
/// relative to it. The whole frame when it is small.
pub fn patches(w: usize, h: usize) -> Vec<(PixelWindow, PixelWindow)> {
    let (w, h) = (w.max(1), h.max(1));
    if w.saturating_mul(h) <= WHOLE_MAX {
        return vec![(PixelWindow { x: 0, y: 0, w, h }, PixelWindow { x: 0, y: 0, w, h })];
    }
    // (start, size, interior start, interior size) along a side of `len` px for patch `i`
    let side = |len: usize, i: usize| {
        let size = (PATCH + 2 * MARGIN).min(len);
        let centre = (2 * i + 1) * len / (2 * GRID);
        let start = centre.saturating_sub(size / 2).min(len - size);
        let m = MARGIN.min(size / 4);
        (start, size, m, size - 2 * m)
    };
    let mut out = Vec::with_capacity(GRID * GRID);
    for j in 0..GRID {
        for i in 0..GRID {
            let ((x, pw, ix, iw), (y, ph, iy, ih)) = (side(w, i), side(h, j));
            out.push((PixelWindow { x, y, w: pw, h: ph }, PixelWindow { x: ix, y: iy, w: iw, h: ih }));
        }
    }
    out
}

/// What the cache keeps: sampled patches by source and framing, measurements by those and the
/// settings they depend on.
type Patches = Arc<Vec<Rgb32f>>;
static PATCHES: Mutex<Vec<(u64, Patches)>> = Mutex::new(Vec::new());
/// (settings key, framing key, colour noise)
static MEASURED: Mutex<Vec<(u64, u64, ChromaNoise)>> = Mutex::new(Vec::new());
const PATCHES_KEPT: usize = 3;
const MEASURED_KEPT: usize = 16;

fn cached<T: Clone>(cache: &Mutex<Vec<(u64, T)>>, key: u64) -> Option<T> {
    let c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    c.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
}

fn remember<T>(cache: &Mutex<Vec<(u64, T)>>, key: u64, v: T, kept: usize) {
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    c.retain(|(k, _)| *k != key);
    if c.len() >= kept {
        c.remove(0);
    }
    c.push((key, v));
}

/// Identity of a source buffer: where it lives, its size and a sample of its pixels (a freed
/// buffer's address can come back with another photo in it).
fn source_key(src: &Rgb32f) -> u64 {
    let n = src.data.len();
    let step = (n / 61).max(1);
    let sample: Vec<[u32; 3]> = src.data.iter().step_by(step).map(|p| p.map(f32::to_bits)).collect();
    hash_of((src.data.as_ptr() as usize, src.width, src.height, sample))
}

/// The colour noise of the render `plan` makes of `src` ([`Measured`]), or `None` when it needs none
/// (no V3 colour noise reduction). A `draft` takes the last measurement of the same framing when its
/// own settings haven't been measured.
pub fn for_plan(src: &Rgb32f, info: &SourceInfo, plan: &Plan<'_>, draft: bool) -> Option<Measured> {
    let s = &*plan.settings;
    let (frame, fw, fh): (&Frame, usize, usize) = match &plan.whole {
        Some((f, w, h)) => (f, *w, *h),
        None => (&plan.frame, plan.w, plan.h),
    };
    let (lum, col) = local::nr_params(s, plan.src_long, frame.output_long(fw, fh));
    let (c, eps) = col.filter(|c| c.keep > 0.0).and_then(|c| Some((c, c.eps?)))?;
    let pkey = hash_of((source_key(src), format!("{frame:?}"), fw, fh));
    let (wb_t, wb_tint) = local::effective_wb(info, s);
    let key = hash_of((
        pkey,
        [wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        format!("{lum:?} {c:?}"),
        local::wb_matrix_for(info, s).map(|m| m.map(|r| r.map(f32::to_bits))),
    ));
    {
        let m = MEASURED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, _, noise)) = m.iter().rev().find(|(k, _, _)| *k == key) {
            return Some(Measured { noise: *noise, stand_in: false });
        }
        if let Some((_, _, noise)) = m.iter().rev().find(|(_, p, _)| draft && *p == pkey) {
            return Some(Measured { noise: *noise, stand_in: true });
        }
    }
    let _t = crate::profiling().then(std::time::Instant::now);
    let layout = patches(fw, fh);
    let sampled = match cached(&PATCHES, pkey) {
        Some(p) => p,
        None => {
            let wins: Vec<PixelWindow> = layout.iter().map(|(win, _)| *win).collect();
            let p = Arc::new(frame.sample_windows(src, fw, fh, &wins));
            remember(&PATCHES, pkey, p.clone(), PATCHES_KEPT);
            p
        }
    };
    let mut colour = Vec::new();
    for (img, (_, inner)) in sampled.iter().zip(&layout) {
        let mut img = img.clone();
        local::white_balance(&mut img, info, s);
        if let Some(l) = &lum {
            local::nr_luminance(&mut img, l);
        }
        let chroma = img.map(local::chromaticity);
        let q = local::nr_color_filtered(&img, &chroma, c.sigma, eps);
        let e = local::removed_energy(&chroma, &q);
        for y in inner.y..inner.y + inner.h {
            for x in inner.x..inner.x + inner.w {
                let i = y * img.width + x;
                if let (Some(p), Some(e)) = (img.data.get(i), e.data.get(i)) {
                    colour.push((nr_guide(*p), e.abs()));
                }
            }
        }
    }
    let noise = ChromaNoise::from_samples(&colour);
    {
        let mut m = MEASURED.lock().unwrap_or_else(PoisonError::into_inner);
        m.retain(|(k, _, _)| *k != key);
        if m.len() >= MEASURED_KEPT {
            m.remove(0);
        }
        m.push((key, pkey, noise));
    }
    if let Some(t) = _t {
        eprintln!("    noise measured: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    Some(Measured { noise, stand_in: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A band's noise is the median of its samples; an empty band takes its nearest measured
    /// neighbour's; a brighter band never has more noise than a darker one.
    #[test]
    fn bands_take_medians_fill_gaps_and_never_rise_with_brightness() {
        let centre = |b: usize| (b as f32 + 0.5) * G_MAX / BANDS as f32;
        let mut samples = Vec::new();
        for i in 0..200 {
            // band 2: energies 1..=200 (median 101 or so); band 5: 0.5; band 9: 4.0 (detail)
            samples.push((centre(2), (i + 1) as f32));
            samples.push((centre(5), 0.5));
            samples.push((centre(9), 4.0));
        }
        let n = ChromaNoise::from_samples(&samples);
        assert!((n.0[2] - 101.0).abs() <= 1.0, "{:?}", n.0);
        assert_eq!(n.0[0], n.0[2], "darker bands without samples take the nearest measured");
        assert_eq!(n.0[5], 0.5);
        assert_eq!(n.0[3], n.0[2].min(n.0[3]), "never above a darker band");
        assert_eq!(n.0[9], 0.5, "a brighter band measuring more than a darker one is detail: {:?}", n.0);
        assert_eq!(n.0[BANDS - 1], 0.5);
        // between band centres the noise is linear, outside them flat
        assert!((n.at((centre(2) + centre(3)) / 2.0) - (n.0[2] + n.0[3]) / 2.0).abs() < 1e-3);
        assert_eq!(n.at(-1.0), n.0[0]);
        assert_eq!(n.at(f32::NAN), n.0[0]);
        assert_eq!(n.at(9.0), n.0[BANDS - 1]);
        assert_eq!(ChromaNoise::from_samples(&[]).0, [0.0; BANDS]);
    }

    /// Patches lie inside the frame with their interiors inside them, in a grid over large frames
    /// and as the whole of small or thin ones.
    #[test]
    fn patches_cover_the_frame_inside_its_bounds() {
        for (w, h) in [(1, 1), (300, 200), (512, 512), (513, 512), (9504, 6336), (6336, 9504), (100, 9000), (20_000, 41)] {
            let p = patches(w, h);
            assert!(!p.is_empty());
            if w * h <= WHOLE_MAX {
                assert_eq!(p.len(), 1, "{w}×{h}");
            } else {
                assert_eq!(p.len(), GRID * GRID, "{w}×{h}");
            }
            for (win, inner) in p {
                assert!(win.w >= 1 && win.h >= 1 && win.x + win.w <= w && win.y + win.h <= h, "{w}×{h}: {win:?}");
                assert!(inner.w >= 1 && inner.h >= 1 && inner.x + inner.w <= win.w && inner.y + inner.h <= win.h, "{w}×{h}: {inner:?} in {win:?}");
            }
        }
    }
}
