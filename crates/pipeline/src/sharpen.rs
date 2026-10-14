//! Process V3's sharpening.
//!
//! - All three channels are scaled by one factor: only luminance is sharpened.
//! - The change is a filter of the square root of display luminance `q` whose lift of each detail
//!   size is `T(f) = exp(c·(G_s1(f) − G_s2(f)))` (Gaussians' responses at frequency `f`): an unsharp
//!   mask's at Amount 40, the finest detail lifted ever more steeply as Amount and Detail rise, and
//!   Radius moving the lift to coarser detail ([`shape`]).
//! - The change is capped softly, `L·tanh(Δ/L)` in units of `q` ([`limit`]): Detail 0 caps
//!   it hard (its halos are faint), Detail 100 least; the cap grows with Amount and Radius.
//! - Deep shadows are left alone: full sharpening from `q` ≈ 0.24 (4 stops below white) down to
//!   none at 0.046 (9 stops), straight in `q` ([`Q0`]).
//! - Masking keeps a pixel's sharpening by `E² / (E² + t²)`, `E` the gradient of `q` (per pixel of
//!   the photo) after a Gaussian of 1.5 px, averaged over 1 px, and `t` rising with Masking
//!   ([`mask_t`]).
//!
//! The renderer works on the photo's scene luminance before exposure (so its planes survive an
//! exposure drag): the band is the filter of `√Y`, and the per-pixel stage turns it into a change of
//! display `q` by the tone curve's slope there ([`gain`]). The filter is designed here
//! for each Amount, Radius, Detail and output scale: the 2-D kernel of `T − 1` (an inverse DFT),
//! cut to the smallest radius that keeps its response within 1.5 % of the target, then split into at
//! most four separable passes (its eigenvectors) that the CPU and the GPU run alike.

/// Largest reach (px) of a filter, its taps per pass and its number of passes.
pub const RMAX: usize = 16;
pub const TAPS: usize = 2 * RMAX + 1;
pub const PASSES: usize = 4;

/// Display `q` (square root of display luminance, SDR white 1) below which nothing is sharpened,
/// and the span above it over which sharpening reaches full strength.
pub const Q0: f32 = 0.046;
pub const QW: f32 = 0.19;

/// The largest lift any detail size is given: the shape at Amount 150, Radius 0.5 and Detail 100
/// would otherwise lift the finest detail 300-fold.
pub const TMAX: f32 = 40.0;

/// The shape's tables: Detail 0, 25, 50, 100 and Radius 0.5, 1, 2, 3.
const AT_DETAIL: [f32; 4] = [0.0, 25.0, 50.0, 100.0];
const AT_RADIUS: [f32; 4] = [0.5, 1.0, 2.0, 3.0];
/// `c` at Amount 40, Radius 1, by Detail; its factor by Radius; its power of Amount / 40.
const C_DETAIL: [f32; 4] = [0.522, 1.025, 1.376, 1.799];
const C_RADIUS: [f32; 4] = [1.186, 1.0, 0.793, 0.639];
const C_AMOUNT_POW: f32 = AMOUNT_POW;
/// How the filter's strength `c` grows with Amount (`(Amount / 40)^this`); local Sharpness scales a
/// band by the same power of its Amount over the designed one.
pub const AMOUNT_POW: f32 = 0.975;
/// `s2` (px of the photo) by Radius at Detail 25 and Amount ≤ 40; its factor by Detail; its factor
/// at Amount 100 and above (in between, in proportion).
const S2_RADIUS: [f32; 4] = [0.693, 1.086, 1.762, 2.401];
const S2_DETAIL: [f32; 4] = [1.327, 1.0, 0.899, 0.855];
const S2_AMOUNT_100: f32 = 0.891;
/// `s1` (px) by Radius (0 at Radius 1), plus this much at Detail 0 (none from 25).
const S1_RADIUS: [f32; 4] = [0.116, 0.0, 0.232, 0.410];
const S1_DETAIL_0: f32 = 0.154;
/// The cap on the change of `q` at Amount 40 and Radius 1, by Detail, and its factor by Radius.
const LIMIT_DETAIL: [f32; 4] = [0.017, 0.0505, 0.0583, 0.0686];
const LIMIT_RADIUS: [f32; 4] = [0.677, 1.0, 1.180, 1.150];
/// How the cap grows with Amount (`(Amount / 40)^this`, up to Amount 100).
pub const LIMIT_POW: f32 = 0.35;

