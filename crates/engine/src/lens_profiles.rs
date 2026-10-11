//! Native, data-only lens provider. Original Rust implementation of public optical models;
//! the independently licensed Lensfun database is embedded, never the LGPL library.
//! Exact normalized names and explicit aliases only: no fuzzy guess between lens revisions.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use lightcraft_develop::{CatalogLens, EmbeddedLens};
use lightcraft_geom::Point;
use lightcraft_raw::RawInfo;
use serde::Deserialize;

#[derive(Deserialize)]
struct Database {
    revision: String,
    cameras: Vec<Camera>,
    mounts: Vec<Mount>,
    lenses: Vec<Lens>,
}
#[derive(Deserialize)]
struct Camera {
    make: String,
    model: String,
    crop: f64,
    mount: String,
}
#[derive(Deserialize)]
struct Mount {
    name: String,
    compatible: Vec<String>,
}
#[derive(Deserialize)]
struct Lens {
    model: String,
    names: Vec<String>,
    mounts: Vec<String>,
    crop: f64,
    aspect: f64,
    samples: Vec<Sample>,
}
#[derive(Deserialize)]
struct Sample {
    kind: String,
    model: String,
    values: BTreeMap<String, f64>,
}

fn database() -> Option<&'static Database> {
    static DB: OnceLock<Option<Database>> = OnceLock::new();
    DB.get_or_init(|| serde_json::from_str(include_str!("../../../assets/lenses/catalog.json")).ok()).as_ref()
}

fn key(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).flat_map(char::to_lowercase).collect()
}

fn camera_key(make: &str, model: &str) -> String {
    let (make, model) = (key(make), key(model));
    if model.starts_with(&make) { model } else { make + &model }
}

fn lens_key(s: &str) -> String {
    // EXIF model identifiers observed in real files. Keep optical revisions and model numbers;
    // never remove a generic "II", "G2", focal length or aperture when matching.
    match s {
        "35mm F1.4 DG HSM | Art 012" | "Sigma 35mm F1.4 DG HSM | Art 012" => key("Sigma 35mm f/1.4 DG HSM | A"),
        "EF50mm f/1.4 USM" => key("Canon EF 50mm f/1.4 USM"),
        "E 28-75mm F2.8 A063" => key("Tamron 28-75mm F2.8 Di III VXD G2 (A063)"),
        _ => key(s),
    }
}

impl Sample {
    fn value(&self, name: &str, default: f64) -> f64 {
        self.values.get(name).copied().unwrap_or(default)
    }
    fn focal(&self) -> f64 {
        self.value("focal", 0.0)
    }
    fn radial(&self, kind: &str) -> Option<Vec<f64>> {
        if self.kind != kind {
            return None;
        }
        let v = |n| self.value(n, 0.0);
        let result = match (kind, self.model.as_str()) {
            ("distortion", "poly3") => vec![1.0 - v("k1"), 0.0, v("k1"), 0.0, 0.0],
            ("distortion", "poly5") => vec![1.0, 0.0, v("k1"), 0.0, v("k2")],
            ("distortion", "ptlens") => vec![1.0 - v("a") - v("b") - v("c"), v("c"), v("b"), v("a"), 0.0],
            ("tca", "linear") => vec![self.value("kr", 1.0), 0.0, 0.0, self.value("kb", 1.0), 0.0, 0.0],
            ("tca", "poly3") => vec![self.value("vr", 1.0), v("cr"), v("br"), self.value("vb", 1.0), v("cb"), v("bb")],
            ("vignetting", "pa") => vec![v("k1"), v("k2"), v("k3")],
            _ => return None,
        };
        result.iter().all(|v| v.is_finite() && v.abs() < 16.0).then_some(result)
    }
}

// Piecewise linear interpolation in the measured focal domain; no extrapolation from a
// distant zoom endpoint. This is deliberately our interpolation, not Lensfun's spline code.
fn interpolate(points: &[(f64, Vec<f64>)], focal: f64) -> Option<Vec<f64>> {
    let lo = points.iter().filter(|(f, _)| *f <= focal + 0.01).max_by(|a, b| a.0.total_cmp(&b.0))?;
    let hi = points.iter().filter(|(f, _)| *f >= focal - 0.01).min_by(|a, b| a.0.total_cmp(&b.0))?;
    if lo.1.len() != hi.1.len() {
        return None;
    }
    let t = if hi.0 - lo.0 > 0.02 { ((focal - lo.0) / (hi.0 - lo.0)).clamp(0.0, 1.0) } else { 0.0 };
    Some(lo.1.iter().zip(&hi.1).map(|(a, b)| a + (b - a) * t).collect())
}

fn radial(lens: &Lens, kind: &str, focal: f64) -> Option<Vec<f64>> {
    let points: Vec<_> = lens.samples.iter().filter_map(|s| Some((s.focal(), s.radial(kind)?))).collect();
    interpolate(&points, focal)
}

