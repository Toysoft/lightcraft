//! The offline basemap: the world's land, lakes and country borders (Natural Earth 1:50m, public domain) drawn
//! for a [`View`]. It needs no network, so it is what the map shows unless online tiles are switched on, and
//! what stays visible under tiles that have not arrived.
//!
//! The data is `assets/map/natural-earth-50m.bin` (format: `tools/natural_earth_basemap.py`). Land and lakes
//! are filled with an even-odd scanline fill, so a lake cut out of the land as a hole needs no special case.
//!
//! Sources: scanline polygon fill (textbook); own file format.

use std::sync::OnceLock;

use crate::{View, project};

static DATA: &[u8] = include_bytes!("../../../assets/map/natural-earth-50m.bin");
const MAGIC: &[u8; 8] = b"LCMAP1\0\0";
/// Most paths per layer and points per path accepted (the real file has ~1 400 and ~12 000).
const MAX_PATHS: usize = 100_000;
const MAX_POINTS: usize = 1_000_000;
/// The largest picture rendered, per edge (pixels).
pub const MAX_PIXELS: usize = 8192;

pub const ATTRIBUTION: &str = "Made with Natural Earth";

/// A path in world coordinates (see [`project`]) with its bounding box.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub points: Vec<(f64, f64)>,
    min: (f64, f64),
    max: (f64, f64),
}

impl Path {
    pub fn new(points: Vec<(f64, f64)>) -> Path {
        let (mut min, mut max) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
        for (x, y) in &points {
            (min, max) = ((min.0.min(*x), min.1.min(*y)), (max.0.max(*x), max.1.max(*y)));
        }
        Path { points, min, max }
    }
}

/// The map's outlines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Basemap {
    /// Closed rings; inner rings (seas inside land) are holes by the even-odd rule.
    pub land: Vec<Path>,
    pub lakes: Vec<Path>,
    /// Open lines.
    pub borders: Vec<Path>,
}

/// The colours of a rendered basemap (RGB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    pub water: [u8; 3],
    pub land: [u8; 3],
    pub border: [u8; 3],
}

impl Colors {
    pub const DARK: Colors = Colors { water: [28, 34, 42], land: [58, 62, 64], border: [96, 100, 104] };
    pub const LIGHT: Colors = Colors { water: [170, 204, 228], land: [236, 234, 224], border: [168, 160, 150] };
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).ok_or("basemap: offset overflow")?;
        let bytes = self.data.get(self.at..end).ok_or("basemap: the data ends early")?;
        self.at = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    fn layer(&mut self) -> Result<Vec<Path>, String> {
        let paths = self.u32()?;
        if paths > MAX_PATHS {
            return Err(format!("basemap: {paths} paths in a layer"));
        }
        let mut out = Vec::new();
        for _ in 0..paths {
            let n = self.u32()?;
            if n > MAX_POINTS {
                return Err(format!("basemap: {n} points in a path"));
            }
            let bytes = self.take(n.checked_mul(4).ok_or("basemap: size overflow")?)?;
            let points = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| {
                    let lon = f64::from(i16::from_le_bytes([c[0], c[1]])) * 180.0 / 32767.0;
                    let lat = f64::from(i16::from_le_bytes([c[2], c[3]])) * 90.0 / 32767.0;
                    project(lat, lon)
                })
                .collect();
            out.push(Path::new(points));
        }
        Ok(out)
    }
}

impl Basemap {
    /// Read the outlines from `data` (the format of `tools/natural_earth_basemap.py`).
    pub fn parse(data: &[u8]) -> Result<Basemap, String> {
        let mut r = Reader { data, at: 0 };
        if r.take(MAGIC.len())? != MAGIC {
            return Err("basemap: not a LightCraft basemap file".into());
        }
        Ok(Basemap { land: r.layer()?, lakes: r.layer()?, borders: r.layer()? })
    }

