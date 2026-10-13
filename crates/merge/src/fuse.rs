//! Mertens exposure fusion: several frames of one scene at different exposures, combined in the
//! *image* domain with no radiance recovery and no tone mapping.
//!
//! This is deliberately not what [`crate::hdr`] does, and the difference is the point of having both.
//! The HDR merge recovers scene radiance — `Rᵢ = vᵢ / kᵢ`, weighting by exposure length and by
//! saturation, then deghosting — and hands the caller a linear image to tone map. Fusion instead
//! scores each pixel of each frame by how *good* it looks: how close to mid-grey it sits, how much
//! local contrast it has, how saturated its colour is. The result is display-ready, with no clipped
//! highlights and no crushed shadows, and it needs no exposure ratios at all — so it also works on
//! frames whose EXIF disagrees, or on a sequence that is not a bracket in the first place
//! (flash/no-flash, a scanned negative, a phone's "best shot" set).
//!
//! The weights are Mertens, Kautz & Van Reeth (2007): well-exposedness × contrast × saturation, each
//! raised to a power. Their paper blends Laplacian pyramids to avoid halos where the winning frame
//! changes sharply; this takes the weighted mean of the images instead, because the pyramid blend
//! belongs in `lightcraft-raster` and would double this module. The contrast and saturation terms
//! carry a floor, so a perfectly flat region does not score zero in every frame (which would be a
//! 0/0), and each weight map is smoothed before use so one noisy pixel cannot decide a region.

use crate::align::{self, Alignment};
use crate::frame::{Frame, FrameColor};
use crate::{MergeError, Progress, Result, check};
use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

/// σ of the well-exposedness bell around mid-grey, from the paper.
const WELL_SIGMA: f32 = 0.2;
/// Floors on the contrast and saturation terms, so a flat region still has a weight.
const CONTRAST_FLOOR: f32 = 1e-3;
const SATURATION_FLOOR: f32 = 1e-3;
/// σ of the blur applied to each weight map, as a fraction of the long edge. Wide enough that a
/// region is decided by its neighbourhood rather than by one pixel, narrow enough to keep the
/// transition where the winning frame changes to a few pixels.
const WEIGHT_SMOOTH: f32 = 0.002;

/// What a fusion should do. The three exponents are the paper's, each 1.0: raising one makes that
/// term decide more of the result.
#[derive(Debug, Clone)]
pub struct FuseOptions {
    /// Register the frames to the reference before fusing.
    pub align: bool,
    pub well_exposed: f32,
    pub contrast: f32,
    pub saturation: f32,
}

impl Default for FuseOptions {
    fn default() -> Self {
        FuseOptions { align: true, well_exposed: 1.0, contrast: 1.0, saturation: 1.0 }
    }
}

#[derive(Debug, Clone)]
pub struct FuseResult {
    pub image: Rgb32f,
    /// The frame each pixel took most of its value from (`0..frames.len()`): a diagnostic, and what a
    /// "which exposure did this come from" view would show.
    pub source: Plane,
    pub alignments: Vec<Alignment>,
    pub reference: usize,
    pub color: FrameColor,
    pub raw: bool,
    pub orientation: Orientation,
    pub metadata: Metadata,
    pub baseline_exposure: f64,
}

