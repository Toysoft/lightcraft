//! Gaussian-like blur by three successive box filters (running sums, O(1) per pixel per pass).

use crate::resample::Pixel;
use crate::{Image, par_rows};

/// Box radii approximating a Gaussian of `sigma` with 3 passes (Kovesi / Wells).
pub fn box_radii(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    std::array::from_fn(|i| (((if (i as i32) < m { wl } else { wu }) - 1) / 2).max(0) as usize)
}

/// One horizontal box pass over `row` (clamped edges), using `tmp` as scratch.
#[inline]
fn box_row<T: Pixel>(row: &mut [T], tmp: &mut Vec<T>, r: usize) {
    let w = row.len();
    if r == 0 || w == 0 {
        return;
    }
    tmp.clear();
    tmp.extend_from_slice(row);
    let s = &tmp[..];
    let last = w - 1;
    let inv = 1.0 / (2 * r + 1) as f32;
    let mut acc = T::zero();
    for i in 0..=2 * r {
        acc = acc.madd(s[i.saturating_sub(r).min(last)], 1.0);
    }
    for x in 0..w {
        row[x] = T::zero().madd(acc, inv);
        acc = acc.madd(s[(x + r + 1).min(last)], 1.0).madd(s[x.saturating_sub(r)], -1.0);
    }
}

/// Rows per parallel band in the vertical pass (each band re-primes its running sums).
pub const BAND: usize = 32;

/// One vertical box pass `src → dst` (clamped edges): a running sum per column, vectorized across
/// the row, parallel over horizontal bands. Cache-friendly — no transposes.
fn box_v<T: Pixel>(src: &Image<T>, dst: &mut Image<T>, r: usize) {
    let (w, h) = (src.width, src.height);
    let inv = 1.0 / (2 * r + 1) as f32;
    let last = h - 1;
    let row = |y: usize| &src.data[y * w..(y + 1) * w];
    par_rows(&mut dst.data, w * BAND, |band, out| {
        let y0 = band * BAND;
        let mut acc = vec![T::zero(); w];
        for i in 0..=2 * r {
            let yy = (y0 + i).saturating_sub(r).min(last);
            for (a, v) in acc.iter_mut().zip(row(yy)) {
                *a = a.madd(*v, 1.0);
            }
        }
        for (k, o) in out.chunks_mut(w).enumerate() {
            let y = y0 + k;
            for (d, a) in o.iter_mut().zip(&acc) {
                *d = T::zero().madd(*a, inv);
            }
            let (add, sub) = (row((y + r + 1).min(last)), row(y.saturating_sub(r)));
            for ((a, p), m) in acc.iter_mut().zip(add).zip(sub) {
                *a = a.madd(*p, 1.0).madd(*m, -1.0);
            }
        }
    });
}