/// Masking 25's threshold on the edge gradient (per pixel of the photo, in `q`), and how fast it
/// rises above (`× e^(k·(masking − 25))`); below 25 it falls in proportion to 0.
const MASK_T25: f32 = 0.0029;
const MASK_K: f32 = 0.0385;

/// The edge map's Gaussian before the gradient and its average after it (px of the photo).
pub const EDGE_PRE: f32 = 1.5;
pub const EDGE_POST: f32 = 1.0;

/// `xs`..`ys` read at `x` (clamped at both ends).
fn table(x: f32, xs: &[f32; 4], ys: &[f32; 4]) -> f32 {
    if x.is_nan() || x <= xs[0] {
        return ys[0];
    }
    for i in 1..4 {
        if x <= xs[i] {
            let t = (x - xs[i - 1]) / (xs[i] - xs[i - 1]);
            return ys[i - 1] + (ys[i] - ys[i - 1]) * t;
        }
    }
    ys[3]
}

/// The response's parameters at Amount `amount` (0..150), Radius `radius` (px), Detail `detail`
/// (0..100): `[c, s1, s2]`, sigmas in pixels of the photo.
pub fn shape(amount: f32, radius: f32, detail: f32) -> [f32; 3] {
    let a = if amount.is_finite() { amount.abs().min(150.0) } else { 40.0 };
    let c = table(detail, &AT_DETAIL, &C_DETAIL) * table(radius, &AT_RADIUS, &C_RADIUS) * (a / 40.0).powf(C_AMOUNT_POW);
    let at_a = 1.0 + (S2_AMOUNT_100 - 1.0) * ((a - 40.0) / 60.0).clamp(0.0, 1.0);
    let s2 = table(radius, &AT_RADIUS, &S2_RADIUS) * table(detail, &AT_DETAIL, &S2_DETAIL) * at_a;
    let d0 = if detail.is_finite() { (1.0 - detail / 25.0).clamp(0.0, 1.0) } else { 0.0 };
    let s1 = table(radius, &AT_RADIUS, &S1_RADIUS) + S1_DETAIL_0 * d0;
    [c, s1, s2]
}

/// The cap on the change of display `q` at Amount 40, Radius `radius` and Detail `detail`.
pub fn limit40(radius: f32, detail: f32) -> f32 {
    table(detail, &AT_DETAIL, &LIMIT_DETAIL) * table(radius, &AT_RADIUS, &LIMIT_RADIUS)
}

/// The cap at Amount `amount` from the one at 40 ([`limit40`]). The GPU's `finish` mirrors it.
#[inline]
pub fn limit(amount: f32, limit40: f32) -> f32 {
    let a = if amount.is_finite() { amount.abs().min(100.0) } else { 40.0 };
    limit40 * (a / 40.0).powf(LIMIT_POW)
}

/// Masking `masking`'s (0..100) threshold on the edge gradient, 0 for none.
pub fn mask_t(masking: f32) -> f32 {
    if masking.is_nan() || masking <= 0.0 {
        0.0
    } else if masking <= 25.0 {
        MASK_T25 * masking / 25.0
    } else {
        MASK_T25 * (MASK_K * (masking.min(100.0) - 25.0)).exp()
    }
}

/// A sharpening filter: `n` separable passes of `2r + 1` taps, horizontal then vertical, summed, plus
/// `centre` times the image (what the passes leave of the kernel's sum, so that flat areas stay
/// exactly as they are); designed for Amount `amount` (the per-pixel stage scales it for local
/// Sharpness).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    pub r: usize,
    pub n: usize,
    pub taps: [[[f32; TAPS]; 2]; PASSES],
    pub centre: f32,
    pub amount: f32,
}

impl Filter {
    /// Pass `i`'s horizontal and vertical taps.
    pub fn pass(&self, i: usize) -> Option<(&[f32], &[f32])> {
        let t = self.taps.get(i).filter(|_| i < self.n)?;
        let k = 2 * self.r.min(RMAX) + 1;
        Some((t[0].get(..k)?, t[1].get(..k)?))
    }

    /// Cache key of the band it makes.
    pub fn key(&self) -> u64 {
        let mut k = 0x5348_4152_5036u64 ^ ((self.r as u64) << 8) ^ (self.n as u64) ^ (u64::from(self.centre.to_bits()) << 20);
        for i in 0..self.n {
            if let Some((h, v)) = self.pass(i) {
                for t in h.iter().chain(v) {
                    k = k.rotate_left(13) ^ u64::from(t.to_bits()).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                }
            }
        }
        k
    }

