//! Sony's plain raw-IFD distortion table, expressed as a DNG warp for the shared optics pipeline.
//!
//! Tag identity/layout: https://exiftool.org/TagNames/EXIF.html (`0x7037`, signed shorts).
//! Geometry inferred independently from files and Sony Imaging Edge exports; see docs/sony-lens-corrections.md.
//! No lens database or external decoder code. The unrelated MakerNote table has a different scale.

use crate::{Opcode, Rect};
use lightcraft_tiff::{Ifd, Value};

const DISTORTION: u16 = 0x7037;
const SAMPLES: usize = 1024;
const MAX_ERROR: f64 = 0.0005;

/// Only independently validated camera models (ILCE-7RM4A, ILCE-7M3) and the 16-entry layout.
/// Unknown models/layouts and implausible/non-smooth maps are left uncorrected. See docs/sony-lens-corrections.md.
/// A camera setting of Off does not remove the lens data: the user can enable it in the developer.
pub(super) fn distortion(model: &str, ifd: &Ifd, active: Rect, crop: Rect) -> Option<Opcode> {
    // A matching table shape does not establish its scale or radial normalization on another body.
    // Keep this exact: ILCE-7RM4 and ILCE-7RM4A are distinct models, not interchangeable aliases.
    if !matches!(model, "ILCE-7RM4A" | "ILCE-7M3") {
        return None;
    }
    let Value::SShort(values) = &ifd.get(DISTORTION)?.value else { return None };
    let [16, table @ ..] = values.as_slice() else { return None };
    let table: &[i16; 16] = table.try_into().ok()?;
    if table.iter().all(|v| *v == 0) || table.iter().any(|v| !(-4096..=4096).contains(v)) {
        return None;
    }
    let crop = crop.clipped(active.width, active.height);
    if active.width < 2 || active.height < 2 || crop.width < 2 || crop.height < 2 {
        return None;
    }
    // Only native 3:2 framing has been validated. An in-camera aspect crop may retain a table
    // normalized to the full sensor frame; do not silently reinterpret it as the smaller crop.
    if (crop.width as f64 / crop.height as f64 - 1.5).abs() > 0.005 {
        return None;
    }
    // Interpolate fractional radial scale at evenly spaced radii from centre to cropped corner.
    let scale = |r: f64| {
        let at = r.clamp(0.0, 1.0) * 15.0;
        let i = (at.floor() as usize).min(14);
        let a = f64::from(table[i]);
        let b = f64::from(table[i + 1]);
        1.0 + (a + (b - a) * (at - i as f64)) / 16384.0
    };
    let (cw, ch) = (crop.width as f64 - 1.0, crop.height as f64 - 1.0);
    let diagonal = cw.hypot(ch);
    // Zoom until the entire rectangular output boundary is inside the source. For a piecewise-linear
    // scale its maximum on that boundary occurs at its nearest radius or at one of the remaining knots.
    let nearest = cw.min(ch) / diagonal;
    let max_scale = (0..16).map(|i| i as f64 / 15.0).filter(|&r| r >= nearest).map(scale).fold(scale(nearest), f64::max);
    let zoom = 1.0 / max_scale;
    let basis = |r: f64| {
        let r2 = r * r;
        [r, r * r2, r * r2 * r2, r * r2 * r2 * r2]
    };
    let mut matrix = [[0.0; 4]; 4];
    let mut rhs = [0.0; 4];
    let mut previous = -1.0;
    for j in 0..=SAMPLES {
        let r = j as f64 / SAMPLES as f64;
        let source = r * scale(r) * zoom;
        if source <= previous {
            return None;
        }
        previous = source;
        let terms = basis(r);
        for (i, &a) in terms.iter().enumerate() {
            rhs[i] += a * source;
            for (k, &b) in terms.iter().enumerate() {
                matrix[i][k] += a * b;
            }
        }
    }
    let mut k = solve4(matrix, rhs)?;
    // Keep the polynomial within a fraction of a preview pixel of the file's table, and forbid folds.
    for j in 0..=SAMPLES {
        let r = j as f64 / SAMPLES as f64;
        let fit = basis(r).iter().zip(k).map(|(a, b)| a * b).sum::<f64>();
        let r2 = r * r;
        let derivative = k[0] + r2 * (3.0 * k[1] + r2 * (5.0 * k[2] + r2 * 7.0 * k[3]));
        if (fit - r * scale(r) * zoom).abs() > MAX_ERROR || derivative <= 0.0 {
            return None;
        }
    }
    // DNG uses the farthest *active-area* corner, not the cropped diagonal. Preserve the same mapping
    // after the engine translates the opcode into default-cropped and EXIF-oriented coordinates.
    let (ax, ay) = (active.width as f64 - 1.0, active.height as f64 - 1.0);
    let (cx, cy) = (crop.x as f64 + cw / 2.0, crop.y as f64 + ch / 2.0);
    let farthest = cx.max(ax - cx).hypot(cy.max(ay - cy));
    let ratio2 = (2.0 * farthest / diagonal).powi(2);
    for (i, coefficient) in k.iter_mut().enumerate() {
        *coefficient *= ratio2.powi(i as i32);
    }
    Some(Opcode::WarpRectilinear { planes: vec![[k[0], k[1], k[2], k[3], 0.0, 0.0]], center: [cx / ax, cy / ay] })
}

