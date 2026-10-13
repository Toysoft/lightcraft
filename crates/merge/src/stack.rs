//! Statistical stacking of a burst: many frames of the *same* exposure combined per pixel with
//! outlier rejection. This is both the engine's multi-frame burst denoise and its statistical /
//! robust stacking — the two differ in how many frames they expect and how hard they reject, not in
//! the operation, so they share this core.
//!
//! A burst is one exposure by definition, so unlike the HDR merge there is no brightness ordering and
//! no exposure normalisation: frames are combined as they are. Averaging different exposures would
//! silently produce a wrong image, so [`stack_burst`] refuses frames whose EXIF exposures span more
//! than [`EXPOSURE_TOLERANCE`] EV and says to use the HDR merge instead.
//!
//! Alignment reuses [`crate::align`] — the same registration the HDR merge uses — because a handheld
//! burst moves exactly like a handheld bracket. The algorithm is the classical sigma-clipped mean
//! (a "robust" stack in astrophotography, where it is also how hot pixels and satellites are
//! removed); no code from any GPL project was consulted.

use crate::align::{self, Alignment};
use crate::frame::{Frame, FrameColor};
use crate::{MergeError, Progress, Result, check};
use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

/// Frames whose EXIF exposures span more than this (EV) are an exposure bracket, not a burst.
pub const EXPOSURE_TOLERANCE: f64 = 0.25;

/// How the frames are combined per pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StackMode {
    /// Arithmetic mean. The lowest noise for a perfectly static scene, but one moving object or one
    /// hot pixel drags the result.
    Mean,
    /// Per-pixel median. Rejects anything not present in the majority of frames, at the cost of
    /// sharpness: a median of N samples keeps roughly the noise of a single one.
    Median,
    /// Sigma-clipped mean (the default). Keeps the mean's 1/√N noise floor while rejecting outliers:
    /// samples further than [`StackOptions::sigma`] standard deviations from the per-pixel mean are
    /// dropped and the mean retaken.
    #[default]
    Robust,
}

/// What a stack of a burst should do.
#[derive(Debug, Clone)]
pub struct StackOptions {
    /// Register the frames to the reference before combining (a handheld burst moves).
    pub align: bool,
    pub mode: StackMode,
    /// [`StackMode::Robust`] only: reject samples further than this many standard deviations from the
    /// per-pixel mean. 3.0 keeps an edge every frame agrees on and drops a passer-by only some frames
    /// see.
    pub sigma: f32,
}

impl Default for StackOptions {
    fn default() -> Self {
        StackOptions { align: true, mode: StackMode::Robust, sigma: 3.0 }
    }
}

/// The stacked image and what it was made from.
#[derive(Debug, Clone)]
pub struct StackResult {
    pub image: Rgb32f,
    /// Per-pixel count of frames that survived rejection (`0..=frames.len()`). A count well below the
    /// burst length is where something moved, which is what a wipe-between-frames view would show.
    pub used: Plane,
    pub alignments: Vec<Alignment>,
    pub reference: usize,
    pub color: FrameColor,
    pub raw: bool,
    pub orientation: Orientation,
    pub metadata: Metadata,
    pub baseline_exposure: f64,
}

/// How far one sample sits from the current estimate, as the largest per-channel deviation.
fn deviation(s: &[f32; 3], m: &[f32; 3]) -> f32 {
    (0..3).map(|c| (s[c] - m[c]).abs()).fold(0.0, f32::max)
}

fn mean_of(samples: &[[f32; 3]], keep: Option<&[bool]>) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    let mut n = 0.0f64;
    for (i, s) in samples.iter().enumerate() {
        if keep.is_some_and(|k| !k[i]) {
            continue;
        }
        for c in 0..3 {
            sum[c] += f64::from(s[c]);
        }
        n += 1.0;
    }
    if n == 0.0 {
        return [0.0; 3];
    }
    [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32]
}

fn per_channel_median(samples: &[[f32; 3]], scratch: &mut Vec<f32>) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        scratch.clear();
        for s in samples {
            scratch.push(s[c]);
        }
        scratch.sort_by(f32::total_cmp);
        let n = scratch.len();
        out[c] = if n == 0 {
            0.0
        } else if n % 2 == 1 {
            scratch[n / 2]
        } else {
            0.5 * (scratch[n / 2 - 1] + scratch[n / 2])
        };
    }
    out
}

/// The samples' own scale about `centre`, from the median absolute deviation scaled by 1.4826 so it
/// estimates a normal distribution's σ. The MAD rather than the standard deviation, because a single
/// frame from a different scene inflates the standard deviation enough to hide itself — with five
/// frames it sits at roughly 2σ, so a 3σ cut keeps it — while the MAD barely moves. This is the
/// classical astrophotography stack.
fn mad_scale(samples: &[[f32; 3]], centre: &[f32; 3], scratch: &mut Vec<f32>) -> f32 {
    scratch.clear();
    for s in samples {
        scratch.push(deviation(s, centre));
    }
    if scratch.is_empty() {
        return 0.0;
    }
    scratch.sort_by(f32::total_cmp);
    let n = scratch.len();
    let mad = if n % 2 == 1 { scratch[n / 2] } else { 0.5 * (scratch[n / 2 - 1] + scratch[n / 2]) };
    1.4826 * mad
}

