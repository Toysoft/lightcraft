//! Process V3 sharpening (`sharpen`), wired through the render: the lift of each detail size, deep shadows left alone, the cap on the change, Masking,
//! local Sharpness, and the previews.

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, Process};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use crate::{Overlay, RenderRequest, SourceInfo, render};

fn v3(amount: f64) -> DevelopSettings {
    let mut s = DevelopSettings { process: Process::V3.version(), ..DevelopSettings::default() };
    s.detail.sharpen_amount = amount;
    s
}

/// Deterministic noise in -1..1.
fn hash(x: usize, y: usize, k: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ k.wrapping_mul(0x9e37_79b9);
    v ^= v >> 13;
    v = v.wrapping_mul(0x5bd1_e995);
    v ^= v >> 15;
    (v & 0xffff) as f32 / 32768.0 - 1.0
}

/// Display-linear luminance of a render at the source's size.
fn lum(src: &Rgb32f, s: &DevelopSettings) -> Vec<f32> {
    let req = RenderRequest { depth: crate::OutputDepth::F32Linear, ..RenderRequest::fit(src.width, src.height) };
    let deep = render(src, &SourceInfo::default(), s, &req).deep.expect("float render");
    let crate::DeepSamples::F32(v) = &deep.samples else { panic!("f32 samples") };
    v.as_chunks::<3>().0.iter().map(|p| lightcraft_color::luminance_2020(*p).max(0.0)).collect()
}

/// The amplitude of the sinusoid of `f` cycles per pixel (along x) in row-averaged `v` (`w` wide),
/// over the middle half.
fn amplitude(v: &[f32], w: usize, h: usize, f: f64) -> f64 {
    let (mut c, mut s, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for y in h / 4..3 * h / 4 {
        for x in w / 4..3 * w / 4 {
            let q = f64::from(v[y * w + x]).sqrt();
            let a = std::f64::consts::TAU * f * x as f64;
            c += q * a.cos();
            s += q * a.sin();
            n += 1.0;
        }
    }
    2.0 * (c * c + s * s).sqrt() / n
}

/// A grey at scene luminance `y` carrying a faint sinusoid of `f` cycles per pixel along x.
fn wave(w: usize, h: usize, y: f32, f: f64) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, _| {
        let v = y * (1.0 + 0.01 * (std::f64::consts::TAU * f * x as f64).sin() as f32);
        [v; 3]
    })
}

/// The render lifts a detail size by the filter's response there (the designed lift): about
/// 2.7× near the finest detail at the default, ~10× at Amount 100, less at Radius 3; at any tone,
/// the tone curve's slope taken out.
#[test]
fn detail_is_lifted_by_the_filters_response() {
    let (w, h) = (256, 64);
    let lift = |s: &DevelopSettings, y: f32, f: f64| {
        let src = wave(w, h, y, f);
        let mut off = s.clone();
        off.detail.sharpen_amount = 0.0;
        amplitude(&lum(&src, s), w, h, f) / amplitude(&lum(&src, &off), w, h, f)
    };
    let want = |s: &DevelopSettings, f: f64| {
        let b = crate::local::sharpen_band(s, 1.0).map(|b| b.filter).expect("a filter");
        1.0 + b.response(f, 0.0)
    };
    for (amount, radius, f) in [(40.0, 1.0, 0.4375), (40.0, 1.0, 0.125), (100.0, 1.0, 0.375), (40.0, 3.0, 0.1875)] {
        let mut s = v3(amount);
        s.detail.sharpen_radius = radius;
        for y in [0.08f32, 0.18, 0.5] {
            let (got, w) = (lift(&s, y, f), want(&s, f));
            assert!((got / w - 1.0).abs() < 0.05, "Amount {amount} Radius {radius} at {f} c/px, grey {y}: lift {got:.2}, the filter's {w:.2}");
        }
    }
    let nyq = |a: f64, r: f64| {
        let mut s = v3(a);
        s.detail.sharpen_radius = r;
        want(&s, 0.4375)
    };
    assert!((2.3..3.2).contains(&nyq(40.0, 1.0)) && (8.0..13.0).contains(&nyq(100.0, 1.0)) && nyq(40.0, 3.0) < nyq(40.0, 1.0));
}