    /// The filter's response at frequency `(fx, fy)` (cycles per pixel), for tests.
    pub fn response(&self, fx: f64, fy: f64) -> f64 {
        let at = |t: &[f32], f: f64| -> f64 {
            let r = t.len() / 2;
            t.iter().enumerate().map(|(k, &w)| f64::from(w) * (std::f64::consts::TAU * f * (k as f64 - r as f64)).cos()).sum()
        };
        (0..self.n).filter_map(|i| self.pass(i)).map(|(h, v)| at(h, fx) * at(v, fy)).sum::<f64>() + f64::from(self.centre)
    }
}

/// Grid of the design's DFT.
const N: usize = 64;

/// The target band response `T − 1` at squared frequency `f2` (cycles² per output px²).
fn target(c: f64, s1: f64, s2: f64, f2: f64) -> f64 {
    let g = |s: f64| (-2.0 * std::f64::consts::PI.powi(2) * s * s * f2).exp();
    (c * (g(s1) - g(s2))).exp() - 1.0
}

/// The filter for Amount `amount`, Radius `radius`, Detail `detail` at `scale` output pixels per
/// pixel of the photo, or `None` when it would change nothing visible.
pub fn design(amount: f32, radius: f32, detail: f32, scale: f32) -> Option<Filter> {
    let [c, s1, s2] = shape(amount, radius, detail);
    let scale = if scale.is_finite() { scale.clamp(0.01, 4.0) } else { 1.0 };
    let (s1, s2) = (f64::from(s1 * scale), f64::from(s2 * scale));
    let mut c = f64::from(c);
    if !(c.is_finite() && c > 0.0 && s2 > s1) {
        return None;
    }
    // the largest lift (on an axis at the Nyquist frequency) capped
    let top = target(1.0, s1, s2, 0.25).ln_1p();
    if top > 0.0 && c * top > f64::from(TMAX).ln() {
        c = f64::from(TMAX).ln() / top;
    }
    let freq = |u: usize| if u <= N / 2 { u as f64 / N as f64 } else { (u as f64 - N as f64) / N as f64 };
    let h: Vec<f64> = (0..N * N).map(|i| target(c, s1, s2, freq(i / N).powi(2) + freq(i % N).powi(2))).collect();
    let lift = (0..=N / 2)
        .flat_map(|u| (0..=N / 2).map(move |v| (u, v)))
        .filter(|&(u, v)| u * u + v * v <= N * N / 4)
        .map(|(u, v)| h.get(u * N + v).copied().unwrap_or(0.0).abs())
        .fold(0.0f64, f64::max);
    if lift < 1e-4 {
        return None;
    }
    let cos: Vec<f64> = (0..N).map(|k| (std::f64::consts::TAU * k as f64 / N as f64).cos()).collect();
    let cs = |a: usize, b: usize| cos.get((a * b) % N).copied().unwrap_or(1.0);
    // the kernel k(x, y), x, y in 0..=RMAX (it is even in both): the inverse DFT of an even response
    let rm = RMAX;
    let mut part = vec![0.0f64; N * (rm + 1)];
    for u in 0..N {
        for y in 0..=rm {
            part[u * (rm + 1) + y] = (0..N).map(|v| h.get(u * N + v).copied().unwrap_or(0.0) * cs(v, y)).sum();
        }
    }
    let mut k = vec![0.0f64; (rm + 1) * (rm + 1)];
    for x in 0..=rm {
        for y in 0..=rm {
            k[x * (rm + 1) + y] = (0..N).map(|u| part[u * (rm + 1) + y] * cs(u, x)).sum::<f64>() / (N * N) as f64;
        }
    }
    let kat = |x: usize, y: usize| k.get(x * (rm + 1) + y).copied().unwrap_or(0.0);
    // the smallest radius whose cut kernel (centre corrected: flat areas stay as they are) responds
    // within 1.5 % of the largest lift
    let mut r = rm;
    for cand in 1..=rm {
        let total: f64 = (0..=cand)
            .flat_map(|x| (0..=cand).map(move |y| (x, y)))
            .map(|(x, y)| kat(x, y) * if x > 0 { 2.0 } else { 1.0 } * if y > 0 { 2.0 } else { 1.0 })
            .sum();
        let kc = |x: usize, y: usize| if x == 0 && y == 0 { kat(0, 0) - total } else { kat(x, y) };
        let mut worst = 0.0f64;
        let mut b = vec![0.0f64; (cand + 1) * (N / 2 + 1)];
        for x in 0..=cand {
            for v in 0..=N / 2 {
                b[x * (N / 2 + 1) + v] = (0..=cand).map(|y| kc(x, y) * if y > 0 { 2.0 } else { 1.0 } * cs(v, y)).sum();
            }
        }
        for u in 0..=N / 2 {
            for v in 0..=N / 2 {
                if u * u + v * v > N * N / 4 {
                    continue;
                }
                let got: f64 = (0..=cand).map(|x| b[x * (N / 2 + 1) + v] * if x > 0 { 2.0 } else { 1.0 } * cs(u, x)).sum();
                worst = worst.max((got - h.get(u * N + v).copied().unwrap_or(0.0)).abs());
            }
        }
        if worst <= 0.015 * lift {
            r = cand;
            break;
        }
    }
    // the cut kernel as a symmetric matrix (rows: vertical offset), split by its eigenvectors
    let n = 2 * r + 1;
    let total: f64 = (0..=r)
        .flat_map(|x| (0..=r).map(move |y| (x, y)))
        .map(|(x, y)| kat(x, y) * if x > 0 { 2.0 } else { 1.0 } * if y > 0 { 2.0 } else { 1.0 })
        .sum();
    let mut m = vec![0.0f64; n * n];
    for i in 0..n {
        for j in 0..n {
            let (y, x) = (i.abs_diff(r), j.abs_diff(r));
            m[i * n + j] = if x == 0 && y == 0 { kat(0, 0) - total } else { kat(x, y) };
        }
    }
    let norm2: f64 = m.iter().map(|v| v * v).sum();
    let (vals, vecs) = jacobi(m, n);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| vals[b].abs().total_cmp(&vals[a].abs()));
    let mut out = Filter { r, n: 0, taps: [[[0.0; TAPS]; 2]; PASSES], centre: 0.0, amount: amount.abs() };
    let mut kept = 0.0f64;
    for (slot, &e) in order.iter().take(PASSES).enumerate() {
        let lam = vals[e];
        for i in 0..n {
            let v = vecs[i * n + e];
            out.taps[slot][0][i] = v as f32;
            out.taps[slot][1][i] = (lam * v) as f32;
        }
        out.n = slot + 1;
        kept += lam * lam;
        if norm2 - kept <= 1e-6 * norm2 {
            break;
        }
    }
    // the passes' sum, as the CPU and the GPU add it (in f32), taken back out
    let dc: f32 = (0..out.n).filter_map(|i| out.pass(i)).map(|(h, v)| h.iter().sum::<f32>() * v.iter().sum::<f32>()).sum();
    out.centre = -dc;
    Some(out)
}

