//! Drizzle: sub-pixel dithered frames mapped onto a finer output grid by area-weighted accumulation.
//!
//! This is how astrophotography — and any dithered burst — recovers resolution past the sensor's own
//! grid (Fruchter & Hook's method). Each input pixel is treated as a square "drop" smaller than one
//! input pixel, mapped through the frame's offset, and its value is added to every output pixel whose
//! area it overlaps, weighted by that overlap. Shrinking the drop below one input pixel is the whole
//! trick: a full-size drop would blur each sample back over a whole output cell and recover nothing.
//!
//! **Conventions, stated once so they cannot drift.** Input pixel `(x, y)` covers the input-space square
//! `[x, x+1] x [y, y+1]`. The reference frame is the one whose offset is `(0, 0)`, and `shifts[f]` is
//! where frame `f` content sits in the reference input coordinates: a positive `sx` means the frame
//! content is displaced towards `+x`, so the reference coordinate of that pixel centre is
//! `x + 0.5 + sx`, which lands at output pixel coordinate `(x + 0.5 + sx) * scale`.
//!
//! Two things this module deliberately does *not* do. It does not resample a frame first — the
//! accumulation *is* the resampling, which is why a drizzle of four dithered frames beats an upsample of
//! one. And it accumulates serially across frames: a scatter into a shared grid cannot be parallelised
//! without one output-sized buffer per thread, and a drizzle is a handful of frames.
//!
//! Algorithms are our own from the published method; no code from any GPL project was consulted.

use crate::align::Alignment;
use crate::frame::{Frame, FrameColor};
use crate::{MergeError, Progress, Result, check};
use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::{Plane, Rgb32f};

/// What a drizzle should do.
#[derive(Debug, Clone)]
pub struct DrizzleOptions {
    /// Linear scale of the output grid: 2 makes an image twice as wide and tall.
    pub scale: usize,
    /// Drop size in input pixels, at most 1: the shrunken footprint each sample accumulates with. 0.8 is
    /// the usual starting point; smaller keeps more resolution and leaves more output pixels uncovered.
    pub drop: f32,
    /// The offset of each frame in input pixels, relative to the reference (see the module docs).
    ///
    /// Empty means the offsets are unknown and the frames are registered instead, which recovers a
    /// translation from the engine feature alignment. A dithered capture usually knows its offsets from
    /// the mount or the stage, and those are exact where a feature fit is approximate.
    pub shifts: Vec<(f64, f64)>,
}

impl Default for DrizzleOptions {
    fn default() -> Self {
        DrizzleOptions { scale: 2, drop: 0.8, shifts: Vec::new() }
    }
}

#[derive(Debug, Clone)]
pub struct DrizzleResult {
    /// The drizzled image, `scale` times the input in each dimension.
    pub image: Rgb32f,
    /// Summed drop weight per output pixel. Zero means no input sample reached that pixel — at the
    /// border and between drops at a small drop size — so a caller can tell real black from unmeasured.
    pub coverage: Plane,
    /// The overlap weight each frame contributed, summed: how much of the result is each frame.
    pub weights: Vec<f64>,
    pub alignments: Vec<Alignment>,
    pub reference: usize,
    pub color: FrameColor,
    pub raw: bool,
    pub orientation: Orientation,
    pub metadata: Metadata,
    pub baseline_exposure: f64,
}

/// The overlap, along one axis, between the output pixel `[i, i+1]` and the drop `[lo, hi]`. Zero when
/// they do not meet, which is what makes the accumulation a weighted sum of real areas.
fn overlap(i: usize, lo: f64, hi: f64) -> f64 {
    let i = i as f64;
    (i + 1.0).min(hi) - i.max(lo)
}

