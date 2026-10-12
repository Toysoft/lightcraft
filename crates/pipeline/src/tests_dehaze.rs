//! Dehaze and the rendering process (issue #1237): V3 gets its own dehaze; V1 and V2 keep theirs,
//! and no stage cache ever hands one process another's work.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, Process};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_scenes::hazy_valley;

use crate::{OutputDepth, PixelWindow, RenderRequest, SourceInfo, StageCache, plan, render, render_cached};

const W: usize = 360;
const H: usize = 240;

fn scene() -> Rgb32f {
    hazy_valley(W, H)
}

/// Settings on `process` with Dehaze `amount`, and with `local` also a gradient mask adding Dehaze.
fn dehazed(process: Process, amount: f64, local: bool) -> DevelopSettings {
    let mut s = DevelopSettings { process: process.version(), ..DevelopSettings::default() };
    s.effects.dehaze = amount;
    s.light.highlights = -40.0;
    if local {
        s.masks = vec![Mask {
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.9) },
            }],
            adjust: LocalAdjustments { dehaze: 40.0, ..Default::default() },
            ..Default::default()
        }];
    }
    s
}

#[test]
fn stage_cache_keys_include_the_process() {
    let src = scene();
    let info = SourceInfo::default();
    let req = RenderRequest::fit(W, H);
    let keys = |p: Process| {
        let s = dehazed(p, 30.0, false);
        let pl = plan(&src, &info, &s, &req);
        (pl.lin_key, pl.pre_nr_key, pl.geo)
    };
    let [k1, k2, k3] = Process::ALL.map(keys);
    for (a, b) in [(k1, k2), (k2, k3), (k1, k3)] {
        assert_ne!(a.0, b.0, "lin_key");
        assert_ne!(a.1, b.1, "pre_nr_key");
        assert_eq!(a.2, b.2, "the resampled source is the same on every process");
    }
    // a number from a newer LightCraft keys like the process it renders with
    let newer =
        DevelopSettings { process: lightcraft_develop::ProcessVersion(Process::LATEST.version().0 + 5), ..dehazed(Process::LATEST, 30.0, false) };
    assert_eq!(plan(&src, &info, &newer, &req).lin_key, keys(Process::LATEST).0);
}

/// V1 and V2 keep the Dehaze they always had (V1 and V2 differ only in the Highlights / Shadows
/// base, so without those they render alike), and V3 has its own: the same pixels as V2 without
/// Dehaze, others with it (globally, in a mask, in a window and at 16 bits).
#[test]
fn v1_and_v2_keep_their_dehaze_and_v3_has_its_own() {
    let src = scene();
    let info = SourceInfo::default();
    let reqs = [
        RenderRequest::fit(W, H),
        RenderRequest { depth: OutputDepth::U16, ..RenderRequest::fit(W, H) },
        RenderRequest { window: Some(PixelWindow { x: 60, y: 90, w: 200, h: 120 }), ..RenderRequest::fit(2 * W, 2 * H) },
    ];
    let plain = |p: Process, amount: f64, local: bool| DevelopSettings { light: Default::default(), ..dehazed(p, amount, local) };
    for amount in [-100.0, -35.0, 0.0, 25.0, 100.0] {
        for local in [false, true] {
            for req in &reqs {
                let r1 = render(&src, &info, &plain(Process::V1, amount, local), req);
                let r2 = render(&src, &info, &plain(Process::V2, amount, local), req);
                let r3 = render(&src, &info, &plain(Process::V3, amount, local), req);
                assert!(r1.image == r2.image && r1.deep == r2.deep, "V1 vs V2: dehaze {amount}, mask {local}, {:?}", req.window);
                let same = r2.image == r3.image && r2.deep == r3.deep;
                assert_eq!(same, amount == 0.0 && !local, "V2 vs V3: dehaze {amount}, mask {local}, {:?}", req.window);
            }
        }
    }
}

// ---- V3's Dehaze (issue #1237) -----------------------------------------------------------------

/// A pixel is clipped to black when every channel is at most this (8-bit, display encoded), as in
/// `examples/dehaze_report.rs`.
const BLACK: u8 = 2;

fn srgb_lin(v: u8) -> f32 {
    lightcraft_color::transfer::srgb_to_linear(f32::from(v) / 255.0)
}