/// Texture nine stops below white is left as it is; the same texture in the midtones is sharpened.
#[test]
fn deep_shadows_are_left_alone() {
    let (w, h) = (192, 96);
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let base = if x < w / 2 { 0.18 * 2f32.powi(-8) } else { 0.18 };
        [base * (1.0 + 0.05 * hash(x, y, 1)); 3]
    });
    let (off, on) = (lum(&src, &v3(0.0)), lum(&src, &v3(100.0)));
    let half = |v: &[f32], left: bool| -> (f64, f64) {
        let xs = if left { 8..w / 2 - 8 } else { w / 2 + 8..w - 8 };
        let mut d = 0.0f64;
        let mut m = 0.0f64;
        let mut n = 0.0f64;
        for y in 8..h - 8 {
            for x in xs.clone() {
                let (a, b) = (f64::from(off[y * w + x]), f64::from(v[y * w + x]));
                d += (b - a).abs() / a.max(1e-9);
                m += a;
                n += 1.0;
            }
        }
        (d / n, m / n)
    };
    let ((dark, dark_y), (mid, _)) = (half(&on, true), half(&on, false));
    assert!(dark_y < 0.0021, "the dark half is below where sharpening starts ({dark_y})");
    assert!(dark < 1e-4, "deep shadows unchanged: {dark}");
    assert!(mid > 0.02, "midtones sharpened: {mid}");
}

/// A strong edge at Amount 150 moves the display `q` (√ luminance) by at most the cap.
#[test]
fn the_change_is_capped() {
    let (w, h) = (128, 32);
    let src = Rgb32f::from_fn(w, h, |x, _| [if x < w / 2 { 0.03 } else { 0.6 }; 3]);
    for (detail, radius) in [(25.0, 1.0), (100.0, 3.0), (0.0, 0.5)] {
        let mut s = v3(150.0);
        s.detail.sharpen_detail = detail;
        s.detail.sharpen_radius = radius;
        let mut off = s.clone();
        off.detail.sharpen_amount = 0.0;
        let (a, b) = (lum(&src, &off), lum(&src, &s));
        let worst = a.iter().zip(&b).map(|(a, b)| (b.sqrt() - a.sqrt()).abs()).fold(0.0f32, f32::max);
        let cap = crate::sharpen::limit(150.0, crate::sharpen::limit40(radius as f32, detail as f32));
        assert!(worst > 0.5 * cap && worst <= cap * 1.01, "Detail {detail} Radius {radius}: {worst} vs cap {cap}");
    }
}

/// Masking 50 leaves faint flat texture alone and keeps a strong edge; its preview shows the same.
#[test]
fn masking_spares_faint_texture_and_keeps_edges() {
    let (w, h) = (192, 96);
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let v = if x < w / 2 {
            0.2 * (1.0 + 0.01 * hash(x, y, 7))
        } else if (x / 8) % 2 == 0 {
            0.05
        } else {
            0.5
        };
        [v; 3]
    });
    let change = |m: f64, left: bool| {
        let mut s = v3(60.0);
        s.detail.sharpen_masking = m;
        let (a, b) = (lum(&src, &v3(0.0)), lum(&src, &s));
        let xs = if left { 8..w / 2 - 8 } else { w / 2 + 8..w - 8 };
        let mut d = 0.0f64;
        for y in 8..h - 8 {
            for x in xs.clone() {
                d += f64::from((b[y * w + x] - a[y * w + x]).abs());
            }
        }
        d
    };
    assert!(change(50.0, true) < 0.1 * change(0.0, true), "faint texture protected");
    assert!(change(50.0, false) > 0.6 * change(0.0, false), "edges kept");
    let mut s = v3(60.0);
    s.detail.sharpen_masking = 50.0;
    let view = render(&src, &SourceInfo::default(), &s, &RenderRequest { overlay: Overlay::SharpenMask, ..RenderRequest::fit(w, h) }).image;
    let mean = |xs: std::ops::Range<usize>| {
        let mut t = 0.0;
        let mut n = 0.0;
        for y in 8..h - 8 {
            for x in xs.clone() {
                t += f64::from(view.get(x, y)[0]);
                n += 1.0;
            }
        }
        t / n
    };
    assert!(mean(8..w / 2 - 8) < 30.0, "the preview shows the texture protected ({:.0})", mean(8..w / 2 - 8));
    assert!(mean(w / 2 + 8..w - 8) > 80.0, "and the edges sharpened ({:.0})", mean(w / 2 + 8..w - 8));
    // no Masking: all white
    let all = render(&src, &SourceInfo::default(), &v3(60.0), &RenderRequest { overlay: Overlay::SharpenMask, ..RenderRequest::fit(w, h) }).image;
    assert!(all.data.iter().all(|p| p[0] == 255));
}