/// Drizzle a set of dithered frames onto a finer grid.
///
/// # Errors
/// [`MergeError::TooFew`] with fewer than two frames; [`MergeError::Mismatch`] when the frames differ in
/// size, or when an explicit shift list does not have one entry per frame.
pub fn drizzle(frames: Vec<Frame>, opts: &DrizzleOptions, progress: &Progress) -> Result<DrizzleResult> {
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
    if !opts.shifts.is_empty() && opts.shifts.len() != n {
        return Err(MergeError::Mismatch(format!("{n} frames but {} shifts: they must correspond one to one", opts.shifts.len())));
    }
    let scale = opts.scale.clamp(1, 6);
    let reference = n / 2;

    let mut alignments = vec![Alignment::identity(); n];
    let shifts: Vec<(f64, f64)> = if opts.shifts.is_empty() {
        check(progress, 0.05, "Aligning")?;
        let g0 = 0.18 / crate::align::median_luma(&frames[reference].image, frames[reference].clip).max(1e-6);
        let feats: Vec<(crate::features::Features, f64)> = frames
            .iter()
            .map(|f| {
                let (p, s) = crate::align::work_image(&f.image, g0, crate::align::WORK_EDGE);
                (crate::features::detect(&p, 1500), s)
            })
            .collect();
        let mut out = vec![(0.0, 0.0); n];
        for i in 0..n {
            if i == reference {
                continue;
            }
            let a = crate::align::align_pair(&feats[reference].0, &feats[i].0, feats[i].1, w, h, i as u64 + 1);
            // Only the translation is taken: a dither is a shift, and a full homography would move
            // pixels out of the drop model this accumulation is built on.
            let p = a.h.apply(lightcraft_geom::Point::new(0.0, 0.0));
            out[i] = (p.x, p.y);
            alignments[i] = a;
        }
        out
    } else {
        opts.shifts.clone()
    };
    check(progress, 0.4, "Drizzling")?;

    let (ow, oh) = (w * scale, h * scale);
    let mut sum = vec![[0.0f64; 3]; ow * oh];
    let mut weight = vec![0.0f64; ow * oh];
    let mut frame_weights = vec![0.0f64; n];
    let drop = f64::from(opts.drop.clamp(0.05, 1.0));
    let fscale = scale as f64;
    let half = drop * fscale / 2.0;
    for (f, (&(sx, sy), _frame)) in shifts.iter().zip(frames.iter()).enumerate() {
        for y in 0..h {
            let oy = (y as f64 + 0.5 - sy) * fscale;
            let (y0, y1) = (oy - half, oy + half);
            let py0 = y0.floor().max(0.0) as usize;
            let py1 = (y1.ceil() as usize).min(oh.saturating_sub(1));
            for x in 0..w {
                let ox = (x as f64 + 0.5 - sx) * fscale;
                let (x0, x1) = (ox - half, ox + half);
                let px0 = x0.floor().max(0.0) as usize;
                let px1 = (x1.ceil() as usize).min(ow.saturating_sub(1));
                let src = frames[f].image.data[y * w + x];
                for py in py0..=py1 {
                    let ay = overlap(py, y0, y1);
                    if ay <= 0.0 {
                        continue;
                    }
                    for px in px0..=px1 {
                        let ax = overlap(px, x0, x1);
                        if ax <= 0.0 {
                            continue;
                        }
                        let area = ax * ay;
                        let i = py * ow + px;
                        for c in 0..3 {
                            sum[i][c] += area * f64::from(src[c]);
                        }
                        weight[i] += area;
                        frame_weights[f] += area;
                    }
                }
            }
        }
        let done = 0.4 + 0.5 * (f as f32 + 1.0) / n as f32;
        check(progress, done, "Drizzling")?;
    }
    check(progress, 0.95, "Drizzling")?;

    let mut image = Rgb32f::new(ow, oh);
    let mut coverage = Plane::new(ow, oh);
    for i in 0..ow * oh {
        let wsum = weight[i];
        coverage.data[i] = wsum as f32;
        if wsum > 0.0 {
            image.data[i] = [(sum[i][0] / wsum) as f32, (sum[i][1] / wsum) as f32, (sum[i][2] / wsum) as f32];
        }
    }

    let rf = &frames[reference];
    Ok(DrizzleResult {
        image,
        coverage,
        weights: frame_weights,
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

    fn frame_of(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Frame {
        Frame::from_linear_rec2020(Rgb32f::from_fn(w, h, f), None)
    }

    /// A pattern whose period is three input pixels, i.e. *finer* than the frames sample: each frame
    /// takes every second pixel, so one frame aliases this pattern and no amount of upsample recovers
    /// it, while four dithered frames — whose offsets put samples on both phases — can. That is the
    /// regime a drizzle is for. A coarser pattern would be a poor test: at a four-pixel period the
    /// unshifted frame samples the pattern exactly and a single upsample already reconstructs it, which
    /// is precisely what the first version of this test measured.
    fn pattern(w: usize, h: usize) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            let v = if (x / 3 + y / 3).is_multiple_of(2) { 0.8 } else { 0.2 };
            let tint = x as f32 / w as f32 * 0.1;
            [v + tint, v, v - tint]
        })
    }

    /// Four frames at the classic half-pixel dither, each a 2x downsample of `base` shifted by the
    /// dither. `dx`/`dy` are in frame pixels, and the base is sampled at twice them.
    fn dithered_frames(base: &Rgb32f) -> (Vec<Frame>, Vec<(f64, f64)>) {
        let (w, h) = (base.width / 2, base.height / 2);
        let offsets = [(0.0f32, 0.0f32), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)];
        let frames = offsets
            .iter()
            .map(|&(dx, dy)| frame_of(w, h, |x, y| base.sample_bilinear(2.0 * (x as f32 + 0.5 + dx), 2.0 * (y as f32 + 0.5 + dy))))
            .collect();
        let shifts = offsets.iter().map(|&(dx, dy)| (-f64::from(dx), -f64::from(dy))).collect();
        (frames, shifts)
    }

    fn mean_abs_error(a: &Rgb32f, b: &Rgb32f) -> f32 {
        let mut sum = 0.0f64;
        let mut n = 0.0f64;
        for (pa, pb) in a.data.iter().zip(b.data.iter()) {
            for c in 0..3 {
                sum += f64::from((pa[c] - pb[c]).abs());
                n += 1.0;
            }
        }
        (sum / n) as f32
    }

    // ---- the pieces, before the end-to-end claim ------------------------------------------------

    /// A full-size drop at scale 1 is an identity: every input pixel maps onto exactly one output pixel
    /// and covers exactly its area. If the overlap arithmetic or the centre convention were wrong, this
    /// returns something other than the input.
    #[test]
    fn a_full_drop_at_scale_one_reproduces_the_input() {
        let (w, h) = (16, 12);
        let a = frame_of(w, h, |x, y| [x as f32 / 16.0, y as f32 / 12.0, 0.4]);
        let b = frame_of(w, h, |x, y| [x as f32 / 16.0, y as f32 / 12.0, 0.4]);
        let opts = DrizzleOptions { scale: 1, drop: 1.0, shifts: vec![(0.0, 0.0), (0.0, 0.0)] };
        let r = drizzle(vec![a.clone(), b], &opts, &crate::no_progress).expect("two frames drizzle");
        assert_eq!((r.image.width, r.image.height), (w, h), "scale 1 keeps the grid");
        let e = mean_abs_error(&r.image, &a.image);
        assert!(e < 1e-4, "a full drop at scale 1 must reproduce the input, error {e}");
    }

    /// A whole-pixel shift is aligned, not ghosted: with one frame displaced by an integer and its
    /// offset given, the result is the undisplaced content. A sign error would double the displacement
    /// and blur it.
    #[test]
    fn a_whole_pixel_shift_is_aligned_not_ghosted() {
        let (w, h) = (16, 12);
        let v = |x: usize, y: usize| if (x / 4 + y / 4).is_multiple_of(2) { 0.8f32 } else { 0.2 };
        let a = frame_of(w, h, |x, y| [v(x, y), 0.5, 0.5]);
        // b is a shifted one pixel right and down: b(x, y) = a(x - 1, y - 1)
        let b = frame_of(w, h, |x, y| [v(x.saturating_sub(1), y.saturating_sub(1)), 0.5, 0.5]);
        let opts = DrizzleOptions { scale: 1, drop: 1.0, shifts: vec![(0.0, 0.0), (1.0, 1.0)] };
        let r = drizzle(vec![a.clone(), b], &opts, &crate::no_progress).expect("drizzles");
        // The interior must match the reference: the shift was removed, not applied twice.
        let mut worst = 0.0f32;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                worst = worst.max((r.image.data[i][0] - a.image.data[i][0]).abs());
            }
        }
        assert!(worst < 1e-4, "the shifted frame must land on the reference, worst {worst}");
    }

    // ---- and then the claim the feature exists for ----------------------------------------------

    /// The point of drizzle: four dithered frames beat a plain upsample of one. This is the assertion
    /// that fails if the drop is not shrunk or the accumulation is not by overlap.
    #[test]
    fn a_drizzle_beats_an_upsample_of_one_frame() {
        let base = pattern(64, 48);
        let (frames, shifts) = dithered_frames(&base);
        // drop 0.5 spans exactly one output pixel at scale 2: the drops then tile the finer grid
        // instead of overlapping it, which is the regime where a dither adds resolution rather than
        // just averaging. The coverage test below still uses 0.8, where overlap is the point.
        let opts = DrizzleOptions { scale: 2, drop: 0.5, shifts };
        let r = drizzle(frames.clone(), &opts, &crate::no_progress).expect("four frames drizzle");
        assert_eq!((r.image.width, r.image.height), (base.width, base.height), "scale 2 doubles the grid");

        // A plain bilinear upsample of the first frame, for comparison.
        let up = Rgb32f::from_fn(base.width, base.height, |x, y| frames[0].image.sample_bilinear((x as f32 + 0.5) / 2.0, (y as f32 + 0.5) / 2.0));
        let (d, u) = (mean_abs_error(&r.image, &base), mean_abs_error(&up, &base));
        assert!(d < u, "the drizzle must be closer to the truth than an upsample: {d} vs {u}");
        // A relative bar, and a bar with a *measured* basis rather than a guessed one. This test first
        // went in with `d < 0.05` (arbitrary) and then `d * 2 < u` (also arbitrary); both were guesses
        // about what the algorithm should achieve, and neither said anything about whether it worked.
        // Measured here: 0.161 versus 0.222, so the drizzle removes 27% of the upsample's error. The bar
        // is 0.85, which leaves room for the pattern and the machine while still failing outright when
        // the drizzle does nothing — the state this test was in for two rounds, at a ratio of 1.000.
        assert!(d < u * 0.85, "and by a real margin, measured at ~0.73 here: {d} vs {u}");
    }

    /// Coverage tells a caller which output pixels no sample reached, so unmeasured black can be told
    /// apart from real black — and every frame carries weight, or the dither was not used.
    #[test]
    fn coverage_and_frame_weights_are_reported() {
        let base = pattern(32, 24);
        let (frames, shifts) = dithered_frames(&base);
        let opts = DrizzleOptions { scale: 2, drop: 0.8, shifts };
        let r = drizzle(frames, &opts, &crate::no_progress).expect("drizzles");
        let interior = r.coverage.data[20 * r.coverage.width + 20];
        assert!(interior > 0.0, "the interior must be covered, got {interior}");
        assert_eq!(r.weights.len(), 4, "one weight per frame");
        assert!(r.weights.iter().all(|w| *w > 0.0), "every dithered frame must contribute: {:?}", r.weights);
    }

    /// A shift list that does not match the frames is a mistake, not something to guess at.
    #[test]
    fn a_mismatched_shift_list_is_refused() {
        let base = pattern(16, 12);
        let (frames, _) = dithered_frames(&base);
        let opts = DrizzleOptions { scale: 2, drop: 0.8, shifts: vec![(0.0, 0.0)] };
        let err = drizzle(frames, &opts, &crate::no_progress).expect_err("four frames, one shift");
        assert!(err.to_string().contains("one to one"), "{err}");
    }

    /// Fewer than two frames is not a dither.
    #[test]
    fn one_frame_is_too_few() {
        let err = drizzle(vec![frame_of(8, 6, |_, _| [0.5, 0.5, 0.5])], &DrizzleOptions::default(), &crate::no_progress)
            .expect_err("one frame cannot be drizzled");
        assert!(err.to_string().contains("at least 2"), "{err}");
    }
}