/// Relative luminance (linear sRGB) of an 8-bit pixel.
fn luminance(p: [u8; 4]) -> f32 {
    0.2126 * srgb_lin(p[0]) + 0.7152 * srgb_lin(p[1]) + 0.0722 * srgb_lin(p[2])
}

fn median_luma(img: &Rgba8) -> f32 {
    let mut v: Vec<f32> = img.data.iter().map(|&p| luminance(p)).collect();
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn black_pct(img: &Rgba8) -> f32 {
    100.0 * img.data.iter().filter(|p| p[0] <= BLACK && p[1] <= BLACK && p[2] <= BLACK).count() as f32 / img.data.len().max(1) as f32
}

fn v3(src: &Rgb32f, amount: f64, req: &RenderRequest) -> Rgba8 {
    let mut s = DevelopSettings { process: Process::V3.version(), ..DevelopSettings::default() };
    s.effects.dehaze = amount;
    render(src, &SourceInfo::default(), &s, req).image
}

/// (max, mean) of the per-channel 8-bit differences.
fn diff(a: &Rgba8, b: &Rgba8) -> (u8, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut max, mut sum) = (0u8, 0u64);
    for (p, q) in a.data.iter().zip(&b.data) {
        for i in 0..3 {
            let d = p[i].abs_diff(q[i]);
            max = max.max(d);
            sum += u64::from(d);
        }
    }
    (max, sum as f64 / (3 * a.data.len().max(1)) as f64)
}

/// The acceptance bands of #1237 on the hazy valley and the misty demo scenes: Dehaze clears haze
/// without making the photo much darker or crushing it to black.
#[test]
fn v3_dehaze_meets_the_acceptance_bands() {
    // amount, median EV shift (low, high), most extra black (% of pixels)
    let bands = [(25.0, -0.2, 0.0, 1.0), (50.0, -0.5, -0.2, 3.0), (100.0, -0.9, -0.4, 8.0)];
    let mut inputs = vec![("hazy valley".to_string(), scene())];
    for s in lightcraft_scenes::demo_library().into_iter().filter(|s| s.kind == lightcraft_scenes::Kind::MistyForest) {
        inputs.push((s.name.clone(), s.render_fit(W)));
    }
    for (name, src) in &inputs {
        let req = RenderRequest::fit(src.width, src.height);
        let base = v3(src, 0.0, &req);
        let (m0, b0) = (median_luma(&base), black_pct(&base));
        let mut last = 0.0f32;
        for (amount, lo, hi, black) in bands {
            let r = v3(src, amount, &req);
            let ev = (median_luma(&r).max(1e-4) / m0.max(1e-4)).log2();
            let extra = black_pct(&r) - b0;
            assert!((lo..=hi).contains(&ev), "{name} +{amount}: median {ev:+.2} EV, want {lo}..{hi}");
            assert!(extra <= black, "{name} +{amount}: {extra:.1} % more black, want ≤ {black}");
            assert!(ev <= last + 1e-3, "{name} +{amount}: darker with the amount ({last:+.2} → {ev:+.2})");
            last = ev;
        }
        // and it does clear haze: more contrast (the 10th–90th percentile spread of the luminance,
        // relative to the median) with the amount. (Only on the valley: in the demo fog a big sun
        // dominates the spread, and Dehaze leaves the sky near the airlight alone.)
        if name != "hazy valley" {
            continue;
        }
        let spread = |i: &Rgba8| {
            let mut v: Vec<f32> = i.data.iter().map(|&p| luminance(p)).collect();
            v.sort_by(f32::total_cmp);
            (v[v.len() * 9 / 10] - v[v.len() / 10]) / v[v.len() / 2].max(1e-4)
        };
        let (s0, s50, s100) = (spread(&base), spread(&v3(src, 50.0, &req)), spread(&v3(src, 100.0, &req)));
        assert!(s50 > s0 * 1.05 && s100 > s50 * 1.05, "{name}: contrast {s0:.3} → +50 {s50:.3} → +100 {s100:.3}");
    }
}