/// Eigenvalues and eigenvectors (columns of the second, row-major) of the symmetric `n × n` matrix
/// `a`, by cyclic Jacobi rotations.
fn jacobi(mut a: Vec<f64>, n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut v = vec![0.0f64; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    let norm2: f64 = a.iter().map(|x| x * x).sum();
    for _ in 0..64 {
        let off: f64 = (0..n).flat_map(|p| (p + 1..n).map(move |q| (p, q))).map(|(p, q)| a[p * n + q] * a[p * n + q]).sum();
        if off <= 1e-26 * norm2 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq.abs() <= 1e-300 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (kp, kq) = (a[k * n + p], a[k * n + q]);
                    a[k * n + p] = c * kp - s * kq;
                    a[k * n + q] = s * kp + c * kq;
                }
                for k in 0..n {
                    let (pk, qk) = (a[p * n + k], a[q * n + k]);
                    a[p * n + k] = c * pk - s * qk;
                    a[q * n + k] = s * pk + c * qk;
                }
                for k in 0..n {
                    let (kp, kq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * kp - s * kq;
                    v[k * n + q] = s * kp + c * kq;
                }
            }
        }
    }
    ((0..n).map(|i| a[i * n + i]).collect(), v)
}

/// A quarter stop up and down (`2^±0.25`, written out so that the GPU multiplies by the same numbers).
pub const TONE_UP: f32 = 1.189_207_1;
pub const TONE_DOWN: f32 = 0.840_896_4;

