//! A hazy valley for measuring Dehaze (issue #1237): ridges that fade with distance into a bright
//! bluish-grey airlight, a sky, a sharp bright/dark edge and a neutral grey patch in the foreground.
//!
//! Every pixel follows the haze model `I = J·t + A·(1 − t)` with a known airlight `A`
//! ([`HAZE_AIRLIGHT`]) and a transmission per depth layer, so a dehaze can be judged against the
//! scene it should recover. Not part of the demo library: a test and measurement scene.

use lightcraft_color::{REC2020, SRGB};
use lightcraft_raster::Rgb32f;
use rayon::prelude::*;

use crate::noise::fbm;

/// The airlight every layer blends towards, linear sRGB (a bright bluish grey).
pub const HAZE_AIRLIGHT: [f32; 3] = [0.62, 0.68, 0.78];

/// Where the measurement features sit, in normalised image coordinates (`0..1` across and down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HazeLayout {
    /// The bright block spans `bright_x0..edge_x`, the dark block `edge_x..dark_x1`, both
    /// `block_y0..block_y1`: a sharp vertical bright/dark edge at `edge_x`.
    pub bright_x0: f32,
    pub edge_x: f32,
    pub dark_x1: f32,
    pub block_y0: f32,
    pub block_y1: f32,
    /// The neutral grey patch (albedo 0.18, under the foreground's haze): `[x0, y0, x1, y1]`.
    pub grey: [f32; 4],
    /// Transmission of the foreground layer the blocks and the patch sit in.
    pub foreground_t: f32,
}

/// The layout of [`hazy_valley`].
pub const HAZE_LAYOUT: HazeLayout =
    HazeLayout { bright_x0: 0.58, edge_x: 0.71, dark_x1: 0.86, block_y0: 0.74, block_y1: 0.93, grey: [0.12, 0.76, 0.26, 0.92], foreground_t: 0.87 };

/// Ridges from far to near: (crest base `v`, crest amplitude, frequency, transmission, albedo).
const RIDGES: [(f32, f32, f32, f32, [f32; 3]); 4] = [
    (0.34, 0.07, 3.0, 0.22, [0.10, 0.12, 0.09]),
    (0.44, 0.07, 4.0, 0.45, [0.08, 0.11, 0.05]),
    (0.55, 0.06, 5.0, 0.68, [0.12, 0.10, 0.05]),
    (0.66, 0.05, 6.0, 0.87, [0.09, 0.13, 0.04]),
];

const SEED: u32 = 1237;

/// Render the hazy valley at `w × h` (meant for 3:2; any size works), linear Rec.2020.
pub fn hazy_valley(w: usize, h: usize) -> Rgb32f {
    let mut img = Rgb32f::new(w, h);
    if w == 0 || h == 0 {
        return img;
    }
    let to2020 = SRGB.to_space(&REC2020).to_f32();
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let c = shade(u, v);
            let m = &to2020;
            *px = [
                (m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2]).max(0.0),
                (m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2]).max(0.0),
                (m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2]).max(0.0),
            ];
        }
    });
    img
}

fn hazed(j: [f32; 3], t: f32) -> [f32; 3] {
    let a = HAZE_AIRLIGHT;
    [j[0] * t + a[0] * (1.0 - t), j[1] * t + a[1] * (1.0 - t), j[2] * t + a[2] * (1.0 - t)]
}

fn shade(u: f32, v: f32) -> [f32; 3] {
    let l = HAZE_LAYOUT;
    // The measurement features sit in the nearest layer.
    let in_blocks = (l.block_y0..l.block_y1).contains(&v);
    if in_blocks && (l.bright_x0..l.edge_x).contains(&u) {
        return hazed([0.85, 0.85, 0.82], l.foreground_t);
    }
    if in_blocks && (l.edge_x..l.dark_x1).contains(&u) {
        return hazed([0.015, 0.015, 0.02], l.foreground_t);
    }
    let [gx0, gy0, gx1, gy1] = l.grey;
    if (gx0..gx1).contains(&u) && (gy0..gy1).contains(&v) {
        return hazed([0.18, 0.18, 0.18], l.foreground_t);
    }
    // Nearest ridge whose crest is above this point.
    for (k, &(base, amp, freq, t, albedo)) in RIDGES.iter().enumerate().rev() {
        let seed = SEED.wrapping_add(k as u32 * 17);
        let crest = base + amp * (fbm(u * freq, 0.5, seed, 4) * 0.5 + 0.5);
        if v >= crest {
            // Contrasty texture: lit faces and dark gullies.
            let tex = fbm(u * 24.0, v * 24.0, seed ^ 0x7e, 4) * 0.5 + 0.5;
            let light = 0.25 + 2.2 * tex * tex;
            return hazed([albedo[0] * light, albedo[1] * light, albedo[2] * light], t);
        }
    }
    // Sky: the airlight at the horizon, bluer and a little darker towards the zenith.
    let elev = (1.0 - v / RIDGES[0].0).clamp(0.0, 1.0);
    let zenith = [0.30, 0.45, 0.80];
    let a = HAZE_AIRLIGHT;
    let s = elev.powf(0.8);
    let wisp = 1.0 + 0.04 * fbm(u * 3.0, v * 9.0, SEED ^ 0x5c, 3);
    [(a[0] + (zenith[0] - a[0]) * s) * wisp, (a[1] + (zenith[1] - a[1]) * s) * wisp, (a[2] + (zenith[2] - a[2]) * s) * wisp]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(img: &Rgb32f, u: f32, v: f32) -> [f32; 3] {
        img.get(((u * img.width as f32) as usize).min(img.width - 1), ((v * img.height as f32) as usize).min(img.height - 1))
    }

    #[test]
    fn the_valley_is_deterministic_finite_and_hazy() {
        let a = hazy_valley(150, 100);
        assert_eq!(a, hazy_valley(150, 100));
        assert!(a.data.iter().all(|p| p.iter().all(|v| v.is_finite() && *v >= 0.0)));
        assert!(hazy_valley(0, 4).data.is_empty());
        // Far ridges are brighter (hazier) than near ones on average.
        let row_mean = |v: f32| (0..150).map(|x| at(&a, x as f32 / 150.0, v)[1]).sum::<f32>() / 150.0;
        assert!(row_mean(0.43) > row_mean(0.70), "{} vs {}", row_mean(0.43), row_mean(0.70));
    }

    #[test]
    fn the_features_are_where_the_layout_says() {
        let img = hazy_valley(600, 400);
        let l = HAZE_LAYOUT;
        let mid = (l.block_y0 + l.block_y1) / 2.0;
        let bright = at(&img, l.edge_x - 0.02, mid);
        let dark = at(&img, l.edge_x + 0.02, mid);
        assert!(bright[1] > 0.6 && dark[1] < 0.15, "{bright:?} {dark:?}");
        let [x0, y0, x1, y1] = l.grey;
        let g = at(&img, (x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!(g[1] > 0.2 && g[1] < 0.3, "{g:?}");
    }
}