/// Small fixed-size least-squares system; partial pivoting, no input-sized allocations.
fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if !a[pivot][col].is_finite() || a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..4 {
            let factor = a[row][col] / a[col][col];
            for j in col..4 {
                a[row][j] -= factor * a[col][j];
            }
            b[row] -= factor * b[col];
        }
    }
    let mut x = [0.0; 4];
    for i in (0..4).rev() {
        x[i] = (b[i] - (i + 1..4).map(|j| a[i][j] * x[j]).sum::<f64>()) / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_tiff::Entry;

    fn ifd(value: Value) -> Ifd {
        Ifd { entries: vec![Entry { tag: DISTORTION, value, offset: 0 }], ..Default::default() }
    }
    fn table(values: &[i16]) -> Ifd {
        ifd(Value::SShort(std::iter::once(16).chain(values.iter().copied()).collect()))
    }
    const TELE: [i16; 16] = [-2, 1, 7, 17, 31, 48, 68, 93, 120, 152, 187, 225, 265, 309, 356, 403];
    const WIDE: [i16; 16] = [14, 0, -27, -59, -106, -158, -223, -291, -367, -440, -519, -593, -667, -731, -792, -839];
    const AREA: Rect = Rect { x: 0, y: 0, width: 9504, height: 6336 };

    #[test]
    fn matches_independent_sony_reference_geometry() {
        // On/off feature correspondences from Imaging Edge 4.1 at 600 mm, independently fitted.
        let Opcode::WarpRectilinear { planes, center } = distortion("ILCE-7RM4A", &table(&TELE), AREA, AREA).unwrap() else { panic!("warp") };
        assert_eq!(center, [0.5, 0.5]);
        let reference = [0.9760159044, 0.0254891644, -0.0005984764, -0.0010120638];
        for i in 0..=100 {
            let r = i as f64 / 100.0;
            let evaluate = |k: &[f64]| r * (k[0] + r * r * (k[1] + r * r * (k[2] + r * r * k[3])));
            assert!((evaluate(&planes[0]) - evaluate(&reference)).abs() < 0.00015);
        }
    }

    // ILCE-7M3 raw-IFD tables (FE 16-35mm F4 ZA OSS at 16 and 35 mm, FE 28-70mm F3.5-5.6 OSS at 70 mm) and the
    // geometry of the camera's own JPEG of the same exposure (Lens Comp. Distortion: Auto): checkerboard corners of
    // that JPEG matched with the uncorrected raw and fitted with the warp's polynomial over the 6000×4000 frame (rms
    // 0.11–0.25 px). See docs/sony-lens-corrections.md.
    const A7M3: [([i16; 16], [f64; 4]); 3] = [
        (
            [32, 2, -38, -83, -138, -199, -270, -336, -402, -462, -519, -568, -611, -643, -670, -690],
            [1.0247000859, -0.1092847508, 0.1061422243, -0.0397989678],
        ),
        ([2, 2, 4, 9, 18, 29, 45, 65, 92, 124, 164, 210, 264, 329, 403, 489], [0.9708150714, 0.0157256288, 0.0175613359, -0.004107544]),
        ([4, 7, 18, 40, 68, 108, 155, 212, 280, 359, 453, 557, 679, 819, 978, 1161], [0.9342002611, 0.0560486347, 0.0076184603, 0.0021497933]),
    ];

    #[test]
    fn ilce_7m3_matches_the_cameras_own_jpeg_geometry() {
        let frame = Rect::new(0, 0, 6000, 4000);
        let evaluate = |k: &[f64], r: f64| r * (k[0] + r * r * (k[1] + r * r * (k[2] + r * r * k[3])));
        for (values, reference) in A7M3 {
            let Opcode::WarpRectilinear { planes, center } = distortion("ILCE-7M3", &table(&values), frame, frame).unwrap() else { panic!("warp") };
            assert_eq!(center, [0.5, 0.5]);
            // The camera fills its frame with its own zoom; compare the shape over the measured radii (to 0.97).
            let radii: Vec<f64> = (1..=97).map(|i| i as f64 / 100.0).collect();
            let (ours, camera): (Vec<f64>, Vec<f64>) = radii.iter().map(|&r| (evaluate(&planes[0], r), evaluate(&reference, r))).unzip();
            let worst = |ours: &[f64]| {
                let zoom = ours.iter().zip(&camera).map(|(a, b)| a * b).sum::<f64>() / ours.iter().map(|a| a * a).sum::<f64>();
                ours.iter().zip(&camera).map(|(a, b)| (a * zoom - b).abs()).fold(0.0, f64::max)
            };
            // 0.0008 of the half diagonal is 2.9 px on the 6000 px frame (measured: 0.4–2.4 px); uncorrected is 10×+ off.
            let (corrected, uncorrected) = (worst(&ours), worst(&radii));
            assert!(corrected < 0.0008 && uncorrected > 10.0 * corrected, "{values:?}: {corrected} vs {uncorrected}");
        }
        // Sony's crop and the default crop agree on this body (12, 12, 6000×4000 of 6048×4024), so the warp sits on
        // the image centre.
        let Opcode::WarpRectilinear { center, .. } =
            distortion("ILCE-7M3", &table(&A7M3[0].0), Rect::new(0, 0, 6048, 4024), Rect::new(12, 12, 6000, 4000)).unwrap()
        else {
            panic!("warp")
        };
        assert!((center[0] * 6047.0 - 3011.5).abs() < 1e-9 && (center[1] * 4023.0 - 2011.5).abs() < 1e-9, "{center:?}");
    }

    #[test]
    fn barrel_framing_and_offset_crop_keep_the_same_geometry() {
        let crop = Rect::new(32, 20, 9504, 6336);
        let active = Rect::new(0, 0, 9600, 6376);
        let Opcode::WarpRectilinear { planes: a, .. } = distortion("ILCE-7RM4A", &table(&WIDE), AREA, AREA).unwrap() else { panic!("warp") };
        let Opcode::WarpRectilinear { planes: b, center } = distortion("ILCE-7RM4A", &table(&WIDE), active, crop).unwrap() else { panic!("warp") };
        assert!(a[0][0] > 1.02 && a[0][0] < 1.03, "barrel correction expands the middle");
        assert!((center[0] * 9599.0 - 4783.5).abs() < 1e-10);
        assert!((center[1] * 6375.0 - 3187.5).abs() < 1e-10);
        let ratio = (4815.5f64.hypot(3187.5)) / (4751.5f64.hypot(3167.5));
        for i in 0..4 {
            assert!((a[0][i] - b[0][i] / ratio.powi(2 * i as i32)).abs() < 1e-10);
        }
    }

    #[test]
    fn refuses_missing_unknown_malformed_or_unusable_tables() {
        for bad in [
            Ifd::default(),
            table(&[]),
            table(&TELE[..15]),
            table(&[0; 16]),
            table(&[i16::MIN; 16]),
            table(&[0, 4096, -4096, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ifd(Value::SShort(vec![15; 17])),
            ifd(Value::Short(vec![16; 17])),
            ifd(Value::Double(vec![f64::NAN; 17])),
        ] {
            assert!(distortion("ILCE-7RM4A", &bad, AREA, AREA).is_none());
        }
        assert!(distortion("ILCE-7RM4A", &table(&TELE), Rect::new(0, 0, 1, 1), AREA).is_none());
        assert!(distortion("ILCE-7RM4A", &table(&TELE), AREA, Rect::new(0, 0, 9504, 5346)).is_none());
        assert!(distortion("ILCE-7RM4A", &table(&TELE), AREA, Rect::new(99999, 0, 10, 10)).is_none());
    }
    #[test]
    fn refuses_unvalidated_models_even_with_a_valid_table() {
        let valid = table(&TELE);
        assert!(distortion("ILCE-7RM4A", &valid, AREA, AREA).is_some());
        assert!(distortion("ILCE-7M3", &valid, AREA, AREA).is_some());
        for model in ["", "ILCE-7M4", "ILCE-7RM4", "ILCE-7RM5", "ILCE-9M2", "DSC-RX100M3", "ILCE-7RM4A unknown", "ILCE-7M3 ", "ILCE-7SM3"] {
            assert!(distortion(model, &valid, AREA, AREA).is_none(), "{model}");
        }
    }

    #[test]
    fn header_and_full_decode_preserve_correction_through_dng() {
        use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, tags as t};
        for (model, expected) in [("ILCE-7RM4A", 1), ("ILCE-7RM4", 0), ("ILCE-7M3", 1), ("ILCE-7M4", 0), ("", 0)] {
            for (height, expected) in [(20, expected), (16, 0)] {
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let mut raw = IfdBuilder::new();
                    raw.set(t::MAKE, Value::Ascii("SONY".into()));
                    raw.set(t::MODEL, Value::Ascii(model.into()));
                    raw.set(t::IMAGE_WIDTH, Value::Long(vec![32]));
                    raw.set(t::IMAGE_LENGTH, Value::Long(vec![24]));
                    raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
                    raw.set(t::SAMPLES_PER_PIXEL, Value::Short(vec![1]));
                    raw.set(t::PHOTOMETRIC, Value::Short(vec![t::photometric::CFA]));
                    raw.set(t::COMPRESSION, Value::Short(vec![1]));
                    raw.set(t::WHITE_LEVEL, Value::Long(vec![16383]));
                    raw.set(0x74c7, Value::Long(vec![0, 2]));
                    raw.set(t::DEFAULT_CROP_ORIGIN, Value::Long(vec![2, 2]));
                    raw.set(t::DEFAULT_CROP_SIZE, Value::Long(vec![30, 20]));
                    // An aspect crop must stay excluded even when DefaultCrop still describes 3:2.
                    raw.set(0x74c8, Value::Long(vec![30, height]));
                    raw.set(DISTORTION, Value::SShort(std::iter::once(16).chain(TELE).collect()));
                    // Zero pixels have the same byte representation in both orders.
                    raw.set_image(ImageData::Strips { rows_per_strip: 24, strips: vec![vec![0; 32 * 24 * 2]] });
                    let file = TiffWriter { order, ..Default::default() }.write(&[raw]).unwrap();
                    let header = super::super::arw::decode(&file, crate::Mode::Header).unwrap();
                    let full = super::super::arw::decode(&file, crate::Mode::Full).unwrap();
                    assert_eq!(header.info(), full.info());
                    assert_eq!(full.opcodes.list3.len(), expected, "{model} {order:?} height={height}");
                    // the image keeps Sony's crop tags; the warp is centred on the DNG default crop (x 2..32 of 32)
                    assert_eq!(full.crop, Rect::new(0, 2, 30, height as usize));
                    if let Some(Opcode::WarpRectilinear { center, .. }) = full.opcodes.list3.first() {
                        assert!((center[0] - 16.5 / 31.0).abs() < 1e-12 && (center[1] - 11.5 / 23.0).abs() < 1e-12, "{center:?}");
                    }
                    let dng = crate::write_dng(&full, &Default::default()).unwrap();
                    assert_eq!(crate::decode(&dng).unwrap().opcodes.list3, full.opcodes.list3);
                }
            }
        }
    }
}