    /// The built-in world map (parsed once). If its data were damaged the map would be all water rather than
    /// an error nobody can act on.
    pub fn builtin() -> &'static Basemap {
        static MAP: OnceLock<Basemap> = OnceLock::new();
        MAP.get_or_init(|| Basemap::parse(DATA).unwrap_or_default())
    }

    /// Draw the map for `view` as `width` × `height` RGBA pixels (opaque). `view`'s size is in points; the
    /// picture covers the same area at whatever density `width` / `view.width` gives. `None` when the picture
    /// would be empty or larger than [`MAX_PIXELS`] per edge.
    pub fn render(&self, view: &View, width: usize, height: usize, colors: Colors) -> Option<Vec<u8>> {
        if width == 0 || height == 0 || width > MAX_PIXELS || height > MAX_PIXELS {
            return None;
        }
        let mut frame = Frame::new(view, width, height);
        let mut out = vec![0u8; width * height * 4];
        for px in out.as_chunks_mut::<4>().0 {
            *px = [colors.water[0], colors.water[1], colors.water[2], 255];
        }
        frame.fill(&self.land, colors.land, &mut out);
        frame.fill(&self.lakes, colors.water, &mut out);
        for path in &self.borders {
            frame.stroke(path, colors.border, &mut out);
        }
        Some(out)
    }
}

/// A picture being drawn: where world coordinates land in it.
struct Frame {
    width: usize,
    height: usize,
    /// Pixels per world unit, and the world position of the picture's top-left corner.
    scale: f64,
    left: f64,
    top: f64,
    /// Per row: where path edges cross its centre line (pixels).
    rows: Vec<Vec<f32>>,
}

impl Frame {
    fn new(view: &View, width: usize, height: usize) -> Frame {
        let scale = view.scale() * width as f64 / view.width.max(1.0);
        let (cx, cy) = project(view.lat, view.lon);
        Frame { width, height, scale, left: cx - width as f64 / 2.0 / scale, top: cy - height as f64 / 2.0 / scale, rows: vec![Vec::new(); height] }
    }

    /// The copies of the world (whole numbers added to x) in which a path spanning `min`..`max` can be seen.
    fn copies(&self, min: (f64, f64), max: (f64, f64)) -> std::ops::Range<i32> {
        let (right, bottom) = (self.left + self.width as f64 / self.scale, self.top + self.height as f64 / self.scale);
        if max.1 < self.top || min.1 > bottom || !(min.0.is_finite() && max.0.is_finite()) {
            return 0..0;
        }
        // k with min.0 + k <= right and max.0 + k >= left; the view is at most a few worlds wide
        let first = (self.left - max.0).ceil().clamp(-8.0, 8.0) as i32;
        let last = (right - min.0).floor().clamp(-8.0, 8.0) as i32;
        first..last.saturating_add(1).max(first)
    }

    fn fill(&mut self, paths: &[Path], color: [u8; 3], out: &mut [u8]) {
        for row in &mut self.rows {
            row.clear();
        }
        for path in paths {
            for k in self.copies(path.min, path.max) {
                let pts = &path.points;
                let Some(last) = pts.last() else { continue };
                let mut a = *last;
                for b in pts {
                    self.edge((a.0 + f64::from(k), a.1), (b.0 + f64::from(k), b.1));
                    a = *b;
                }
            }
        }
        let width = self.width as f32;
        for (y, row) in self.rows.iter_mut().enumerate() {
            row.sort_unstable_by(f32::total_cmp);
            for pair in row.as_chunks::<2>().0 {
                let (x0, x1) = (pair[0].clamp(0.0, width), pair[1].clamp(0.0, width));
                if x1 <= x0 {
                    continue;
                }
                // whole pixels are painted solid, the two ends by how much of them the span covers
                let (first, last) = (x0.floor() as usize, (x1.ceil() as usize).min(self.width));
                for x in first..last {
                    let cover = (x1.min(x as f32 + 1.0) - x0.max(x as f32)).clamp(0.0, 1.0);
                    blend(out, (y * self.width + x) * 4, color, cover);
                }
            }
        }
    }