/// The tone curve's slope (display stops per scene stop) at scene luminance `y`, over a quarter stop
/// each way; `apply` is the tone map. The GPU's `finish` mirrors it.
#[inline]
pub fn tone_slope(y: f32, apply: impl Fn(f32) -> f32) -> f32 {
    if y.is_nan() || y <= 0.0 {
        return 0.0;
    }
    let (up, down) = (apply(y * TONE_UP), apply(y * TONE_DOWN));
    if up > 0.0 && down > 0.0 { ((up.log2() - down.log2()) * 2.0).clamp(0.0, 4.0) } else { 0.0 }
}

/// Masking's weight (0..1) for edge gradient `e` (of scene `q`, `qs` the pixel's) at display `qd` and
/// tone slope `st`, threshold `t` ([`mask_t`]; 0: no Masking).
#[inline]
pub fn mask_weight(e: f32, qs: f32, qd: f32, st: f32, t: f32) -> f32 {
    if t.is_nan() || t <= 0.0 {
        return 1.0;
    }
    if qs.is_nan() || qs <= 0.0 {
        return 0.0;
    }
    let ed = e / qs * qd * st;
    let (e2, t2) = (ed * ed, t * t);
    if e2 + t2 > 0.0 { e2 / (e2 + t2) } else { 0.0 }
}

/// The change sharpening makes to a pixel's display `qd` (√ of its display luminance): band value `b` (of
/// scene `q`, before exposure), the pixel's log luminance `l` before exposure (`q` = 2^(l/2)), the
/// tone curve's slope `st` there, the band's scale `k` for local Sharpness (1 without), the cap
/// `limit` ([`limit`]) and Masking's weight `wm`; deep shadows are left alone. The GPU's
/// `sharp_delta` mirrors it.
#[inline]
pub fn delta(b: f32, l: f32, qd: f32, st: f32, k: f32, limit: f32, wm: f32) -> f32 {
    let ws = ((qd - Q0) / QW).clamp(0.0, 1.0);
    let qs = (0.5 * l).exp2();
    if ws.is_nan() || ws <= 0.0 || qs.is_nan() || qs <= 0.0 || qs.is_infinite() || limit.is_nan() || limit <= 0.0 {
        return 0.0;
    }
    let dq = qd * st * k * b / qs;
    if !dq.is_finite() {
        return 0.0;
    }
    // (tanh is 1 in f32 from 10 on; some GPUs return NaN for large arguments)
    let dq = limit * (dq / limit).clamp(-10.0, 10.0).tanh();
    ws * wm * dq
}

/// The factor sharpening multiplies a pixel's display luminance `o` by ([`delta`] of its `√o`).
/// The GPU's `finish` mirrors it.
#[inline]
pub fn gain(b: f32, l: f32, o: f32, st: f32, k: f32, limit: f32, wm: f32) -> f32 {
    let qd = o.max(0.0).sqrt();
    let d = delta(b, l, qd, st, k, limit, wm);
    if d == 0.0 {
        return 1.0;
    }
    let q2 = (qd + d).max(0.0);
    let g = q2 / qd;
    if g.is_finite() { g * g } else { 1.0 }
}