fn vignette(lens: &Lens, focal: f64, aperture: Option<f64>) -> Option<[f64; 3]> {
    let aperture = aperture.filter(|a| a.is_finite() && *a > 0.0)?;
    let mut points: Vec<(f64, Vec<f64>)> = Vec::new();
    let mut focals: Vec<_> = lens.samples.iter().filter(|s| s.kind == "vignetting").map(Sample::focal).collect();
    focals.sort_by(f64::total_cmp);
    focals.dedup();
    for focal in focals {
        let mut aps: Vec<(f64, Vec<f64>)> = Vec::new();
        for s in lens.samples.iter().filter(|s| s.kind == "vignetting" && s.focal() == focal) {
            let a = s.value("aperture", 0.0);
            let k = s.radial("vignetting")?;
            // The RAW metadata currently has no reliable focus distance. Only use a measured
            // aperture if every available distance agrees; guessing infinity can badly overcorrect.
            let distances: Vec<_> =
                lens.samples.iter().filter(|p| p.kind == "vignetting" && p.focal() == focal && p.value("aperture", 0.0) == a).collect();
            if distances.iter().any(|p| p.radial("vignetting").as_ref() != Some(&k))
                || !distances.iter().any(|p| p.value("distance", 0.0) != s.value("distance", 0.0))
            {
                continue;
            }
            aps.push((a, k));
        }
        if let Some(k) = interpolate(&aps, aperture) {
            points.push((focal, k));
        }
    }
    interpolate(&points, focal)?.try_into().ok()
}

fn compatible(db: &Database, camera: &Camera, lens: &Lens) -> bool {
    lens.mounts.iter().any(|m| {
        *m == camera.mount ||
        db.mounts.iter().any(|m2| m2.name == camera.mount && m2.compatible.contains(m)) ||
        // Canon EF-to-RF adapters preserve focal length and image-circle geometry.
        (camera.mount == "Canon RF" && m == "Canon EF")
    })
}

fn bounded_polynomial(k: &[f64], rmax: f64, monotonic: bool) -> bool {
    if !(rmax.is_finite() && rmax > 0.0 && rmax < 8.0) {
        return false;
    }
    (0..64).all(|i| {
        let r = lightcraft_geom::Interval::new(rmax * i as f64 / 64.0, rmax * (i + 1) as f64 / 64.0);
        let evaluate = |derivative: bool| {
            k.iter()
                .enumerate()
                .rev()
                .fold(lightcraft_geom::Interval::from(0.0), |p, (i, k)| p * r + *k * if derivative { (i + 1) as f64 } else { 1.0 })
        };
        let scale = evaluate(false);
        scale.lo() > 0.25 && scale.hi() < 4.0 && (!monotonic || evaluate(true).lo() > 0.05)
    })
}