    /// Record where the edge `a`–`b` (world) crosses the centre line of each row it spans.
    fn edge(&mut self, a: (f64, f64), b: (f64, f64)) {
        let (ax, ay) = ((a.0 - self.left) * self.scale, (a.1 - self.top) * self.scale);
        let (bx, by) = ((b.0 - self.left) * self.scale, (b.1 - self.top) * self.scale);
        if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) || ay == by {
            return;
        }
        let ((x0, y0), (x1, y1)) = if ay < by { ((ax, ay), (bx, by)) } else { ((bx, by), (ax, ay)) };
        // rows whose centre (y + 0.5) lies in y0..y1 (half-open, so a shared vertex counts once)
        let first = (y0 - 0.5).ceil().max(0.0);
        let last = ((y1 - 0.5).ceil()).min(self.height as f64);
        if first >= last {
            return;
        }
        let slope = (x1 - x0) / (y1 - y0);
        for y in first as usize..last as usize {
            let x = x0 + (y as f64 + 0.5 - y0) * slope;
            if let Some(row) = self.rows.get_mut(y) {
                row.push(x.clamp(-1.0e9, 1.0e9) as f32);
            }
        }
    }

    /// Draw an open path one pixel wide.
    fn stroke(&self, path: &Path, color: [u8; 3], out: &mut [u8]) {
        for k in self.copies(path.min, path.max) {
            for pair in path.points.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (ax, ay) = ((a.0 + f64::from(k) - self.left) * self.scale, (a.1 - self.top) * self.scale);
                let (bx, by) = ((b.0 + f64::from(k) - self.left) * self.scale, (b.1 - self.top) * self.scale);
                let (w, h) = (self.width as f64, self.height as f64);
                if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite())
                    || (ax < 0.0 && bx < 0.0)
                    || (ay < 0.0 && by < 0.0)
                    || (ax > w && bx > w)
                    || (ay > h && by > h)
                {
                    continue;
                }
                // a segment partly outside is walked only over the picture's own extent
                let steps = (bx - ax).abs().max((by - ay).abs()).ceil().clamp(1.0, (w + h) * 4.0);
                let n = steps as usize;
                for i in 0..=n {
                    let t = i as f64 / steps;
                    let (x, y) = (ax + (bx - ax) * t, ay + (by - ay) * t);
                    if x >= 0.0 && y >= 0.0 && x < w && y < h {
                        blend(out, (y as usize * self.width + x as usize) * 4, color, 0.8);
                    }
                }
            }
        }
    }
}