/// Width (px, mean over the middle rows of the valley's blocks) of the band next to the bright /
/// dark edge that is darker than its block's interior by more than 10 (8-bit luma), on the dark
/// and the bright side (as `examples/dehaze_report.rs` measures it).
fn halo_widths(img: &Rgba8) -> (f32, f32) {
    let l = lightcraft_scenes::HAZE_LAYOUT;
    let luma8 = |p: [u8; 4]| lightcraft_color::transfer::linear_to_srgb(luminance(p).clamp(0.0, 1.0)) * 255.0;
    let median = |mut v: Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let (w, h) = (img.width as f32, img.height as f32);
    let px = |u: f32| ((u * w) as usize).min(img.width - 1);
    let (edge, bright0, dark1) = (px(l.edge_x), px(l.bright_x0), px(l.dark_x1));
    let span = l.block_y1 - l.block_y0;
    let (y0, y1) = (((l.block_y0 + span * 0.25) * h) as usize, ((l.block_y1 - span * 0.25) * h) as usize);
    let (mut dark_sum, mut bright_sum, mut rows) = (0.0f32, 0.0f32, 0.0f32);
    for y in y0..y1.min(img.height) {
        let luma = |x: usize| luma8(img.get(x, y));
        let dark_ref = median((dark1 - (dark1 - edge) / 2..dark1 - 2).map(luma).collect());
        let bright_ref = median((bright0 + 2..bright0 + (edge - bright0) / 2).map(luma).collect());
        dark_sum += (edge..dark1).take_while(|&x| luma(x) < dark_ref - 10.0).count() as f32;
        bright_sum += (bright0..edge).rev().take_while(|&x| luma(x) < bright_ref - 10.0).count() as f32;
        rows += 1.0;
    }
    (dark_sum / rows.max(1.0), bright_sum / rows.max(1.0))
}

/// No dark halo next to a bright edge, even at +100 (V1 makes one ~20 px wide at this size).
#[test]
fn v3_dehaze_makes_no_halos() {
    let src = hazy_valley(900, 600);
    let req = RenderRequest::fit(900, 600);
    for amount in [50.0, 100.0] {
        let (dark, bright) = halo_widths(&v3(&src, amount, &req));
        assert!(dark <= 2.0 && bright <= 2.0, "+{amount}: halo {dark:.1} px (dark side), {bright:.1} px (bright side)");
    }
    // (the probe does see V1's)
    let mut s = DevelopSettings { process: Process::V1.version(), ..DevelopSettings::default() };
    s.effects.dehaze = 100.0;
    let (dark, _) = halo_widths(&render(&src, &SourceInfo::default(), &s, &req).image);
    assert!(dark > 2.0, "V1's halo: {dark:.1} px");
}

/// A flat grey image has no haze to remove: every amount leaves it alone (within 1 LSB).
#[test]
fn v3_leaves_a_flat_grey_alone() {
    for v in [0.03f32, 0.18, 0.6] {
        let src = Rgb32f::filled(120, 80, [v; 3]);
        let req = RenderRequest::fit(120, 80);
        let base = v3(&src, 0.0, &req);
        for amount in [-100.0, -50.0, 25.0, 50.0, 100.0] {
            let (max, _) = diff(&base, &v3(&src, amount, &req));
            assert!(max <= 1, "grey {v}, dehaze {amount}: {max} LSB");
        }
    }
}

/// NaN, infinite, negative and black pixels never panic and never turn finite pixels into NaN.
#[test]
fn v3_dehaze_survives_hostile_pixels() {
    let src = Rgb32f::from_fn(96, 64, |x, y| match (x + 3 * y) % 7 {
        0 => [f32::NAN, 0.2, 0.1],
        1 => [f32::INFINITY, 0.5, 0.5],
        2 => [-1.0, 0.0, 0.0],
        3 => [0.0; 3],
        _ => [0.3, 0.32, 0.36],
    });
    let req = RenderRequest { depth: OutputDepth::F32Linear, ..RenderRequest::fit(96, 64) };
    let deep = |amount: f64, local: bool| {
        let s = DevelopSettings { light: Default::default(), ..dehazed(Process::V3, amount, local) };
        let r = render(&src, &SourceInfo::default(), &s, &req);
        let Some(crate::DeepImage { samples: crate::DeepSamples::F32(v), .. }) = r.deep else { panic!("no float render") };
        v
    };
    let base = deep(0.0, false);
    for amount in [-100.0, -40.0, 30.0, 100.0] {
        for local in [false, true] {
            let r = deep(amount, local);
            for (i, (a, b)) in base.iter().zip(&r).enumerate() {
                assert!(!a.is_finite() || b.is_finite(), "dehaze {amount}, mask {local}: sample {i} {a} → {b}");
            }
        }
    }
    // a black frame
    let black = Rgb32f::filled(40, 30, [0.0; 3]);
    for amount in [-100.0, 100.0] {
        assert!(v3(&black, amount, &RenderRequest::fit(40, 30)).data.iter().all(|p| p[..3] == [0, 0, 0] || amount < 0.0));
    }
    // an empty source has no haze map to sample (and doesn't panic building one)
    let s = DevelopSettings { process: Process::V3.version(), ..DevelopSettings::default() };
    let empty = Rgb32f::new(0, 0);
    let frame = crate::geometry::Frame::new(0, 0, &s, true);
    let map = crate::haze::for_frame(&empty, &SourceInfo::default(), &s, &frame);
    assert_eq!(map.size(), (1, 1));
}

