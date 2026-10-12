//! Measure Dehaze (issue #1237): for every process this build knows and Dehaze −100, −50, 0, +25,
//! +50, +100, print the median luminance shift, how many pixels clip to black, the width of the
//! dark halo at a bright/dark edge, the hue of a neutral grey patch and what a flat grey image does.
//!
//! `cargo run --release -p lightcraft-pipeline --example dehaze_report -- [--size 1200] [image…]`
//!
//! Inputs: the procedural hazy valley (`lightcraft_scenes::hazy_valley`, which also has the edge
//! and grey patch), two misty demo scenes, and any sRGB PNG/JPEG paths given (e.g. CC0 corpus
//! photos); edge, grey and flat-grey figures need the valley's known layout, so they are only
//! printed for it.
use lightcraft_color::{REC2020, SRGB};
use lightcraft_develop::{DevelopSettings, Process};
use lightcraft_pipeline::{RenderRequest, SourceInfo, render};
use lightcraft_raster::{Rgb32f, Rgba8};
use lightcraft_scenes::{HAZE_LAYOUT, Kind, demo_library, hazy_valley};

const AMOUNTS: [f64; 6] = [-100.0, -50.0, 0.0, 25.0, 50.0, 100.0];
/// A pixel is clipped to black when every channel is at most this (8-bit, display encoded).
const BLACK: u8 = 2;
/// A halo pixel is darker than the interior of its block by more than this (8-bit luma).
const HALO_STEP: f32 = 10.0;

fn main() {
    let mut size = 1200usize;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--size" {
            size = args.next().and_then(|v| v.parse().ok()).unwrap_or(size).max(64);
        } else {
            paths.push(a);
        }
    }

    let (w, h) = (size, size * 2 / 3);
    let mut inputs: Vec<(String, Rgb32f, bool)> = vec![("hazy valley".into(), hazy_valley(w, h), true)];
    for s in demo_library().into_iter().filter(|s| s.kind == Kind::MistyForest) {
        inputs.push((format!("demo: {}", s.name), s.render_fit(size), false));
    }
    for p in &paths {
        match load_srgb(p, size) {
            Ok(img) => inputs.push((p.clone(), img, false)),
            Err(e) => eprintln!("skipping {p}: {e}"),
        }
    }

    for (name, src, layout) in &inputs {
        println!("\n== {name} ({}×{}) ==", src.width, src.height);
        for process in Process::ALL {
            println!("-- process V{} --", process.version().0);
            println!(
                "{:>7} {:>9} {:>8} {:>9} {:>10} {:>9} {:>9} {:>7} {:>6} {:>7} {:>9}",
                "amount", "med luma", "EV", "black %", "Δblack %", "halo dk", "halo br", "grey L", "C*", "h°", "Δh vs+25"
            );
            let renders: Vec<(f64, Rgba8)> = AMOUNTS.iter().map(|&a| (a, develop(src, process, a))).collect();
            let Some((_, base)) = renders.iter().find(|(a, _)| *a == 0.0) else { continue };
            let base_med = median_luma(base);
            let base_black = black_pct(base);
            let hue25 = renders.iter().find(|(a, _)| *a == 25.0).map(|(_, r)| grey_lch(r)[2]);
            for (amount, r) in &renders {
                let med = median_luma(r);
                let ev = (med.max(1e-4) / base_med.max(1e-4)).log2();
                let black = black_pct(r);
                let mut line = format!("{:>+7.0} {:>9.1} {:>+8.2} {:>9.1} {:>+10.1}", amount, encode(med) * 255.0, ev, black, black - base_black);
                if *layout {
                    let (dk, br) = halo_widths(r);
                    let [l, c, hue] = grey_lch(r);
                    let dh = hue25.map_or(f32::NAN, |h25| hue_diff(hue, h25));
                    line += &format!(" {dk:>9.1} {br:>9.1} {l:>7.1} {c:>6.2} {hue:>7.1} {dh:>+9.1}");
                }
                println!("{line}");
            }
        }
    }

    println!("\n== flat grey (0.18), max |Δ| vs Dehaze 0, 8-bit ==");
    let flat = Rgb32f::filled(300, 200, [0.18; 3]);
    for process in Process::ALL {
        let base = develop(&flat, process, 0.0);
        let diffs: Vec<String> = AMOUNTS
            .iter()
            .map(|&a| {
                let r = develop(&flat, process, a);
                let d = r.data.iter().zip(&base.data).map(|(p, q)| (0..3).map(|i| p[i].abs_diff(q[i])).max().unwrap_or(0)).max().unwrap_or(0);
                format!("{a:+.0}: {d}")
            })
            .collect();
        println!("V{}  {}", process.version().0, diffs.join("  "));
    }
}