fn blend(out: &mut [u8], at: usize, color: [u8; 3], cover: f32) {
    let Some(px) = out.get_mut(at..at + 3) else { return };
    for (p, c) in px.iter_mut().zip(color) {
        *p = (f32::from(*p) + (f32::from(c) - f32::from(*p)) * cover).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Colors = Colors { water: [0, 0, 200], land: [0, 200, 0], border: [200, 0, 0] };

    fn pixel(img: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
        let at = (y * width + x) * 4;
        [img[at], img[at + 1], img[at + 2]]
    }

    fn at(view: &View, img: &[u8], lat: f64, lon: f64) -> [u8; 3] {
        let (x, y) = view.to_screen(lat, lon);
        pixel(img, view.width as usize, x as usize, y as usize)
    }

    #[test]
    fn the_built_in_map_has_land_where_the_continents_are() {
        let map = Basemap::builtin();
        assert!(map.land.len() > 1000 && map.lakes.len() > 100 && map.borders.len() > 100);
        let view = View::new(20.0, 0.0, 2.0, 1024.0, 640.0);
        let img = map.render(&view, 1024, 640, C).unwrap();
        assert_eq!(img.len(), 1024 * 640 * 4);
        assert!(img.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // Sahara, Siberia, Brazil, Australia: land. Mid-Atlantic, Pacific, Indian Ocean: water
        for (lat, lon) in [(23.0, 10.0), (62.0, 100.0), (-10.0, -55.0), (-25.0, 135.0)] {
            assert_eq!(at(&view, &img, lat, lon), C.land, "{lat},{lon}");
        }
        for (lat, lon) in [(30.0, -40.0), (0.0, -150.0), (-30.0, 80.0)] {
            assert_eq!(at(&view, &img, lat, lon), C.water, "{lat},{lon}");
        }
        // the Caspian Sea is a hole in the land, Lake Superior a lake on it
        let asia = View::new(42.0, 51.0, 5.0, 800.0, 600.0);
        assert_eq!(at(&asia, &map.render(&asia, 800, 600, C).unwrap(), 42.0, 51.0), C.water);
        let lakes = View::new(47.7, -87.5, 6.0, 800.0, 600.0);
        assert_eq!(at(&lakes, &map.render(&lakes, 800, 600, C).unwrap(), 47.7, -87.5), C.water);
        // borders are drawn
        assert!(img.as_chunks::<4>().0.iter().any(|p| p[0] > 100));
    }

    #[test]
    fn the_map_repeats_across_the_antimeridian() {
        // centred on the date line: Fiji (east of 177° E) and Alaska's Aleutians side both in view
        let view = View::new(-17.8, 180.0, 5.0, 1000.0, 600.0);
        let img = Basemap::builtin().render(&view, 1000, 600, C).unwrap();
        assert_eq!(at(&view, &img, -17.8, 178.0), C.land, "Viti Levu");
        assert_eq!(at(&view, &img, -16.6, 179.3), C.land, "Vanua Levu");
        // no continent-sized smear of land: most of the picture is ocean
        let land = img.as_chunks::<4>().0.iter().filter(|p| p[1] == 200).count();
        assert!(land * 20 < 1000 * 600, "{land}");
    }

    #[test]
    fn a_denser_picture_shows_the_same_area() {
        let view = View::new(46.8, 8.2, 6.0, 400.0, 300.0);
        let map = Basemap::builtin();
        let (one, two) = (map.render(&view, 400, 300, C).unwrap(), map.render(&view, 800, 600, C).unwrap());
        for (x, y) in [(50, 50), (200, 150), (350, 250), (100, 280)] {
            let (a, b) = (pixel(&one, 400, x, y), pixel(&two, 800, x * 2 + 1, y * 2 + 1));
            assert!(a == b || a == C.border || b == C.border, "{x},{y}: {a:?} {b:?}");
        }
    }

    #[test]
    fn impossible_sizes_and_damaged_data_are_refused_not_fatal() {
        let view = View::world(800.0, 600.0);
        let map = Basemap::builtin();
        assert!(map.render(&view, 0, 10, C).is_none());
        assert!(map.render(&view, 10, 0, C).is_none());
        assert!(map.render(&view, MAX_PIXELS + 1, 10, C).is_none());
        assert!(Basemap::parse(b"").is_err());
        assert!(Basemap::parse(b"NOTAMAP!\0\0\0\0").is_err());
        // truncated at every length: an error, never a panic
        for n in (0..DATA.len().min(4096)).chain([DATA.len() - 1]) {
            assert!(Basemap::parse(&DATA[..n]).is_err(), "{n}");
        }
        // counts far beyond the data
        let mut huge = MAGIC.to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(Basemap::parse(&huge).is_err());
        let mut long_path = MAGIC.to_vec();
        long_path.extend_from_slice(&1u32.to_le_bytes());
        long_path.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(Basemap::parse(&long_path).is_err());
        // an empty map renders as water
        let empty = Basemap::default().render(&view, 8, 8, C).unwrap();
        assert!(empty.as_chunks::<4>().0.iter().all(|p| p[..3] == C.water));
        // degenerate paths (one point, non-finite) are skipped
        let odd = Basemap {
            land: vec![Path::new(vec![(0.5, 0.5)]), Path::new(vec![(f64::NAN, 0.1), (0.2, f64::INFINITY), (0.3, 0.3)])],
            ..Default::default()
        };
        assert!(odd.render(&view, 64, 64, C).is_some());
    }
}