/// Combine one pixel's samples, returning the value and how many samples were kept.
///
/// `samples` holds only the frames that passed the alignment's validity mask, so it can be shorter
/// than the burst; it is never empty here.
fn combine(samples: &[[f32; 3]], mode: StackMode, sigma: f32, scratch: &mut Vec<f32>) -> ([f32; 3], f32) {
    let k = samples.len();
    if k == 1 {
        return (samples[0], 1.0);
    }
    match mode {
        StackMode::Mean => (mean_of(samples, None), k as f32),
        StackMode::Median => (per_channel_median(samples, scratch), k as f32),
        StackMode::Robust => {
            // Clip around the *median* with a MAD scale, twice: the first estimate includes the
            // outlier, which moves a median far less than a mean, and the scale comes from the MAD
            // rather than the standard deviation so the outlier cannot mask itself.
            let mut keep = vec![true; k];
            let mut centre = per_channel_median(samples, scratch);
            let mut kept = k as f32;
            for _ in 0..2 {
                let cut = (sigma * mad_scale(samples, &centre, scratch)).max(1e-6);
                let mut survivors = 0.0f32;
                for (i, s) in samples.iter().enumerate() {
                    // A sample is far when *any* channel is beyond the cut.
                    let far = deviation(s, &centre) > cut;
                    keep[i] = !far;
                    if !far {
                        survivors += 1.0;
                    }
                }
                if survivors == 0.0 {
                    // Refusing every sample would be worse than not rejecting: keep the median.
                    return (centre, k as f32);
                }
                kept = survivors;
                let survivors_only: Vec<[f32; 3]> = samples.iter().zip(&keep).filter(|(_, k)| **k).map(|(s, _)| *s).collect();
                centre = per_channel_median(&survivors_only, scratch);
            }
            (mean_of(samples, Some(&keep)), kept)
        }
    }
}