/// [`gain`] of a pixel with band value `b`, edge gradient `e` (Masking on), log luminance
/// `l` before exposure, scene luminance `y` at the tone map and display luminance `o` from it, band
/// scale `k`, cap `limit`, Masking threshold `t`, tone map `apply`. The render calls it after the
/// tone map; the GPU's `finish` mirrors it.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn pixel(b: f32, e: Option<f32>, l: f32, y: f32, o: f32, k: f32, limit: f32, t: f32, apply: impl Fn(f32) -> f32) -> f32 {
    let st = tone_slope(y, apply);
    let wm = match e {
        Some(e) => mask_weight(e, (0.5 * l).exp2(), o.max(0.0).sqrt(), st, t),
        None => 1.0,
    };
    gain(b, l, o, st, k, limit, wm)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The designed passes reproduce the target response within a few per cent of its lift, for the
    /// exported settings and the extremes, at the photo's size and smaller; flat areas don't change.
    #[test]
    fn design_follows_the_target_response() {
        for (a, r, d) in [
            (40.0, 1.0, 25.0),
            (100.0, 1.0, 25.0),
            (150.0, 1.0, 25.0),
            (40.0, 0.5, 25.0),
            (40.0, 3.0, 25.0),
            (40.0, 1.0, 0.0),
            (40.0, 1.0, 100.0),
            (150.0, 0.5, 100.0),
            (150.0, 3.0, 0.0),
            (5.0, 1.0, 25.0),
        ] {
            for scale in [1.0f32, 0.5, 0.25] {
                let f = design(a, r, d, scale).expect("a filter");
                let [c, s1, s2] = shape(a, r, d);
                let (s1, s2) = (f64::from(s1 * scale), f64::from(s2 * scale));
                let top = target(1.0, s1, s2, 0.25).ln_1p();
                let c = f64::from(c).min(f64::from(TMAX).ln() / top.max(1e-12));
                let mut lift = 0.0f64;
                let mut worst = 0.0f64;
                for i in 0..=16 {
                    for j in 0..=16 {
                        let (fx, fy) = (i as f64 / 32.0, j as f64 / 32.0);
                        if fx * fx + fy * fy > 0.25 {
                            continue;
                        }
                        let want = target(c, s1, s2, fx * fx + fy * fy);
                        lift = lift.max(want.abs());
                        worst = worst.max((f.response(fx, fy) - want).abs());
                    }
                }
                // (the extremes may need more than the largest reach)
                let tol = if f.r == RMAX { 0.05 } else { 0.035 };
                assert!(worst <= tol * lift, "A{a} R{r} D{d} ×{scale}: off by {worst} of {lift} (r {})", f.r);
                assert!(f.response(0.0, 0.0).abs() < 1e-4, "flat areas change: {}", f.response(0.0, 0.0));
                assert!(f.n >= 1 && f.n <= PASSES && f.r <= RMAX);
            }
        }
    }

    /// The default lifts the finest detail about 2.8×, Amount 100 about 11×, Detail 100 about 5.5×;
    /// Radius 3 lifts it less than Radius 1.
    #[test]
    fn lifts_of_the_finest_detail() {
        let nyq = |a: f32, r: f32, d: f32| 1.0 + design(a, r, d, 1.0).expect("a filter").response(0.5, 0.0);
        let (def, a100, d100, r3) = (nyq(40.0, 1.0, 25.0), nyq(100.0, 1.0, 25.0), nyq(40.0, 1.0, 100.0), nyq(40.0, 3.0, 25.0));
        assert!((2.4..3.2).contains(&def), "default {def}");
        assert!((9.0..13.0).contains(&a100), "Amount 100 {a100}");
        assert!((4.5..6.5).contains(&d100), "Detail 100 {d100}");
        assert!(r3 < def, "Radius 3 {r3} vs 1 {def}");
        assert!(design(150.0, 0.5, 100.0, 1.0).expect("a filter").response(0.5, 0.0) <= f64::from(TMAX));
    }

    #[test]
    fn jacobi_rebuilds_its_matrix() {
        let n = 5;
        let m: Vec<f64> =
            (0..n * n).map(|i| ((i / n) as f64 - 2.0).abs() + ((i % n) as f64 - 2.0).abs() - 1.5 * ((i / n) == (i % n)) as u8 as f64).collect();
        let m: Vec<f64> = (0..n * n).map(|i| 0.5 * (m[i] + m[(i % n) * n + i / n])).collect();
        let (vals, vecs) = jacobi(m.clone(), n);
        for i in 0..n {
            for j in 0..n {
                let got: f64 = (0..n).map(|e| vals[e] * vecs[i * n + e] * vecs[j * n + e]).sum();
                assert!((got - m[i * n + j]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn gain_leaves_shadows_and_flat_areas_alone() {
        // a pixel 10 stops below white: no change; nothing in the band: no change
        assert_eq!(gain(0.05, 0.0, 2f32.powi(-10), 1.0, 1.0, 0.05, 1.0), 1.0);
        assert_eq!(gain(0.0, 0.0, 0.2, 1.0, 1.0, 0.05, 1.0), 1.0);
        // a large band is capped: the display `q` moves by at most the limit
        let g = gain(10.0, 0.0, 0.25, 1.0, 1.0, 0.05, 1.0);
        assert!((g.sqrt() * 0.5 - 0.5 - 0.05).abs() < 1e-3, "{g}");
        // Masking at 0 and garbage stay harmless
        assert_eq!(mask_weight(0.0, 1.0, 0.5, 1.0, 0.0), 1.0);
        assert_eq!(mask_weight(0.0, 1.0, 0.5, 1.0, 0.003), 0.0);
        assert_eq!(gain(f32::NAN, 0.0, 0.25, 1.0, 1.0, 0.05, 1.0), 1.0);
    }
}