/// A zoomed window samples the whole frame's haze map: it matches the same pixels of the whole
/// render (within 1/255 mean, 3/255 max), at every amount, with a mask too.
#[test]
fn v3_window_matches_the_whole_render() {
    let src = scene();
    let info = SourceInfo::default();
    let full = RenderRequest::fit(4 * W, 4 * H);
    let win = PixelWindow { x: 700, y: 500, w: 320, h: 240 };
    for amount in [-100.0, 40.0, 100.0] {
        for local in [false, true] {
            let s = DevelopSettings { light: Default::default(), ..dehazed(Process::V3, amount, local) };
            let whole = render(&src, &info, &s, &full).image.crop(win.x, win.y, win.w, win.h);
            let part = render(&src, &info, &s, &RenderRequest { window: Some(win), ..full }).image;
            let (max, mean) = diff(&whole, &part);
            assert!(max <= 3 && mean <= 1.0, "dehaze {amount}, mask {local}: max {max}, mean {mean:.3}");
        }
    }
}

/// Preview and export dehaze alike: a small render matches the large one scaled down about as
/// well as without Dehaze (the haze map is the same for both).
#[test]
fn v3_preview_matches_the_export() {
    let src = hazy_valley(1440, 960);
    let (small, big) = (RenderRequest::fit(360, 240), RenderRequest::fit(1440, 960));
    let shrink = |i: &Rgba8| {
        Rgba8::from_fn(i.width / 4, i.height / 4, |x, y| {
            let mut acc = [0.0f32; 3];
            for dy in 0..4 {
                for dx in 0..4 {
                    let p = i.get(4 * x + dx, 4 * y + dy);
                    for k in 0..3 {
                        acc[k] += srgb_lin(p[k]);
                    }
                }
            }
            let e = acc.map(|v| (lightcraft_color::transfer::linear_to_srgb(v / 16.0) * 255.0).round() as u8);
            [e[0], e[1], e[2], 255]
        })
    };
    let gap = |amount: f64| diff(&v3(&src, amount, &small), &shrink(&v3(&src, amount, &big))).1;
    let g0 = gap(0.0);
    for amount in [-100.0, 50.0, 100.0] {
        let g = gap(amount);
        assert!(g <= g0 + 1.0, "dehaze {amount}: preview vs export mean {g:.3} LSB (without Dehaze {g0:.3})");
    }
}

/// A mask's Dehaze goes through the same path: a mask over the whole frame dehazes like the
/// slider, mask and slider add up (to ±100 at most), and outside a mask nothing changes.
#[test]
fn v3_mask_dehaze_adds_to_the_slider() {
    let src = scene();
    let info = SourceInfo::default();
    let req = RenderRequest::fit(W, H);
    let with = |global: f64, local: f64, start: f64, end: f64| {
        let mut s = DevelopSettings { process: Process::V3.version(), ..DevelopSettings::default() };
        s.effects.dehaze = global;
        s.masks = vec![Mask {
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Linear { start: Point::new(0.5, start), end: Point::new(0.5, end) },
            }],
            adjust: LocalAdjustments { dehaze: local, ..Default::default() },
            ..Default::default()
        }];
        render(&src, &info, &s, &req).image
    };
    let everywhere = |g, l| with(g, l, 3.0, 4.0);
    let slider = |a| v3(&src, a, &req);
    assert!(diff(&everywhere(0.0, 60.0), &slider(60.0)).0 <= 1, "a whole-frame mask is the slider");
    assert!(diff(&everywhere(-30.0, 80.0), &slider(50.0)).0 <= 1, "mask and slider add");
    assert!(diff(&everywhere(80.0, 60.0), &slider(100.0)).0 <= 1, "clamped to +100");
    assert!(diff(&everywhere(-80.0, -60.0), &slider(-100.0)).0 <= 1, "clamped to −100");
    // a mask over the top half: the bottom rows are the undehazed photo's
    let half = with(0.0, 100.0, 0.0, 0.5);
    let base = slider(0.0);
    let rows = |i: &Rgba8, y0: usize| i.crop(0, y0, W, H - y0);
    assert_eq!(rows(&half, H * 6 / 10), rows(&base, H * 6 / 10), "outside the mask");
    assert!(diff(&rows(&half, 0).crop(0, 0, W, H / 4), &base.crop(0, 0, W, H / 4)).0 > 5, "inside the mask");
}

