//! Dehaze and the rendering process (issue #1237): V3 gets its own dehaze; V1 and V2 keep theirs,
//! and no stage cache ever hands one process another's work.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, Process};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;
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

/// Until the V3 haze model lands, V3 renders Dehaze exactly as V2 (globally, in a mask, in a
/// window and at 16 bits).
#[test]
fn v3_renders_like_v2_until_its_dehaze_lands() {
    let src = scene();
    let info = SourceInfo::default();
    let reqs = [
        RenderRequest::fit(W, H),
        RenderRequest { depth: OutputDepth::U16, ..RenderRequest::fit(W, H) },
        RenderRequest { window: Some(PixelWindow { x: 60, y: 90, w: 200, h: 120 }), ..RenderRequest::fit(2 * W, 2 * H) },
    ];
    for amount in [-100.0, -35.0, 0.0, 25.0, 100.0] {
        for local in [false, true] {
            for req in &reqs {
                let r2 = render(&src, &info, &dehazed(Process::V2, amount, local), req);
                let r3 = render(&src, &info, &dehazed(Process::V3, amount, local), req);
                assert!(r2.image == r3.image && r2.deep == r3.deep, "dehaze {amount}, mask {local}, {:?}", req.window);
            }
        }
    }
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