/// Local Sharpness sharpens where Amount is 0 (with a filter designed at the default), and adds to
/// Amount where it isn't; Amount 0 alone changes nothing.
#[test]
fn local_sharpness_works_with_and_without_amount() {
    let (w, h) = (128, 64);
    let src = Rgb32f::from_fn(w, h, |x, y| [0.18 * (1.0 + 0.1 * hash(x, y, 3)); 3]);
    let with_mask = |amount: f64, k: f64| {
        let mut s = v3(amount);
        s.masks = vec![Mask {
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.9) },
            }],
            adjust: LocalAdjustments { sharpness: k, ..Default::default() },
            ..Default::default()
        }];
        lum(&src, &s)
    };
    let base = lum(&src, &v3(0.0));
    let diff = |v: &[f32]| v.iter().zip(&base).map(|(a, b)| f64::from((a - b).abs())).sum::<f64>();
    assert!(diff(&with_mask(0.0, 0.0)) == 0.0, "Amount 0 changes nothing");
    let local = diff(&with_mask(0.0, 0.8));
    assert!(local > 0.0, "local Sharpness alone sharpens");
    assert!(diff(&with_mask(60.0, 0.8)) > diff(&with_mask(60.0, 0.0)), "and adds to Amount");
}

/// The Radius / Detail preview shows the band as grey around mid-grey: flat areas exactly mid-grey.
#[test]
fn the_detail_preview_is_grey_on_flat_areas() {
    let (w, h) = (96, 64);
    let src = Rgb32f::from_fn(w, h, |x, _| {
        [if x < w / 2 {
            0.18
        } else if x % 2 == 0 {
            0.1
        } else {
            0.4
        }; 3]
    });
    let img = render(&src, &SourceInfo::default(), &v3(40.0), &RenderRequest { overlay: Overlay::SharpenDetail, ..RenderRequest::fit(w, h) }).image;
    let flat = img.get(w / 4, h / 2)[0];
    assert!((126..=129).contains(&flat), "flat: {flat}");
    let busy = (w / 2 + 8..w - 8).map(|x| i32::from(img.get(x, h / 2)[0]) - 128).map(i32::abs).max().unwrap_or(0);
    assert!(busy > 40, "texture shows: {busy}");
}

/// The Radius / Detail preview shows what the render changes: noise in deep shadows (which
/// sharpening leaves alone) stays mid-grey even at Amount 150, the same noise in the midtones shows. It used to
/// show the band relative to the pixel's own √luminance, which grew without bound in the shadows
/// and filled them with black and white static.
#[test]
fn the_detail_preview_shows_no_static_in_the_shadows() {
    let (w, h) = (192, 96);
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let base = if x < w / 2 { 0.18 * 2f32.powi(-8) } else { 0.18 };
        [base * (1.0 + 0.05 * hash(x, y, 5)); 3]
    });
    let img = render(&src, &SourceInfo::default(), &v3(150.0), &RenderRequest { overlay: Overlay::SharpenDetail, ..RenderRequest::fit(w, h) }).image;
    let spread = |xs: std::ops::Range<usize>| {
        let mut t = 0.0;
        let mut n = 0.0;
        for y in 8..h - 8 {
            for x in xs.clone() {
                t += f64::from((i32::from(img.get(x, y)[0]) - 128).abs());
                n += 1.0;
            }
        }
        t / n
    };
    let (dark, mid) = (spread(8..w / 2 - 8), spread(w / 2 + 8..w - 8));
    assert!(dark < 1.0, "deep shadows mid-grey: {dark:.1}");
    assert!(mid > 5.0, "midtone noise shows: {mid:.1}");
}