/// Moving a dehazed photo between processes (Update to Current Process, then undo) with warm
/// caches renders what a fresh render does.
#[test]
fn warm_caches_across_processes_match_fresh_renders() {
    let src = Arc::new(scene());
    let info = SourceInfo { raw: true, ..Default::default() };
    let cache = StageCache::default();
    for req in
        [RenderRequest::fit(W, H), RenderRequest { window: Some(PixelWindow { x: 40, y: 30, w: 180, h: 150 }), ..RenderRequest::fit(2 * W, 2 * H) }]
    {
        for p in [Process::V1, Process::V3, Process::V2, Process::V3, Process::V1, Process::V2] {
            let s = dehazed(p, 45.0, true);
            let warm = render_cached(&src, &info, &s, &req, &cache).image;
            assert!(warm == render(&src, &info, &s, &req).image, "{p:?}, window {:?}", req.window);
        }
    }
}

/// The V3 haze map is built only when V3 dehazes (globally or in a mask), and every render of the
/// photo gets the same one: preview and export sizes, a zoomed window, any exposure.
#[test]
fn v3_plans_carry_one_haze_map_per_photo() {
    let src = scene();
    let info = SourceInfo::default();
    let req = RenderRequest::fit(W, H);
    assert!(plan(&src, &info, &dehazed(Process::V3, 0.0, false), &req).haze.is_none());
    assert!(plan(&src, &info, &dehazed(Process::V2, 50.0, true), &req).haze.is_none());
    assert!(plan(&src, &info, &dehazed(Process::V1, 50.0, false), &req).haze.is_none());
    let local_only = plan(&src, &info, &dehazed(Process::V3, 0.0, true), &req).haze;
    let Some(map) = plan(&src, &info, &dehazed(Process::V3, 50.0, false), &req).haze else { panic!("no haze map for V3 Dehaze") };
    assert!(local_only.is_some_and(|m| *m == *map), "a mask's dehaze uses the same map");
    let reqs = [
        RenderRequest::fit(W / 3, H / 3),
        RenderRequest::fit(4 * W, 4 * H),
        RenderRequest { window: Some(PixelWindow { x: 300, y: 200, w: 150, h: 90 }), ..RenderRequest::fit(3 * W, 3 * H) },
        RenderRequest { depth: OutputDepth::U16, ..req },
    ];
    for (i, r) in reqs.iter().enumerate() {
        for amount in [-60.0, 50.0, 100.0] {
            let mut s = dehazed(Process::V3, amount, false);
            s.light.exposure = 1.5;
            let Some(m) = plan(&src, &info, &s, r).haze else { panic!("request {i}: no haze map") };
            assert!(*m == *map, "request {i}, dehaze {amount}: another haze map");
        }
    }
    // (the scene is smaller than the proxy edge: the proxy is the frame at its own size)
    assert_eq!(map.size(), (W, H));
}

/// A large photo's map is built on the fixed-size proxy, once: the next plan reuses it.
#[test]
fn v3_haze_map_is_built_once() {
    let src = hazy_valley(3000, 2000);
    let info = SourceInfo::default();
    let s = dehazed(Process::V3, 50.0, false);
    let a = plan(&src, &info, &s, &RenderRequest::fit(1500, 1000)).haze;
    let b = plan(&src, &info, &s, &RenderRequest::fit(600, 400)).haze;
    let (Some(a), Some(b)) = (a, b) else { panic!("no haze map") };
    assert!(Arc::ptr_eq(&a, &b), "the second plan reuses the cached map");
    assert_eq!(a.size(), (1024, 683));
}
