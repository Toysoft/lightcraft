//! Scene geometry stored with a photo's Relight settings ([`RelightGeometry`]), so rendering never
//! needs the model that found it.

use serde::{Deserialize, Serialize};

use crate::segmask::{base64_decode, base64_encode};

/// Largest grid side a stored geometry may claim (hostile input caps the allocation).
pub const MAX_SIDE: usize = 1024;
/// Long edge of the grid a new geometry is stored at: lighting is smooth, and the render sharpens
/// it against the photo's own edges.
pub const STORE_EDGE: usize = 256;

/// One sample of a [`RelightGeometry`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReliefSample {
    /// The surface normal's components along the image's x (right) and y (down) axes, −1..1. The
    /// third, towards the viewer, is what is left of a unit vector (never negative: every visible
    /// surface faces the camera).
    pub nx: f32,
    pub ny: f32,
    /// How near the surface is: 0 (the farthest thing in the photo, or no surface: sky) .. 1 (the nearest).
    pub height: f32,
}

/// The geometry of a photo as a model saw it (MoGe-2 in `lightcraft-relief`): a `width × height`
/// grid over the uncropped, oriented image (row `y`, column `x` covers normalized
/// `((x + 0.5) / width, (y + 0.5) / height)`), three bytes per sample (normal x and y as signed
/// bytes, height as an unsigned one), stored plane by plane with each row as differences from the
/// sample to its left, deflated and base64 encoded.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelightGeometry {
    pub width: u32,
    pub height: u32,
    pub data: String,
}

impl RelightGeometry {
    /// Store `samples` (row-major, `width × height`). `None` for a grid that is empty, too large
    /// or not the size it says.
    pub fn encode(width: usize, height: usize, samples: &[ReliefSample]) -> Option<RelightGeometry> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || samples.len() != width * height {
            return None;
        }
        let signed = |v: f32| ((if v.is_finite() { v } else { 0.0 }).clamp(-1.0, 1.0) * 127.0).round() as i8 as u8;
        let unsigned = |v: f32| ((if v.is_finite() { v } else { 0.0 }).clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut bytes = Vec::with_capacity(samples.len() * 3);
        let planes: [&dyn Fn(&ReliefSample) -> u8; 3] = [&|s| signed(s.nx), &|s| signed(s.ny), &|s| unsigned(s.height)];
        for plane in planes {
            for row in samples.chunks(width) {
                let mut left = 0u8;
                for sample in row {
                    let value = plane(sample);
                    bytes.push(value.wrapping_sub(left));
                    left = value;
                }
            }
        }
        let packed = miniz_oxide::deflate::compress_to_vec(&bytes, 8);
        Some(RelightGeometry { width: width as u32, height: height as u32, data: base64_encode(&packed) })
    }

    /// The grid's size, when it is one a geometry can have.
    pub fn size(&self) -> Option<(usize, usize)> {
        let (w, h) = (self.width as usize, self.height as usize);
        (w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE).then_some((w, h))
    }

    /// The samples, or `None` when the data is damaged or claims an absurd size.
    pub fn decode(&self) -> Option<Vec<ReliefSample>> {
        let (w, h) = self.size()?;
        let n = w * h;
        let packed = base64_decode(&self.data)?;
        let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(&packed, n * 3).ok()?;
        if bytes.len() != n * 3 {
            return None;
        }
        // undo the row differences, plane by plane
        let mut planes = bytes;
        for row in planes.chunks_mut(w) {
            let mut left = 0u8;
            for value in row {
                left = left.wrapping_add(*value);
                *value = left;
            }
        }
        let (nx, rest) = planes.split_at_checked(n)?;
        let (ny, height) = rest.split_at_checked(n)?;
        Some(
            nx.iter()
                .zip(ny)
                .zip(height)
                .map(|((x, y), z)| ReliefSample { nx: f32::from(*x as i8) / 127.0, ny: f32::from(*y as i8) / 127.0, height: f32::from(*z) / 255.0 })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dome(w: usize, h: usize) -> Vec<ReliefSample> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32 * 2.0 - 1.0, (i / w) as f32 / h as f32 * 2.0 - 1.0);
                ReliefSample { nx: x * 0.7, ny: y * 0.7, height: (1.0 - x * x - y * y).max(0.0) }
            })
            .collect()
    }

    #[test]
    fn a_geometry_round_trips_within_a_quantization_step_and_packs_well() {
        let (w, h) = (96, 64);
        let samples = dome(w, h);
        let stored = RelightGeometry::encode(w, h, &samples).unwrap();
        assert_eq!(stored.size(), Some((w, h)));
        let back = stored.decode().unwrap();
        assert_eq!(back.len(), samples.len());
        for (a, b) in samples.iter().zip(&back) {
            assert!(
                (a.nx - b.nx).abs() <= 0.5 / 127.0 + 1e-6
                    && (a.ny - b.ny).abs() <= 0.5 / 127.0 + 1e-6
                    && (a.height - b.height).abs() <= 0.5 / 255.0 + 1e-6
            );
        }
        // smooth geometry is a fraction of its three bytes per sample
        assert!(stored.data.len() < w * h * 3 / 2, "{} bytes of text for {} samples", stored.data.len(), w * h);
        // it survives the settings JSON
        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(serde_json::from_value::<RelightGeometry>(json).unwrap(), stored);
    }

    #[test]
    fn numbers_that_are_not_numbers_are_stored_as_flat_and_far() {
        let bad = ReliefSample { nx: f32::NAN, ny: f32::INFINITY, height: f32::NEG_INFINITY };
        let back = RelightGeometry::encode(2, 1, &[bad, bad]).unwrap().decode().unwrap();
        assert_eq!(back[0], ReliefSample { nx: 0.0, ny: 0.0, height: 0.0 });
    }

    #[test]
    fn damaged_or_absurd_geometry_is_none_not_a_panic() {
        let good = RelightGeometry::encode(8, 8, &dome(8, 8)).unwrap();
        for (width, height) in [(0, 8), (8, 0), (9, 8), (8, 7), (5000, 8), (u32::MAX, u32::MAX)] {
            assert!(RelightGeometry { width, height, data: good.data.clone() }.decode().is_none(), "{width} × {height}");
        }
        for data in ["", "!!!!", "AAAA", &good.data[..good.data.len() / 2]] {
            assert!(RelightGeometry { width: 8, height: 8, data: data.to_string() }.decode().is_none(), "{data:?}");
        }
        assert!(RelightGeometry::encode(0, 4, &[]).is_none());
        assert!(RelightGeometry::encode(4, 4, &dome(4, 3)).is_none());
        assert!(RelightGeometry::encode(MAX_SIDE + 1, 1, &dome(MAX_SIDE + 1, 1)).is_none());
    }
}