/// Blur with Gaussian `sigma` (pixels). `sigma <= 0.3` returns a copy.
pub fn gaussian<T: Pixel>(img: &Image<T>, sigma: f32) -> Image<T> {
    if sigma <= 0.3 || img.is_empty() {
        return img.clone();
    }
    let radii = box_radii(sigma);
    let mut a = img.clone();
    par_rows(&mut a.data, img.width, |_, row| {
        let mut tmp = Vec::with_capacity(row.len());
        for r in radii {
            box_row(row, &mut tmp, r);
        }
    });
    let mut b = Image::<T>::new(img.width, img.height);
    for r in radii {
        if r == 0 {
            continue;
        }
        box_v(&a, &mut b, r);
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// Longest reach (px) of [`gaussian_taps`]: sigmas above ~10 px belong to [`gaussian`].
pub const EXACT_MAX_RADIUS: usize = 32;

/// Weights of the discrete Gaussian of `sigma` px (Lindeberg's `e^(−t)·Iₙ(t)`, `t = σ²`) over offsets
/// `-r..=r` (`r` = ⌈3σ⌉ + 1, at most [`EXACT_MAX_RADIUS`]), summing to 1. Its variance is σ² at any
/// sigma, where a sampled Gaussian's collapses below half a pixel (σ 0.25 sampled spreads 0.03 px).
/// A sigma near 0 (or not a number) gives the pixel itself.
pub fn gaussian_taps(sigma: f32) -> Vec<f32> {
    let s = if sigma.is_finite() { f64::from(sigma).clamp(0.0, EXACT_MAX_RADIUS as f64 / 3.0) } else { 0.0 };
    let r = ((3.0 * s).ceil() as usize + 1).min(EXACT_MAX_RADIUS);
    let t = s * s;
    // Iₙ(t) by its series Σ (t/2)^(2k+n) / (k! (k+n)!), scaled by e^(−t)
    let bessel = |n: usize| {
        let h = t / 2.0;
        let mut term = (1..=n).fold(1.0f64, |a, k| a * h / k as f64);
        let mut sum = term;
        for k in 1..400 {
            term *= h * h / (k as f64 * (k + n) as f64);
            sum += term;
            if term <= sum * 1e-17 {
                break;
            }
        }
        sum * (-t).exp()
    };
    let w: Vec<f64> = (0..=2 * r).map(|i| bessel(i.abs_diff(r))).collect();
    let sum: f64 = w.iter().sum();
    if !(sum.is_finite() && sum > 0.0) {
        let mut one = vec![0.0; 2 * r + 1];
        one[r] = 1.0;
        return one;
    }
    w.iter().map(|v| (v / sum) as f32).collect()
}

/// Separable Gaussian of a plane with exact taps ([`gaussian_taps`], clamped edges): right at any
/// sigma, including the sub-pixel ones the box approximation ([`gaussian`]) skips or gets wrong
/// (sharpening bands at preview scale). The GPU's `conv_h` / `conv_v` mirror it.
pub fn gaussian_exact(img: &crate::Plane, sigma: f32) -> crate::Plane {
    let taps = gaussian_taps(sigma);
    convolve_separable(img, &taps, &taps)
}

/// A plane through `h` along its rows, then `v` along its columns (odd numbers of taps, centred;
/// clamped edges). An even count is taken as one tap shorter; none gives a copy. The GPU's
/// `conv_h` / `conv_v` mirror it, summing in the same order.
pub fn convolve_separable(img: &crate::Plane, h: &[f32], v: &[f32]) -> crate::Plane {
    let (w, ht) = (img.width, img.height);
    if w == 0 || ht == 0 || h.is_empty() || v.is_empty() {
        return img.clone();
    }
    let h = &h[..h.len() - (1 - h.len() % 2)];
    let v = &v[..v.len() - (1 - v.len() % 2)];
    let (rh, rv) = (h.len() / 2, v.len() / 2);
    let mut a = crate::Plane::new(w, ht);
    par_rows(&mut a.data, w, |y, row| {
        let src = &img.data[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (k, t) in h.iter().enumerate() {
                let sx = (x + k).saturating_sub(rh).min(w - 1);
                acc += t * src[sx];
            }
            *o = acc;
        }
    });
    let mut b = crate::Plane::new(w, ht);
    par_rows(&mut b.data, w, |y, row| {
        for (k, t) in v.iter().enumerate() {
            let sy = (y + k).saturating_sub(rv).min(ht - 1);
            let src = &a.data[sy * w..(sy + 1) * w];
            for (o, v) in row.iter_mut().zip(src) {
                *o += t * v;
            }
        }
    });
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact Gaussian keeps a constant, moves no mass, has the variance it is asked for at
    /// any sigma (where the box approximation is a copy below 0.3 px), and is the pixel itself at 0.
    #[test]
    fn exact_gaussian_is_right_at_small_sigmas() {
        let flat = crate::Plane::filled(9, 7, 0.3);
        assert!(gaussian_exact(&flat, 0.7).data.iter().all(|v| (v - 0.3).abs() < 1e-6));
        for sigma in [0.25f32, 0.5, 1.0, 2.5] {
            let mut imp = crate::Plane::new(41, 41);
            imp.set(20, 20, 1.0);
            let b = gaussian_exact(&imp, sigma);
            let sum: f32 = b.data.iter().sum();
            let var: f32 = b.data.iter().enumerate().map(|(i, v)| v * ((i % 41) as f32 - 20.0).powi(2)).sum();
            assert!((sum - 1.0).abs() < 1e-4, "{sigma}: {sum}");
            assert!((var.sqrt() - sigma).abs() < 0.002 + 0.002 * sigma, "{sigma}: {}", var.sqrt());
        }
        let mut imp = crate::Plane::new(5, 5);
        imp.set(2, 2, 1.0);
        assert!((gaussian_exact(&imp, 0.0).get(2, 2) - 1.0).abs() < 1e-6);
        assert!((gaussian_exact(&imp, f32::NAN).get(2, 2) - 1.0).abs() < 1e-6);
        assert_eq!(gaussian_taps(1e9).len(), 2 * EXACT_MAX_RADIUS + 1);
        assert!(gaussian_taps(1e9).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn preserves_constant_and_mass() {
        let img = Image::<f32>::filled(50, 30, 0.7);
        let b = gaussian(&img, 4.0);
        assert!(b.data.iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut imp = Image::<f32>::new(101, 101);
        imp.set(50, 50, 1.0);
        let b = gaussian(&imp, 5.0);
        let sum: f32 = b.data.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        // peak moved to the centre and symmetric
        assert!((b.get(45, 50) - b.get(55, 50)).abs() < 1e-6);
        assert!((b.get(50, 45) - b.get(50, 55)).abs() < 1e-6);
    }

    #[test]
    fn variance_close_to_sigma_squared() {
        let mut imp = Image::<f32>::new(201, 1);
        imp.set(100, 0, 1.0);
        let sigma = 8.0;
        let b = gaussian(&imp, sigma);
        let var: f32 = b.data.iter().enumerate().map(|(i, v)| v * ((i as f32 - 100.0).powi(2))).sum();
        assert!((var.sqrt() - sigma).abs() / sigma < 0.1, "{}", var.sqrt());
    }
}