/// Combine a burst of same-exposure frames into one image.
///
/// # Errors
/// [`MergeError::TooFew`] with fewer than two frames; [`MergeError::Mismatch`] when the frames are
/// not all the same size, or when their EXIF exposures say this is a bracket rather than a burst.
pub fn stack_burst(mut frames: Vec<Frame>, opts: &StackOptions, progress: &Progress) -> Result<StackResult> {
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
    if let Some(ev) = frames.iter().map(|f| f.exposure).collect::<Option<Vec<f64>>>() {
        let lo = ev.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = ev.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if hi - lo > EXPOSURE_TOLERANCE {
            return Err(MergeError::Mismatch(format!(
                "these frames span {:.1} EV, so they are an exposure bracket rather than a burst: \
                 merge them as HDR instead",
                hi - lo
            )));
        }
    }

    // The middle frame is the reference: a burst has no brightness order to sort by, and the middle
    // frame is the one the others are most likely to still overlap after hand movement.
    let reference = n / 2;

    // `None` means "every pixel valid" (the reference), `Some(mask)` traces the warp.
    let mut mask: Vec<Option<Vec<bool>>> = vec![None; n];
    let mut alignments = vec![Alignment::identity(); n];
    if opts.align {
        check(progress, 0.05, "Aligning")?;
        // Normalise to middle grey before detecting features, as the HDR merge does: features have to
        // be found on the scene, not on the exposure.
        let g0 = 0.18 / align::median_luma(&frames[reference].image, frames[reference].clip).max(1e-6);
        let feats: Vec<(crate::features::Features, f64)> = frames
            .par_iter()
            .map(|f| {
                let (p, s) = align::work_image(&f.image, g0, align::WORK_EDGE);
                (crate::features::detect(&p, 1500), s)
            })
            .collect();
        check(progress, 0.25, "Aligning")?;
        let found: Vec<(usize, Alignment)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != reference)
            .map(|i| (i, align::align_pair(&feats[reference].0, &feats[i].0, feats[i].1, w, h, i as u64 + 1)))
            .collect();
        for (i, a) in found {
            alignments[i] = a;
        }
        check(progress, 0.35, "Resampling")?;
        let warped: Vec<(usize, Rgb32f, Vec<bool>)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != reference && alignments[i].model != "identity")
            .map(|i| {
                let (img, v) = align::warp(&frames[i].image, &alignments[i].h);
                (i, img, v)
            })
            .collect();
        for (i, img, v) in warped {
            frames[i].image = img;
            mask[i] = Some(v);
        }
    }
    check(progress, 0.5, "Combining")?;

    let mut out = Rgb32f::new(w, h);
    let mut used = Plane::new(w, h);
    out.data.par_chunks_mut(w).zip(used.data.par_chunks_mut(w)).enumerate().for_each(|(y, (row, urow))| {
        let mut samples: Vec<[f32; 3]> = Vec::with_capacity(n);
        let mut scratch: Vec<f32> = Vec::with_capacity(n);
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            samples.clear();
            for f in 0..n {
                if mask[f].as_ref().is_some_and(|v| !v[i]) {
                    continue;
                }
                samples.push(frames[f].image.data[i]);
            }
            if samples.is_empty() {
                *o = [0.0; 3];
                urow[x] = 0.0;
            } else {
                let (v, k) = combine(&samples, opts.mode, opts.sigma, &mut scratch);
                *o = v;
                urow[x] = k;
            }
        }
    });
    check(progress, 0.95, "Combining")?;

    let rf = &frames[reference];
    Ok(StackResult {
        image: out,
        used,
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

    fn frame_at(v: f32) -> Frame {
        Frame::from_linear_rec2020(Rgb32f::from_fn(8, 6, |_, _| [v, v, v]), None)
    }

    fn first(r: &StackResult) -> f32 {
        r.image.data[0][0]
    }

    /// Identical frames average to themselves, which is the floor every other mode is measured from.
    #[test]
    fn the_mean_of_identical_frames_is_that_frame() {
        let frames = vec![frame_at(0.4), frame_at(0.4), frame_at(0.4)];
        let opts = StackOptions { align: false, mode: StackMode::Mean, sigma: 3.0 };
        let r = stack_burst(frames, &opts, &crate::no_progress).expect("3 frames stack");
        assert!((first(&r) - 0.4).abs() < 1e-6, "{}", first(&r));
        assert!((r.used.data[0] - 3.0).abs() < 1e-6, "every frame should be counted");
        assert_eq!(r.reference, 1, "the middle frame is the reference");
    }

    /// The whole point of a median: one frame that disagrees is dropped rather than averaged in.
    #[test]
    fn the_median_drops_a_frame_that_disagrees() {
        let frames = vec![frame_at(0.5), frame_at(0.5), frame_at(5.0)];
        let median = StackOptions { align: false, mode: StackMode::Median, sigma: 3.0 };
        let r = stack_burst(frames.clone(), &median, &crate::no_progress).expect("stacks");
        assert!((first(&r) - 0.5).abs() < 1e-6, "median {}", first(&r));

        // The mean of the same three is dragged towards the outlier, which is what sigma-clipping is
        // there to avoid without the median's sharpness cost.
        let mean = StackOptions { align: false, mode: StackMode::Mean, sigma: 3.0 };
        let m = stack_burst(frames, &mean, &crate::no_progress).expect("stacks");
        assert!((first(&m) - 2.0).abs() < 1e-6, "mean {}", first(&m));
    }

    /// Sigma-clipping keeps the mean's noise floor while dropping the outlier, and counts what it kept
    /// so the UI can show where something moved.
    #[test]
    fn sigma_clipping_rejects_one_frame_and_counts_the_rest() {
        // Four frames a hair apart (as a real burst is) and one that is not the same scene.
        let frames = vec![frame_at(0.500), frame_at(0.502), frame_at(0.498), frame_at(0.501), frame_at(9.0)];
        let opts = StackOptions { align: false, mode: StackMode::Robust, sigma: 3.0 };
        let r = stack_burst(frames, &opts, &crate::no_progress).expect("stacks");
        assert!((first(&r) - 0.5003).abs() < 2e-3, "robust {}", first(&r));
        assert!((r.used.data[0] - 4.0).abs() < 1e-6, "the odd frame must be counted out, kept {}", r.used.data[0]);
    }

    /// A bracket handed to the stacker is refused, because averaging different exposures is not a
    /// burst: it is an HDR merge, and this says so rather than producing a wrong image.
    #[test]
    fn an_exposure_bracket_is_refused() {
        let mut a = frame_at(0.5);
        let mut b = frame_at(0.5);
        a.exposure = Some(0.0);
        b.exposure = Some(2.0);
        let err = stack_burst(vec![a, b], &StackOptions::default(), &crate::no_progress).expect_err("a 2 EV span is a bracket");
        let text = err.to_string();
        assert!(text.contains("bracket") && text.contains("HDR"), "{text}");
    }

    /// Fewer than two frames is not a stack.
    #[test]
    fn one_frame_is_too_few() {
        let err = stack_burst(vec![frame_at(0.5)], &StackOptions::default(), &crate::no_progress).expect_err("one frame cannot be stacked");
        assert!(err.to_string().contains("at least 2"), "{err}");
    }

    /// Different sizes cannot be combined, and the message names the odd frame.
    #[test]
    fn mismatched_sizes_are_refused() {
        let a = frame_at(0.5);
        let b = Frame::from_linear_rec2020(Rgb32f::from_fn(4, 4, |_, _| [0.5, 0.5, 0.5]), None);
        let err = stack_burst(vec![a, b], &StackOptions::default(), &crate::no_progress).expect_err("sizes differ");
        assert!(err.to_string().contains("4x4"), "{err}");
    }
}
