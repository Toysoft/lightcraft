use super::*;
use lightcraft_develop::ProcessVersion;

fn scene() -> Rgb32f {
    Rgb32f::from_fn(6000, 24, |x, y| {
        let base = if x % 160 < 80 { 0.12 } else { 0.48 };
        let fine = ((x * 17 + y * 31) % 11) as f32 * 0.0005;
        [base + fine; 3]
    })
}

fn settings(process: u32) -> DevelopSettings {
    let mut s = DevelopSettings { process: ProcessVersion(process), ..Default::default() };
    s.detail.sharpen_amount = 80.0;
    s
}

fn picture(src: &Rgb32f, s: &DevelopSettings) -> Rgba8 {
    render(src, &SourceInfo::default(), s, &RenderRequest::fit(src.width, src.height)).image
}

#[test]
fn radius_and_detail_change_fine_sharpening_but_keep_legacy_edits() {
    let src = scene();
    for process in [1, 2] {
        let mut s = settings(process);
        s.detail.sharpen_radius = 0.5;
        let small = picture(&src, &s);
        s.detail.sharpen_radius = 3.0;
        let large = picture(&src, &s);
        s.detail.sharpen_detail = 100.0;
        let detailed = picture(&src, &s);
        if process == 1 {
            assert_eq!(small, large, "legacy Radius behaviour is frozen");
            assert_eq!(large, detailed, "legacy Detail behaviour is frozen");
        } else {
            assert_ne!(small, large, "Radius must affect the rendered pixels");
            assert_ne!(large, detailed, "Detail must affect the rendered pixels");
        }
    }
}

#[test]
fn fine_sharpening_does_not_change_a_flat_field() {
    let src = Rgb32f::filled(6000, 4, [0.2, 0.15, 0.1]);
    let mut s = settings(2);
    s.detail.sharpen_amount = 0.0;
    let plain = picture(&src, &s);
    s.detail.sharpen_amount = 150.0;
    assert_eq!(plain, picture(&src, &s));
}

#[test]
fn new_sharpening_does_not_retune_texture_or_tone() {
    let src = scene();
    let mut s = settings(1);
    s.detail.sharpen_amount = 0.0;
    s.effects.texture = 35.0;
    s.light.contrast = 25.0;
    let old = picture(&src, &s);
    s.process = ProcessVersion(2);
    assert_eq!(old, picture(&src, &s));
}

#[test]
fn changing_radius_invalidates_the_cached_detail_plane() {
    let src = Arc::new(scene());
    let info = SourceInfo::default();
    let req = RenderRequest::fit(src.width, src.height);
    let cache = StageCache::default();
    let mut s = settings(2);
    s.detail.sharpen_radius = 0.5;
    let first = render_cached(&src, &info, &s, &req, &cache).image;
    s.detail.sharpen_radius = 3.0;
    let next = render_cached(&src, &info, &s, &req, &cache).image;
    assert_ne!(first, next);
    assert_eq!(next, render(&src, &info, &s, &req).image);
}

#[test]
fn fine_sharpening_avoids_the_legacy_broad_edge_halo() {
    let src = Rgb32f::from_fn(6000, 8, |x, _| [if x < 3000 { 0.12 } else { 0.48 }; 3]);
    let mut plain_settings = settings(1);
    plain_settings.detail.sharpen_amount = 0.0;
    let plain = picture(&src, &plain_settings);
    let old = picture(&src, &settings(1));
    let new = picture(&src, &settings(2));
    let halo = |img: &Rgba8| -> u32 { (2985..2996).chain(3004..3015).map(|x| u32::from(img.data[x][0].abs_diff(plain.data[x][0]))).sum() };
    assert!(halo(&old) > 0);
    assert!(halo(&new) < halo(&old) / 4, "old halo {}, new halo {}", halo(&old), halo(&new));
}

#[test]
fn default_colour_denoise_removes_speckles_without_blurring_luminance() {
    let src = Rgb32f::from_fn(192, 96, |x, y| {
        let v = if x < 96 { 0.12 } else { 0.4 };
        let noise = (((x * 31 + y * 17) % 11) as f32 - 5.0) * 0.003;
        [v + noise, v - noise * 0.2627 / 0.6780, v]
    });
    let clean = |process| {
        let mut img = src.clone();
        let mut s = settings(process);
        s.detail.nr_color = 25.0;
        local::denoise(&mut img, &s, 192, 192, 0.0);
        img
    };
    let (old, new) = (clean(1), clean(2));
    let chroma = |img: &Rgb32f| img.data.iter().map(|p| f64::from(p[0] - p[1]).powi(2)).sum::<f64>();
    assert!(chroma(&new) < chroma(&old) * 0.3);
    for (before, after) in src.data.iter().zip(&new.data) {
        assert!((lightcraft_color::luminance_2020(*before) - lightcraft_color::luminance_2020(*after)).abs() < 1e-5);
    }
}

#[test]
fn process_change_invalidates_cached_noise_reduction() {
    let src = Arc::new(Rgb32f::from_fn(192, 96, |x, y| {
        let n = (((x * 31 + y * 17) % 11) as f32 - 5.0) * 0.004;
        [0.2 + n, 0.15 - n, 0.1]
    }));
    let req = RenderRequest::fit(192, 96);
    let info = SourceInfo::default();
    let cache = StageCache::default();
    let mut s = settings(1);
    s.detail.nr_color = 25.0;
    render_cached(&src, &info, &s, &req, &cache);
    s.process = ProcessVersion(2);
    let next = render_cached(&src, &info, &s, &req, &cache).image;
    assert_eq!(next, render(&src, &info, &s, &req).image);
}
