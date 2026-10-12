//! Analyze a photo and write what the model saw as PNGs next to `<out>`: `<out>.normal.png`
//! (x, y, z mapped to red, green, blue), `<out>.depth.png` (near = bright) and
//! `<out>.surface.png`.
//!
//! `cargo run -p lightcraft-relief --example relief_dump -- <model dir> <photo> <out> [tokens]`

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(photo), Some(out)) = (args.next(), args.next(), args.next()) else {
        return Err("usage: relief_dump <model dir> <photo> <out> [tokens]".into());
    };
    let tokens = args.next().and_then(|t| t.parse().ok()).unwrap_or(lightcraft_relief::DEFAULT_TOKENS);
    let img = image::open(&photo)?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rgb: Vec<[f32; 3]> = img.pixels().map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
    let t = std::time::Instant::now();
    let model = lightcraft_relief::Moge::load(&PathBuf::from(dir))?;
    eprintln!("loaded in {:.1?}", t.elapsed());
    let t = std::time::Instant::now();
    let g = model.infer(&rgb, w, h, tokens)?;
    eprintln!("{}×{} analyzed in {:.1?}", g.width, g.height, t.elapsed());
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    let normal: Vec<u8> = g.normal.iter().flat_map(|n| [byte(n[0] * 0.5 + 0.5), byte(n[1] * 0.5 + 0.5), byte(n[2] * 0.5 + 0.5)]).collect();
    let (near, far) = g.depth.iter().fold((f32::MAX, 0f32), |(a, b), d| (a.min(*d), b.max(*d)));
    let depth: Vec<u8> = g.depth.iter().map(|d| byte(1.0 - (d.ln() - near.ln()) / (far.ln() - near.ln()).max(1e-6))).collect();
    let surface: Vec<u8> = g.surface.iter().map(|s| byte(*s)).collect();
    let (gw, gh) = (g.width as u32, g.height as u32);
    image::save_buffer(format!("{out}.normal.png"), &normal, gw, gh, image::ColorType::Rgb8)?;
    image::save_buffer(format!("{out}.depth.png"), &depth, gw, gh, image::ColorType::L8)?;
    image::save_buffer(format!("{out}.surface.png"), &surface, gw, gh, image::ColorType::L8)?;
    let mean = g.normal.iter().fold([0f32; 3], |a, n| [a[0] + n[0], a[1] + n[1], a[2] + n[2]]).map(|v| v / g.normal.len() as f32);
    eprintln!("depth {near:.3}..{far:.3}; mean normal {mean:?}");
    Ok(())
}
