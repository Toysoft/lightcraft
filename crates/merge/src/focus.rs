//! Focus stacking: frames of one scene at different focus distances, combined so every part of the
//! result comes from the frame that is sharpest there.
//!
//! Sharpness is measured as the local energy of the Laplacian of log-luminance — the Tenengrad-class
//! measure focus stacking uses — and smoothed over a neighbourhood, so one noisy pixel cannot claim a
//! region on its own. The frames are registered first, because a focus pull moves the framing slightly
//! even on a tripod.
//!
//! The combination is a weighted mean whose weights come from that measure, not a hard per-pixel
//! winner: a winner changes in steps, and at any non-trivial resolution those steps read as seams.
//! [`FocusOptions::select`] decides how decisively the sharpest frame wins, and the winning frame's
//! index is reported so a caller can show which frame each region came from.
//!
//! Algorithms are our own from the published method (a Laplacian sharpness measure plus a weighted
//! combination); no code from any GPL project was consulted.

use crate::align::{self, Alignment};
use crate::frame::{Frame, FrameColor};
use crate::{MergeError, Progress, Result, check};
use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

/// What a focus stack should do.
#[derive(Debug, Clone)]
pub struct FocusOptions {
    /// Register the frames to the reference before measuring.
    pub align: bool,
    /// How decisively the sharpest frame wins: 1 is a soft average of everything, higher approaches a
    /// hard selection. 6 keeps a smooth transition without smearing a sharp region into a soft one.
    pub select: f32,
    /// σ of the blur applied to each sharpness map, as a fraction of the long edge. Wide enough that a
    /// region is decided by its neighbourhood, narrow enough to keep the seam where focus changes to a
    /// few pixels.
    pub smooth: f32,
}

impl Default for FocusOptions {
    fn default() -> Self {
        FocusOptions { align: true, select: 6.0, smooth: 0.004 }
    }
}

#[derive(Debug, Clone)]
pub struct FocusResult {
    pub image: Rgb32f,
    /// The frame each pixel took most of its value from (`0..frames.len()`): which frame was sharpest
    /// there, which is what a focus-depth view would show.
    pub sharpest: Plane,
    pub alignments: Vec<Alignment>,
    pub reference: usize,
    pub color: FrameColor,
    pub raw: bool,
    pub orientation: Orientation,
    pub metadata: Metadata,
    pub baseline_exposure: f64,
}