/// Resolve RAW lens corrections for both import and decode. Embedded corrections win as a
/// complete provider: do not mix an external calibration with a camera's already warped image.
pub fn resolve(raw: &RawInfo) -> Option<EmbeddedLens> {
    if let Some(lens) = crate::files::embedded_lens(raw) {
        return Some(lens);
    }
    // A rendered/linear DNG may already contain baked camera lens correction.
    raw.cfa.as_ref()?;
    let db = database()?;
    let m = &raw.metadata;
    let wanted_camera = camera_key(m.make.as_deref()?, m.model.as_deref()?);
    let cameras: Vec<_> = db.cameras.iter().filter(|c| camera_key(&c.make, &c.model) == wanted_camera).collect();
    let camera = *cameras.first()?;
    if !camera.crop.is_finite() || camera.crop <= 0.0 || cameras.iter().any(|c| c.crop != camera.crop || c.mount != camera.mount) {
        return None;
    }
    let name = lens_key(m.lens_model.as_deref()?);
    let mut lenses: Vec<_> = db
        .lenses
        .iter()
        .filter(|l| {
            l.crop.is_finite()
                && l.crop > 0.0
                && l.crop <= camera.crop + 0.01
                && l.aspect.is_finite()
                && l.aspect >= 1.0
                && l.aspect <= 3.0
                && compatible(db, camera, l)
                && l.names.iter().any(|n| lens_key(n) == name)
        })
        .collect();
    lenses.sort_by(|a, b| b.crop.total_cmp(&a.crop));
    let lens = *lenses.first()?;
    // Multiple equally suitable calibration sets with distinct coefficients are ambiguous.
    if lenses.get(1).is_some_and(|l| (l.crop - lens.crop).abs() < 0.01) {
        return None;
    }
    let focal = m.focal_length.filter(|f| f.is_finite() && *f > 0.0)?;
    let distortion = radial(lens, "distortion", focal).and_then(|k| k.try_into().ok());
    let tca = radial(lens, "tca", focal).and_then(|k| <[f64; 6]>::try_from(k).ok()).map(|[vr, cr, br, vb, cb, bb]| [[vr, cr, br], [vb, cb, bb]]);
    let vignette = vignette(lens, focal, m.f_number);
    if distortion.is_none() && tca.is_none() && vignette.is_none() {
        return None;
    }
    let (aw, ah) = (raw.active_area.width as f64, raw.active_area.height as f64);
    let (cw, ch) = raw.developed_size();
    if aw < 2.0 || ah < 2.0 || cw < 2 || ch < 2 {
        return None;
    }
    let c = raw.crop.clipped(raw.active_area.width, raw.active_area.height);
    let (x, y) = if c.width > 1 && c.height > 1 { (c.x as f64, c.y as f64) } else { (0.0, 0.0) };
    let long = cw.max(ch) as f64;
    let diagonal = aw.hypot(ah) / 2.0 * camera.crop / lens.crop;
    let mut profile_id = 0xcbf2_9ce4_8422_2325u64;
    for b in format!("{}:{}:{}", db.revision, lens.model, lens.crop).bytes() {
        profile_id ^= u64::from(b);
        profile_id = profile_id.wrapping_mul(0x0100_0000_01b3);
    }
    let mut profile = CatalogLens {
        profile_id,
        distortion,
        tca,
        vignette,
        center: Point::new((aw / 2.0 - x) / cw as f64, (ah / 2.0 - y) / ch as f64),
        radius: diagonal / (1.0 + lens.aspect.powi(2)).sqrt() / long,
        vignette_radius: diagonal / long,
    };
    // Verify an injective, bounded radial map over this sensor's actual field. Interpolated
    // coefficients must not fold the image or sample distant unrelated pixels. Interval
    // arithmetic encloses every radius, rather than only checking a few plot points.
    let rmax = [(0.0, 0.0), (cw as f64, 0.0), (0.0, ch as f64), (cw as f64, ch as f64)]
        .iter()
        .map(|&(x, y)| (x - profile.center.x * cw as f64).hypot(y - profile.center.y * ch as f64) / (profile.radius * long))
        .fold(0.0, f64::max);
    if let Some(k) = profile.distortion
        && !bounded_polynomial(&k, rmax, true)
    {
        profile.distortion = None;
    }
    if let Some(tca) = profile.tca
        && tca.iter().any(|k| !bounded_polynomial(k, rmax * 4.0, false))
    {
        profile.tca = None;
    }
    if profile.distortion.is_none() && profile.tca.is_none() && profile.vignette.is_none() {
        return None;
    }
    let lens = EmbeddedLens { catalog: Some(profile), ..EmbeddedLens::default() };
    Some(lightcraft_pipeline::optics::reorient_lens(&lens, raw.orientation, cw as f64, ch as f64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_geom::Orientation;

    fn raw(make: &str, model: &str, lens: &str, focal: f64, aperture: f64) -> RawInfo {
        let bytes = crate::tests_xmp::synthetic_dng_of(300, 200, "RGGB", vec![512; 300 * 200], None, Default::default());
        let mut r = lightcraft_raw::probe_info(&bytes).unwrap();
        r.metadata.make = Some(make.into());
        r.metadata.model = Some(model.into());
        r.metadata.lens_model = Some(lens.into());
        r.metadata.focal_length = Some(focal);
        r.metadata.f_number = Some(aperture);
        r
    }

    #[test]
    fn bundled_database_is_valid_and_has_independent_capabilities() {
        let db = database().expect("bundled database parses");
        assert_eq!(db.revision, "43f9e001ab66c4fcdd4f400463f13b72f94b2288");
        assert!(db.lenses.len() > 1500 && db.cameras.len() > 1000);
        let sigma = resolve(&raw("Canon", "EOS R6m2", "35mm F1.4 DG HSM | Art 012", 35.0, 1.4)).unwrap().catalog.unwrap();
        assert!(sigma.distortion.is_some() && sigma.tca.is_some());
        assert!(sigma.vignette.is_none(), "focus-sensitive measurements must not assume infinity");
        let tamron = resolve(&raw("Sony", "ILME-FX30", "E 28-75mm F2.8 A063", 35.0, 4.0)).unwrap().catalog.unwrap();
        assert!(tamron.distortion.is_some() && tamron.vignette.is_some() && tamron.tca.is_none());
        assert!((tamron.radius - 1.546 / 3.0).abs() < 0.002, "full-frame calibration must account for APS-C radius: {}", tamron.radius);
        assert!(
            resolve(&raw("Canon", "EOS 5D Mark III", "Canon EF 28-105mm f/3.5-4.5 USM", 105.0, 5.6)).is_none(),
            "do not silently choose the II revision"
        );
        assert!(resolve(&raw("Canon", "EOS 5D Mark III", "Canon EF 28-105mm f/3.5-4.5 II USM", 105.0, 5.6)).is_some());
    }

    #[test]
    fn resolver_rejects_unknown_camera_wrong_mount_and_extrapolation() {
        assert!(resolve(&raw("Canon", "Unknown", "Canon EF 50mm f/1.4 USM", 50.0, 4.0)).is_none());
        assert!(resolve(&raw("Sony", "ILME-FX30", "Canon EF 50mm f/1.4 USM", 50.0, 4.0)).is_none());
        assert!(resolve(&raw("Sony", "ILME-FX30", "E 28-75mm F2.8 A063", 100.0, 4.0)).is_none());
        assert!(resolve(&raw("Sony", "ILME-FX30", "E 28-75mm F2.8 A063", f64::NAN, 4.0)).is_none());
    }

    #[test]
    fn crop_and_orientation_preserve_physical_radius_and_center() {
        let mut r = raw("Sony", "ILME-FX30", "E 28-75mm F2.8 A063", 35.0, 4.0);
        let full = resolve(&r).unwrap().catalog.unwrap();
        r.crop = lightcraft_raw::Rect { x: 30, y: 20, width: 240, height: 160 };
        let crop = resolve(&r).unwrap().catalog.unwrap();
        assert!((crop.radius * 240.0 - full.radius * 300.0).abs() < 1e-9);
        assert_eq!(crop.center, Point::new(0.5, 0.5));
        r.orientation = Orientation::Rotate90;
        let rotate = resolve(&r).unwrap().catalog.unwrap();
        assert_eq!(rotate.center, crop.center);
        assert_eq!(rotate.radius, crop.radius);
        assert_eq!(rotate.profile_id, full.profile_id);
    }

    #[test]
    fn folded_and_nonfinite_radial_maps_are_rejected() {
        assert!(!bounded_polynomial(&[1.0, 0.0, -0.5], 2.0, true));
        assert!(!bounded_polynomial(&[f64::NAN], 1.0, true));
        assert!(!bounded_polynomial(&[1.0], f64::INFINITY, true));
        assert!(bounded_polynomial(&[1.02, -0.01, -0.02, 0.01], 2.0, true));
    }

    #[test]
    fn focal_interpolation_does_not_choose_a_zoom_endpoint() {
        let points = vec![(28.0, vec![0.1, 1.0]), (42.0, vec![0.3, 3.0])];
        assert_eq!(interpolate(&points, 35.0), Some(vec![0.2, 2.0]));
        assert_eq!(interpolate(&points, 27.0), None);
        assert_eq!(interpolate(&points, f64::INFINITY), None);
    }

    #[test]
    fn vignetting_without_focus_distance_requires_agreeing_measurements() {
        let sample = |distance, k1| Sample {
            kind: "vignetting".into(),
            model: "pa".into(),
            values: BTreeMap::from([("focal".into(), 35.0), ("aperture".into(), 4.0), ("distance".into(), distance), ("k1".into(), k1)]),
        };
        let mut lens = Lens { model: "test".into(), names: vec![], mounts: vec![], crop: 1.0, aspect: 1.5, samples: vec![sample(1.0, -0.2)] };
        assert_eq!(vignette(&lens, 35.0, Some(4.0)), None, "one measurement cannot establish focus independence");
        lens.samples.push(sample(1000.0, -0.2));
        assert_eq!(vignette(&lens, 35.0, Some(4.0)), Some([-0.2, 0.0, 0.0]));
        lens.samples.push(sample(10.0, -0.3));
        assert_eq!(vignette(&lens, 35.0, Some(4.0)), None, "contradictory distances must not be interpolated as apertures");
        assert_eq!(vignette(&lens, 35.0, None), None);
    }

    #[test]
    fn embedded_geometry_wins_and_legacy_catalogs_deserialize() {
        let mut r = raw("Sony", "ILME-FX30", "E 28-75mm F2.8 A063", 35.0, 4.0);
        r.opcodes.list3.push(lightcraft_raw::Opcode::WarpRectilinear { planes: vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]], center: [0.5, 0.5] });
        let l = resolve(&r).unwrap();
        assert!(l.warp.is_some() && l.catalog.is_none());
        let old: EmbeddedLens = serde_json::from_str(r#"{"warp":null,"vignette":null}"#).unwrap();
        assert!(old.catalog.is_none());
        let resolved = resolve(&raw("Sony", "ILME-FX30", "E 28-75mm F2.8 A063", 35.0, 4.0)).unwrap();
        let round: EmbeddedLens = serde_json::from_str(&serde_json::to_string(&resolved).unwrap()).unwrap();
        assert_eq!(round, resolved, "coefficients survive database changes and catalog moves");
    }
}