/// Rec.709 grey, which is what the contrast term is measured on.
fn grey_of(px: [f32; 3]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// The 5-point Laplacian of `grey` at `(x, y)`, with edges clamped so a border pixel is defined.
fn laplacian(grey: &Plane, x: usize, y: usize) -> f32 {
    let w = grey.width;
    let h = grey.height;
    let at = |x: usize, y: usize| grey.data[y * w + x];
    let xm = x.saturating_sub(1);
    let xp = (x + 1).min(w.saturating_sub(1));
    let ym = y.saturating_sub(1);
    let yp = (y + 1).min(h.saturating_sub(1));
    4.0 * at(x, y) - at(xm, y) - at(xp, y) - at(x, ym) - at(x, yp)
}

/// Fuse an exposure sequence into one display-ready image.
///
/// # Errors
/// [`MergeError::TooFew`] with fewer than two frames; [`MergeError::Mismatch`] when the frames are not
/// all the same size.
pub fn fuse_exposures(mut frames: Vec<Frame>, opts: &FuseOptions, progress: &Progress) -> Result<FuseResult> {
    let n = frames.len();
    if n < 2 {
        return Err(MergeError::TooFew(n, 2));
    }
    let (w, h) = (frames[0].image.width, frames[0].image.height);
    for (i, f) in frames.iter().enumerate() {
        if f.image.width != w || f.image.height != h {
            return Err(MergeError::Mismatch(format!("frame {i} is {}x{}, not {w}x{h}", f.image.width, f.image.height)));
        }
    }

    // The middle frame supplies the output's colour tags and orientation. Fusion has no brightness
    // order of its own, so there is nothing better to pick by.
    let reference = n / 2;

    let mut alignments = vec![Alignment::identity(); n];
    if opts.align {
        check(progress, 0.05, "Aligning")?;
        // Features are found on a middle-grey-normalised copy, as in the HDR merge, so they come from
        // the scene rather than from the exposure.
        let g0 = 0.18 / align::median_luma(&frames[reference].image, frames[reference].clip).max(1e-6);
        let feats: Vec<(crate::features::Features, f64)> = frames
            .par_iter()
            .map(|f| {
                let (p, s) = align::work_image(&f.image, g0, align::WORK_EDGE);
                (crate::features::detect(&p, 1500), s)
            })
            .collect();
        check(progress, 0.2, "Aligning")?;
        let found: Vec<(usize, Alignment)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != reference)
            .map(|i| (i, align::align_pair(&feats[reference].0, &feats[i].0, feats[i].1, w, h, i as u64 + 1)))
            .collect();
        for (i, a) in found {
            alignments[i] = a;
        }
        check(progress, 0.3, "Resampling")?;
        let warped: Vec<(usize, Rgb32f)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != reference && alignments[i].model != "identity")
            .map(|i| {
                let (img, _valid) = align::warp(&frames[i].image, &alignments[i].h);
                (i, img)
            })
            .collect();
        for (i, img) in warped {
            frames[i].image = img;
        }
    }
    check(progress, 0.4, "Scoring frames")?;

    let sigma = (WEIGHT_SMOOTH * w.max(h) as f32).max(1.0);
    let weights: Vec<Plane> = (0..n)
        .into_par_iter()
        .map(|i| {
            let img = &frames[i].image;
            let grey = img.map(grey_of);
            let mut p = Plane::new(w, h);
            p.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let i = y * w + x;
                    let px = img.data[i];
                    // Well-exposedness: a bell around mid-grey, multiplied over the three channels.
                    let mut we = 1.0f32;
                    for c in 0..3 {
                        let d = px[c] - 0.5;
                        we *= (-(d * d) / (2.0 * WELL_SIGMA * WELL_SIGMA)).exp();
                    }
                    // Contrast: the Laplacian of the grey image.
                    let lap = laplacian(&grey, x, y).abs();
                    // Saturation: how far the channels spread.
                    let m = (px[0] + px[1] + px[2]) / 3.0;
                    let var = ((px[0] - m) * (px[0] - m) + (px[1] - m) * (px[1] - m) + (px[2] - m) * (px[2] - m)) / 3.0;
                    let sat = var.sqrt();
                    *o = we.powf(opts.well_exposed) * (lap + CONTRAST_FLOOR).powf(opts.contrast) * (sat + SATURATION_FLOOR).powf(opts.saturation);
                }
            });
            gaussian(&p, sigma)
        })
        .collect();
    check(progress, 0.7, "Fusing")?;

    let mut out = Rgb32f::new(w, h);
    let mut source = Plane::new(w, h);
    out.data.par_chunks_mut(w).zip(source.data.par_chunks_mut(w)).enumerate().for_each(|(y, (row, srow))| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let mut total = 0.0f32;
            let mut best = 0usize;
            let mut best_w = f32::NEG_INFINITY;
            for f in 0..n {
                let v = weights[f].data[i];
                total += v;
                if v > best_w {
                    best_w = v;
                    best = f;
                }
            }
            if total <= 0.0 {
                // No frame has any weight here (possible only if every weight underflowed).
                // Taking the reference is the honest fallback: it is the frame the geometry and
                // colour come from.
                *o = frames[reference].image.data[i];
                srow[x] = reference as f32;
                continue;
            }
            let mut acc = [0.0f32; 3];
            for f in 0..n {
                let v = weights[f].data[i];
                if v <= 0.0 {
                    continue;
                }
                let px = frames[f].image.data[i];
                for c in 0..3 {
                    acc[c] += v * px[c];
                }
            }
            for c in 0..3 {
                acc[c] /= total;
            }
            *o = acc;
            srow[x] = best as f32;
        }
    });
    check(progress, 0.97, "Fusing")?;

    let rf = &frames[reference];
    Ok(FuseResult {
        image: out,
        source,
        alignments,
        reference,
        color: rf.color.clone(),
        raw: rf.raw,
        orientation: rf.orientation,
        metadata: rf.metadata.clone(),
        baseline_exposure: rf.baseline_exposure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(v: f32) -> Frame {
        Frame::from_linear_rec2020(Rgb32f::from_fn(16, 12, |_, _| [v, v, v]), None)
    }

    fn fuse(frames: Vec<Frame>) -> FuseResult {
        let opts = FuseOptions { align: false, ..FuseOptions::default() };
        fuse_exposures(frames, &opts, &crate::no_progress).expect("fuses")
    }

    /// On a flat region there is no contrast and no saturation to score, so the well-exposedness term
    /// decides: the frame nearest mid-grey is taken. This is also the flat-region edge case that
    /// would be a 0/0 without the floors on the other two terms.
    #[test]
    fn a_flat_region_takes_the_best_exposed_frame() {
        let r = fuse(vec![flat(0.05), flat(0.5), flat(0.95)]);
        let v = r.image.data[0][0];
        assert!((v - 0.5).abs() < 0.02, "flat fusion should sit at mid-grey, got {v}");
        assert!((r.source.data[0] - 1.0).abs() < 1e-6, "the middle frame should own it");
    }

    /// Where a frame is *both* well exposed and detailed, its contrast term makes it win — which is
    /// what fusion is for: keeping the detail each exposure holds.
    #[test]
    fn a_detailed_frame_wins_over_a_flat_one() {
        // Both sit at mid-grey, so only contrast separates them.
        let flat_mid = flat(0.5);
        let checker = Frame::from_linear_rec2020(
            Rgb32f::from_fn(16, 12, |x, y| {
                let v = if (x + y) % 2 == 0 { 0.35 } else { 0.65 };
                [v, v, v]
            }),
            None,
        );
        let r = fuse(vec![flat_mid, checker]);
        // The fused value must carry the checker's alternation, not sit on the flat frame's 0.5.
        let a = r.image.data[0][0];
        let b = r.image.data[1][0];
        assert!((a - b).abs() > 0.05, "the checkerboard should survive: {a} vs {b}");
        assert!((a - 0.5).abs() > 0.02 || (b - 0.5).abs() > 0.02, "not flattened to mid-grey");
    }

    /// Fewer than two frames is not a fusion.
    #[test]
    fn one_frame_is_too_few() {
        let err = fuse_exposures(vec![flat(0.5)], &FuseOptions::default(), &crate::no_progress).expect_err("one frame cannot be fused");
        assert!(err.to_string().contains("at least 2"), "{err}");
    }

    /// Different sizes cannot be fused, and the message names the odd frame.
    #[test]
    fn mismatched_sizes_are_refused() {
        let a = flat(0.5);
        let b = Frame::from_linear_rec2020(Rgb32f::from_fn(4, 4, |_, _| [0.5, 0.5, 0.5]), None);
        let err = fuse_exposures(vec![a, b], &FuseOptions::default(), &crate::no_progress).expect_err("sizes differ");
        assert!(err.to_string().contains("4x4"), "{err}");
    }
}