fn develop(src: &Rgb32f, process: Process, amount: f64) -> Rgba8 {
    let mut s = DevelopSettings { process: process.version(), ..DevelopSettings::default() };
    s.effects.dehaze = amount;
    render(src, &SourceInfo::default(), &s, &RenderRequest::fit(src.width, src.height)).image
}

fn decode(v: u8) -> f32 {
    lightcraft_color::transfer::srgb_to_linear(f32::from(v) / 255.0)
}

fn encode(v: f32) -> f32 {
    lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0))
}

fn lin(p: [u8; 4]) -> [f32; 3] {
    [decode(p[0]), decode(p[1]), decode(p[2])]
}

/// Relative luminance of a linear sRGB colour.
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Display-encoded luminance, 0..255.
fn luma8(p: [u8; 4]) -> f32 {
    encode(luminance(lin(p))) * 255.0
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// Median linear luminance.
fn median_luma(img: &Rgba8) -> f32 {
    median(img.data.iter().map(|&p| luminance(lin(p))).collect())
}

fn black_pct(img: &Rgba8) -> f32 {
    let n = img.data.iter().filter(|p| p[0] <= BLACK && p[1] <= BLACK && p[2] <= BLACK).count();
    100.0 * n as f32 / img.data.len().max(1) as f32
}

/// Width in pixels (mean over the middle rows of the blocks) of the band next to the bright/dark
/// edge that is darker than the interior of its block: on the dark side and on the bright side.
fn halo_widths(img: &Rgba8) -> (f32, f32) {
    let l = HAZE_LAYOUT;
    let (w, h) = (img.width as f32, img.height as f32);
    let px = |u: f32| ((u * w) as usize).min(img.width - 1);
    let (edge, bright0, dark1) = (px(l.edge_x), px(l.bright_x0), px(l.dark_x1));
    let span = l.block_y1 - l.block_y0;
    let (y0, y1) = (((l.block_y0 + span * 0.25) * h) as usize, ((l.block_y1 - span * 0.25) * h) as usize);
    let (mut dark_sum, mut bright_sum, mut rows) = (0.0f32, 0.0f32, 0.0f32);
    for y in y0..y1.min(img.height) {
        let luma = |x: usize| luma8(img.get(x, y));
        // Interiors: the outer half of each block, away from the edge.
        let dark_ref = median((dark1 - (dark1 - edge) / 2..dark1.saturating_sub(2)).map(luma).collect());
        let bright_ref = median((bright0 + 2..bright0 + (edge - bright0) / 2).map(luma).collect());
        dark_sum += (edge..dark1).take_while(|&x| luma(x) < dark_ref - HALO_STEP).count() as f32;
        bright_sum += (bright0..edge).rev().take_while(|&x| luma(x) < bright_ref - HALO_STEP).count() as f32;
        rows += 1.0;
    }
    (dark_sum / rows.max(1.0), bright_sum / rows.max(1.0))
}

/// CIE L*C*h° (D65) of the mean colour of the inner part of the grey patch.
fn grey_lch(img: &Rgba8) -> [f32; 3] {
    let [x0, y0, x1, y1] = HAZE_LAYOUT.grey;
    let (w, h) = (img.width as f32, img.height as f32);
    let (mx, my) = ((x1 - x0) * 0.2, (y1 - y0) * 0.2);
    let (xa, xb) = (((x0 + mx) * w) as usize, ((x1 - mx) * w) as usize);
    let (ya, yb) = (((y0 + my) * h) as usize, ((y1 - my) * h) as usize);
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for y in ya..yb.min(img.height) {
        for x in xa..xb.min(img.width) {
            let c = lin(img.get(x, y));
            sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
            n += 1.0;
        }
    }
    let c = sum.map(|v| v / n.max(1.0));
    let [l, a, b] = lab(c);
    [l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0)]
}

fn lab(c: [f32; 3]) -> [f32; 3] {
    let x = 0.4124 * c[0] + 0.3576 * c[1] + 0.1805 * c[2];
    let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let z = 0.0193 * c[0] + 0.1192 * c[1] + 0.9505 * c[2];
    let f = |t: f32| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x / 0.950_47), f(y), f(z / 1.088_83));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn hue_diff(a: f32, b: f32) -> f32 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// An sRGB PNG/JPEG as linear Rec.2020, fitted within `size` on the long edge.
fn load_srgb(path: &str, size: usize) -> Result<Rgb32f, image::ImageError> {
    let img = image::open(path)?;
    let img = img.resize(size as u32, size as u32, image::imageops::FilterType::Triangle).to_rgb32f();
    let to2020 = SRGB.to_space(&REC2020);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data = img
        .pixels()
        .map(|p| {
            let c = p.0.map(lightcraft_color::transfer::srgb_to_linear);
            to2020.apply_f32(c).map(|v| v.max(0.0))
        })
        .collect();
    Ok(Rgb32f { width: w, height: h, data })
}