/// Rec.709 grey, which is what the sharpness measure reads.
fn grey_of(px: [f32; 3]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// The Laplacian energy of `grey` at `(x, y)`, edges clamped so a border pixel is defined.
fn laplacian_energy(grey: &Plane, x: usize, y: usize) -> f32 {
    let w = grey.width;
    let h = grey.height;
    let at = |x: usize, y: usize| grey.data[y * w + x];
    let xm = x.saturating_sub(1);
    let xp = (x + 1).min(w.saturating_sub(1));
    let ym = y.saturating_sub(1);
    let yp = (y + 1).min(h.saturating_sub(1));
    (4.0 * at(x, y) - at(xm, y) - at(xp, y) - at(x, ym) - at(x, yp)).abs()
}

/// Combine frames of one scene taken at different focus distances.
///
/// # Errors
/// [`MergeError::TooFew`] with fewer than two frames; [`MergeError::Mismatch`] when the frames are not
/// all the same size.
pub fn stack_focus(mut frames: Vec<Frame>, opts: &FocusOptions, progress: &Progress) -> Result<FocusResult> {
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

    // The middle frame is the reference. A focus stack has no brightness order to sort by, and the
    // middle frame is the one the others most likely still line up with.
    let reference = n / 2;

    let mut alignments = vec![Alignment::identity(); n];
    if opts.align {
        check(progress, 0.05, "Aligning")?;
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
    check(progress, 0.4, "Measuring focus")?;

    let sigma = (opts.smooth * w.max(h) as f32).max(1.0);
    let measures: Vec<Plane> = (0..n)
        .into_par_iter()
        .map(|i| {
            let grey = frames[i].image.map(grey_of);
            let mut p = Plane::new(w, h);
            p.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    *o = laplacian_energy(&grey, x, y);
                }
            });
            gaussian(&p, sigma)
        })
        .collect();
    check(progress, 0.75, "Combining")?;

    let mut out = Rgb32f::new(w, h);
    let mut sharpest = Plane::new(w, h);
    out.data.par_chunks_mut(w).zip(sharpest.data.par_chunks_mut(w)).enumerate().for_each(|(y, (row, srow))| {
        // Per row, not per pixel: a 24 MP stack must not allocate 24M times.
        let mut weights = vec![0.0f32; n];
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let mut total = 0.0f32;
            let mut best = 0usize;
            let mut best_w = f32::NEG_INFINITY;
            for f in 0..n {
                let v = measures[f].data[i].powf(opts.select);
                weights[f] = v;
                total += v;
                if v > best_w {
                    best_w = v;
                    best = f;
                }
            }
            if total <= 0.0 {
                // No frame is sharp here (a perfectly flat region in every frame). Falling back to
                // the reference is honest: it is where the geometry and colour come from.
                *o = frames[reference].image.data[i];
                srow[x] = reference as f32;
                continue;
            }
            let mut acc = [0.0f32; 3];
            for f in 0..n {
                let v = weights[f];
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
    check(progress, 0.97, "Combining")?;

    let rf = &frames[reference];
    Ok(FocusResult {
        image: out,
        sharpest,
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

    /// A frame whose left half carries a fine pattern and whose right half is flat, or the reverse.
    /// This is what a focus stack is for: no single frame is sharp everywhere.
    fn half_sharp(w: usize, h: usize, sharp_left: bool) -> Frame {
        let img = Rgb32f::from_fn(w, h, |x, y| {
            let on_left = x < w / 2;
            let v = if on_left == sharp_left && (x + y) % 2 == 0 { 0.65 } else { 0.35 };
            [v, v, v]
        });
        Frame::from_linear_rec2020(img, None)
    }

    /// Column variance: high where the fine pattern is, low on a flat half.
    fn column_contrast(image: &Rgb32f, x: usize) -> f32 {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for y in 0..image.height {
            let v = image.data[y * image.width + x][0];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo
    }

    /// Each half comes back from the frame that was sharp there, which is the whole point.
    #[test]
    fn each_half_comes_from_the_frame_sharp_there() {
        let (w, h) = (32, 24);
        let a = half_sharp(w, h, true);
        let b = half_sharp(w, h, false);
        let opts = FocusOptions { align: false, ..FocusOptions::default() };
        let r = stack_focus(vec![a, b], &opts, &crate::no_progress).expect("two frames stack");
        let left = column_contrast(&r.image, 4);
        let right = column_contrast(&r.image, w - 4);
        assert!(left > 0.1, "the left half must carry the fine pattern, contrast {left}");
        assert!(right > 0.1, "the right half must too, contrast {right}");
    }

    /// The winning frame is reported per pixel, so a caller can show which frame each region came from.
    #[test]
    fn the_sharpest_frame_is_reported_per_pixel() {
        let (w, h) = (32, 24);
        let r = stack_focus(
            vec![half_sharp(w, h, true), half_sharp(w, h, false)],
            &FocusOptions { align: false, ..FocusOptions::default() },
            &crate::no_progress,
        )
        .expect("stacks");
        let left = r.sharpest.data[2 * w + 4];
        let right = r.sharpest.data[2 * w + (w - 4)];
        assert!(left != right, "different halves, different sources: {left} vs {right}");
    }

    /// Fewer than two frames is not a stack.
    #[test]
    fn one_frame_is_too_few() {
        let err = stack_focus(vec![half_sharp(8, 6, true)], &FocusOptions::default(), &crate::no_progress).expect_err("one frame cannot be stacked");
        assert!(err.to_string().contains("at least 2"), "{err}");
    }

    /// Different sizes cannot be combined.
    #[test]
    fn mismatched_sizes_are_refused() {
        let a = half_sharp(8, 6, true);
        let b = Frame::from_linear_rec2020(Rgb32f::from_fn(4, 4, |_, _| [0.5, 0.5, 0.5]), None);
        let err = stack_focus(vec![a, b], &FocusOptions::default(), &crate::no_progress).expect_err("sizes differ");
        assert!(err.to_string().contains("4x4"), "{err}");
    }
}
