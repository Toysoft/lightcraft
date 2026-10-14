//! The map view's model: a Web Mercator viewport, photos stacked by place, tile addresses and the
//! offline basemap ([`basemap`]). No UI and no network code. See README.md.
//!
//! Sources: Web Mercator and slippy-map tile numbering (OpenStreetMap wiki, prose); stacking own design.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod basemap;

use serde::Serialize;

/// How many photos the map shows at once unless set otherwise (Settings ▸ Interface ▸ Map).
pub const MAX_PHOTOS: usize = 100;
/// The limits Settings offers.
pub const PHOTO_LIMITS: [usize; 5] = [50, 100, 250, 500, 1000];
/// No limit is higher than this: every marker is a thumbnail, and stacking compares each photo with each stack.
pub const MOST_PHOTOS: usize = 1000;

/// `max` made a usable limit (1 to [`MOST_PHOTOS`]; 0 means the default).
pub fn photo_limit(max: usize) -> usize {
    if max == 0 { MAX_PHOTOS } else { max.min(MOST_PHOTOS) }
}
/// The edge of a map tile at its own zoom level, in points.
pub const TILE: f64 = 256.0;
/// The deepest zoom (OpenStreetMap's standard tiles end here).
pub const MAX_ZOOM: f64 = 19.0;
/// Web Mercator ends at this latitude (the world is then a square).
pub const MAX_LAT: f64 = 85.051_128_779_806_59;
/// Photos closer than this on screen (points) form one stack.
pub const STACK_RADIUS: f64 = 44.0;
/// The largest view edge handled (points); anything larger is treated as this.
pub const MAX_EDGE: f64 = 16_384.0;
/// Most tiles one view asks for.
const MAX_TILES: usize = 400;

/// Whether `lat`, `lon` (degrees) is a position on the globe.
pub fn valid(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)
}

/// Degrees to world coordinates: x and y in 0..1, x east from the antimeridian, y south from the top.
pub fn project(lat: f64, lon: f64) -> (f64, f64) {
    let lat = if lat.is_finite() { lat.clamp(-MAX_LAT, MAX_LAT) } else { 0.0 };
    let lon = if lon.is_finite() { lon.clamp(-180.0, 180.0) } else { 0.0 };
    let s = lat.to_radians().sin();
    let y = 0.5 - ((1.0 + s) / (1.0 - s)).ln() / (4.0 * std::f64::consts::PI);
    (lon / 360.0 + 0.5, y.clamp(0.0, 1.0))
}

/// World coordinates back to degrees (x is wrapped around the globe, y clamped to it).
pub fn unproject(x: f64, y: f64) -> (f64, f64) {
    let x = if x.is_finite() { x.rem_euclid(1.0) } else { 0.5 };
    let y = if y.is_finite() { y.clamp(0.0, 1.0) } else { 0.5 };
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    ((lat).clamp(-MAX_LAT, MAX_LAT), x * 360.0 - 180.0)
}

/// What the map shows: its centre (degrees), zoom level and size in points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct View {
    pub lat: f64,
    pub lon: f64,
    /// Zoom level: the world is `256 · 2^zoom` points wide.
    pub zoom: f64,
    pub width: f64,
    pub height: f64,
}

/// A tile of the view: its address and where its top-left corner and edge are on screen (points).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Tile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub left: f64,
    pub top: f64,
    pub edge: f64,
}

impl View {
    /// A view of `width` × `height` points; nonsense values become usable ones (the centre moves onto the
    /// globe, the zoom into its range, and the map never shows less than its own height of the world).
    pub fn new(lat: f64, lon: f64, zoom: f64, width: f64, height: f64) -> View {
        let edge = |v: f64| if v.is_finite() { v.clamp(1.0, MAX_EDGE) } else { 1.0 };
        let (width, height) = (edge(width), edge(height));
        let min_zoom = (height / TILE).log2().max(0.0);
        let zoom = if zoom.is_finite() { zoom.clamp(min_zoom.min(MAX_ZOOM), MAX_ZOOM) } else { min_zoom.min(MAX_ZOOM) };
        let mut v = View { lat: 0.0, lon: 0.0, zoom, width, height };
        let (x, y) = project(lat, lon);
        v.set_centre(x, y);
        v
    }

    /// The whole world, as far as it fits.
    pub fn world(width: f64, height: f64) -> View {
        View::new(20.0, 0.0, 0.0, width, height)
    }

    /// Points per world unit.
    pub fn scale(&self) -> f64 {
        TILE * self.zoom.exp2()
    }

    fn centre(&self) -> (f64, f64) {
        project(self.lat, self.lon)
    }

    /// Centre on world `x`, `y`, keeping the top and bottom of the world out of the view.
    fn set_centre(&mut self, x: f64, y: f64) {
        let half = (self.height / 2.0 / self.scale()).min(0.5);
        let y = if y.is_finite() { y.clamp(half, 1.0 - half) } else { 0.5 };
        (self.lat, self.lon) = unproject(x, y);
    }

    /// Where a world position is on screen. `x` takes the copy of the world nearest the view's centre.
    fn world_to_screen(&self, x: f64, y: f64) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let mut dx = (x - cx).rem_euclid(1.0);
        if dx > 0.5 {
            dx -= 1.0;
        }
        (self.width / 2.0 + dx * self.scale(), self.height / 2.0 + (y - cy) * self.scale())
    }

    /// Where `lat`, `lon` is on screen (points from the view's top-left corner).
    pub fn to_screen(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (x, y) = project(lat, lon);
        self.world_to_screen(x, y)
    }

    /// The position under a screen point.
    pub fn from_screen(&self, x: f64, y: f64) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let (x, y) = (if x.is_finite() { x } else { self.width / 2.0 }, if y.is_finite() { y } else { self.height / 2.0 });
        unproject(cx + (x - self.width / 2.0) / self.scale(), cy + (y - self.height / 2.0) / self.scale())
    }

    /// The view after dragging the map by `dx`, `dy` points.
    pub fn panned(&self, dx: f64, dy: f64) -> View {
        let (cx, cy) = self.centre();
        let d = |v: f64| if v.is_finite() { v / self.scale() } else { 0.0 };
        let mut v = *self;
        v.set_centre(cx - d(dx), cy - d(dy));
        v
    }

    /// The view at `zoom`, with the position under screen point `x`, `y` staying there.
    pub fn zoomed_about(&self, x: f64, y: f64, zoom: f64) -> View {
        let (lat, lon) = self.from_screen(x, y);
        let (wx, wy) = project(lat, lon);
        let mut v = View::new(self.lat, self.lon, zoom, self.width, self.height);
        let (x, y) = (if x.is_finite() { x } else { self.width / 2.0 }, if y.is_finite() { y } else { self.height / 2.0 });
        // the centre that puts (wx, wy) under (x, y); the nearest copy of the world, as on screen
        let (cx, _) = self.centre();
        let mut dx = (wx - cx).rem_euclid(1.0);
        if dx > 0.5 {
            dx -= 1.0;
        }
        v.set_centre(cx + dx - (x - v.width / 2.0) / v.scale(), wy - (y - v.height / 2.0) / v.scale());
        v
    }

    /// The closest view that shows every position in `points` (lat, lon) with `margin` points around them; one
    /// place (or several at the same spot) is shown at street level, none gives the world.
    pub fn fit(points: &[(f64, f64)], width: f64, height: f64, margin: f64) -> View {
        let world: Vec<(f64, f64)> = points.iter().filter(|(la, lo)| valid(*la, *lo)).map(|(la, lo)| project(*la, *lo)).collect();
        let Some(first) = world.first().copied() else { return View::world(width, height) };
        // longitudes relative to the first one, so a group across the antimeridian stays together
        let rel = |x: f64| {
            let d = (x - first.0).rem_euclid(1.0);
            if d > 0.5 { d - 1.0 } else { d }
        };
        let (mut x0, mut x1, mut y0, mut y1) = (0.0_f64, 0.0_f64, first.1, first.1);
        for (x, y) in &world {
            (x0, x1, y0, y1) = (x0.min(rel(*x)), x1.max(rel(*x)), y0.min(*y), y1.max(*y));
        }
        let base = View::new(0.0, 0.0, 0.0, width, height);
        let margin = if margin.is_finite() { margin.max(0.0) } else { 0.0 };
        let room = |edge: f64| (edge - 2.0 * margin).max(edge / 4.0);
        let span = |d: f64, edge: f64| if d > 0.0 { (room(edge) / (d * TILE)).log2() } else { f64::INFINITY };
        let zoom = span(x1 - x0, base.width).min(span(y1 - y0, base.height)).min(15.0);
        let (lat, lon) = unproject(first.0 + (x0 + x1) / 2.0, (y0 + y1) / 2.0);
        View::new(lat, lon, zoom, width, height)
    }

    /// The tiles covering the view, at the tile zoom level nearest the view's zoom.
    pub fn tiles(&self) -> Vec<Tile> {
        let z = self.zoom.round().clamp(0.0, MAX_ZOOM) as u8;
        let n = 1u32 << z;
        let edge = TILE * (self.zoom - f64::from(z)).exp2();
        let (cx, cy) = self.centre();
        // the view's top-left corner in tile units (x may be negative or beyond n: the world repeats)
        let left = cx * f64::from(n) - self.width / 2.0 / edge;
        let top = cy * f64::from(n) - self.height / 2.0 / edge;
        let (x0, x1) = (left.floor() as i64, (left + self.width / edge).ceil() as i64);
        let (y0, y1) = ((top.floor() as i64).max(0), ((top + self.height / edge).ceil() as i64).min(i64::from(n)));
        let mut out = Vec::new();
        for ty in y0..y1 {
            for tx in x0..x1 {
                if out.len() >= MAX_TILES {
                    return out;
                }
                out.push(Tile {
                    z,
                    x: tx.rem_euclid(i64::from(n)) as u32,
                    y: ty as u32,
                    left: (tx as f64 - left) * edge,
                    top: (ty as f64 - top) * edge,
                    edge,
                });
            }
        }
        out
    }
}

/// A photo with a position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub id: u64,
    pub lat: f64,
    pub lon: f64,
}

/// Photos shown as one marker: their ids (in the order given), where the marker sits on screen and on the globe.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stack {
    pub ids: Vec<u64>,
    pub lat: f64,
    pub lon: f64,
    pub x: f64,
    pub y: f64,
}

/// The photos to show: the first `max` of `photos` that have a valid position (`max` as [`photo_limit`] makes it
/// usable). Returns them and how many photos with a position were left out.
pub fn limit(photos: impl IntoIterator<Item = Pin>, max: usize) -> (Vec<Pin>, usize) {
    let max = photo_limit(max);
    let mut kept = Vec::new();
    let mut dropped = 0;
    for p in photos.into_iter().filter(|p| valid(p.lat, p.lon)) {
        if kept.len() < max {
            kept.push(p);
        } else {
            dropped += 1;
        }
    }
    (kept, dropped)
}

/// Group `pins` into stacks for `view`: a photo joins the nearest stack whose first photo is within `radius`
/// points on screen, else starts a new one. Photos at the same position are therefore always one stack. The
/// order of `pins` decides which photo is a stack's first (its picture) and the order of the stacks.
pub fn stacks(pins: &[Pin], view: &View, radius: f64) -> Vec<Stack> {
    let radius = if radius.is_finite() { radius.max(0.0) } else { STACK_RADIUS };
    // (anchor on screen, world sums for the marker's position, ids)
    let mut groups: Vec<((f64, f64), (f64, f64), Vec<u64>)> = Vec::new();
    for pin in pins.iter().filter(|p| valid(p.lat, p.lon)).take(MOST_PHOTOS) {
        let at = view.to_screen(pin.lat, pin.lon);
        let nearest =
            groups.iter_mut().map(|g| ((g.0.0 - at.0).hypot(g.0.1 - at.1), g)).filter(|(d, _)| *d <= radius).min_by(|a, b| a.0.total_cmp(&b.0));
        match nearest {
            Some((_, g)) => {
                g.1 = (g.1.0 + at.0, g.1.1 + at.1);
                g.2.push(pin.id);
            }
            None => groups.push((at, at, vec![pin.id])),
        }
    }
    groups
        .into_iter()
        .map(|(_, sum, ids)| {
            let n = ids.len().max(1) as f64;
            let (x, y) = (sum.0 / n, sum.1 / n);
            let (lat, lon) = view.from_screen(x, y);
            Stack { ids, lat, lon, x, y }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(id: u64, lat: f64, lon: f64) -> Pin {
        Pin { id, lat, lon }
    }

    #[test]
    fn projection_round_trips_and_survives_nonsense() {
        for (lat, lon) in [(0.0, 0.0), (48.8566, 2.3522), (-33.87, 151.21), (64.15, -21.94), (85.0, 179.9), (-85.0, -179.9)] {
            let (x, y) = project(lat, lon);
            let (la, lo) = unproject(x, y);
            assert!((la - lat).abs() < 1e-9 && (lo - lon).abs() < 1e-9, "{lat},{lon} → {la},{lo}");
        }
        assert_eq!(project(0.0, 0.0), (0.5, 0.5));
        // the poles are outside Web Mercator: clamped to its edge, never infinite
        assert_eq!(project(90.0, 0.0).1, 0.0);
        assert_eq!(project(-90.0, 0.0).1, 1.0);
        for v in [f64::NAN, f64::INFINITY, -f64::INFINITY, 1e300] {
            let (x, y) = project(v, v);
            assert!(x.is_finite() && y.is_finite());
            let (la, lo) = unproject(v, v);
            assert!(valid(la, lo));
        }
        assert!(!valid(f64::NAN, 0.0) && !valid(91.0, 0.0) && !valid(0.0, 181.0) && valid(-90.0, 180.0));
    }

    #[test]
    fn a_view_made_from_nonsense_is_usable() {
        for v in [f64::NAN, f64::INFINITY, -1e300, 0.0, -5.0] {
            let view = View::new(v, v, v, v, v);
            assert!(valid(view.lat, view.lon) && view.zoom.is_finite() && view.width >= 1.0 && view.height >= 1.0, "{view:?}");
            assert!(view.tiles().len() <= MAX_TILES);
            let _ = (view.panned(v, v), view.zoomed_about(v, v, v), view.from_screen(v, v), view.to_screen(v, v));
        }
        assert_eq!(View::new(0.0, 0.0, 99.0, 800.0, 600.0).zoom, MAX_ZOOM);
        // a huge window still asks for a bounded number of tiles
        assert!(View::new(0.0, 0.0, 19.0, 1e9, 1e9).tiles().len() <= MAX_TILES);
    }

    #[test]
    fn the_centre_is_in_the_middle_and_screen_positions_round_trip() {
        let view = View::new(48.0, 11.0, 9.0, 1000.0, 600.0);
        let (x, y) = view.to_screen(48.0, 11.0);
        assert!((x - 500.0).abs() < 1e-6 && (y - 300.0).abs() < 1e-6);
        let (lat, lon) = view.from_screen(120.0, 77.0);
        let (x, y) = view.to_screen(lat, lon);
        assert!((x - 120.0).abs() < 1e-6 && (y - 77.0).abs() < 1e-6);
        // north is up, east is right
        assert!(view.to_screen(49.0, 11.0).1 < 300.0 && view.to_screen(48.0, 12.0).0 > 500.0);
    }

    #[test]
    fn panning_moves_the_map_with_the_pointer_and_stops_at_the_poles() {
        let view = View::new(10.0, 20.0, 6.0, 800.0, 600.0);
        let moved = view.panned(100.0, -50.0);
        let (x, y) = moved.to_screen(10.0, 20.0);
        assert!((x - 500.0).abs() < 1e-6 && (y - 250.0).abs() < 1e-6, "{x},{y}");
        // dragged far down: the top of the world stays at the top edge of the view
        let top = view.panned(0.0, 1e9);
        assert!(top.to_screen(MAX_LAT, 20.0).1.abs() < 1e-6);
        // across the antimeridian the longitude wraps
        let east = View::new(0.0, 179.0, 6.0, 800.0, 600.0).panned(-400.0, 0.0);
        assert!(east.lon < -170.0, "{}", east.lon);
    }

    #[test]
    fn zooming_keeps_the_place_under_the_pointer() {
        let view = View::new(47.0, 8.0, 7.0, 900.0, 700.0);
        let under = view.from_screen(700.0, 150.0);
        let zoomed = view.zoomed_about(700.0, 150.0, 9.5);
        assert_eq!(zoomed.zoom, 9.5);
        let (x, y) = zoomed.to_screen(under.0, under.1);
        assert!((x - 700.0).abs() < 1e-6 && (y - 150.0).abs() < 1e-6, "{x},{y}");
    }

    #[test]
    fn fit_shows_every_photo_inside_the_margin() {
        let points = [(48.14, 11.58), (52.52, 13.40), (53.55, 9.99)];
        let view = View::fit(&points, 800.0, 600.0, 60.0);
        for (lat, lon) in points {
            let (x, y) = view.to_screen(lat, lon);
            assert!((59.0..=741.0).contains(&x) && (59.0..=541.0).contains(&y), "{x},{y}");
        }
        // one more zoom level would not fit
        let closer = View::new(view.lat, view.lon, view.zoom + 1.0, 800.0, 600.0);
        assert!(points.iter().any(|(la, lo)| {
            let (x, y) = closer.to_screen(*la, *lo);
            !(60.0..=740.0).contains(&x) || !(60.0..=540.0).contains(&y)
        }));
        // one place: street level, centred; none or only invalid ones: the world
        let one = View::fit(&[(48.14, 11.58), (48.14, 11.58)], 800.0, 600.0, 60.0);
        assert_eq!(one.zoom, 15.0);
        assert!((one.lat - 48.14).abs() < 1e-6 && (one.lon - 11.58).abs() < 1e-6);
        assert_eq!(View::fit(&[], 800.0, 600.0, 60.0), View::world(800.0, 600.0));
        assert_eq!(View::fit(&[(f64::NAN, 0.0), (200.0, 0.0)], 800.0, 600.0, 60.0), View::world(800.0, 600.0));
        // Fiji and Samoa, either side of the antimeridian: a close view over the Pacific, not the whole world
        let pacific = View::fit(&[(-17.7, 178.0), (-13.8, -172.0)], 800.0, 600.0, 60.0);
        assert!(pacific.zoom > 4.0 && (pacific.lon.abs() > 170.0), "{pacific:?}");
    }

    #[test]
    fn tiles_cover_the_view_without_gaps() {
        let view = View::new(48.14, 11.58, 10.3, 700.0, 500.0);
        let tiles = view.tiles();
        assert!(tiles.iter().all(|t| t.z == 10 && t.x < 1024 && t.y < 1024));
        assert!((tiles[0].edge - 256.0 * 0.3_f64.exp2()).abs() < 1e-9);
        // every corner and the centre lie in some tile
        for (x, y) in [(0.5, 0.5), (699.5, 0.5), (0.5, 499.5), (699.5, 499.5), (350.0, 250.0)] {
            assert!(tiles.iter().any(|t| x >= t.left && x < t.left + t.edge && y >= t.top && y < t.top + t.edge), "{x},{y}");
        }
        // Munich's tile at zoom 10 (OpenStreetMap wiki's formula): 544 / 355
        let centre = tiles.iter().find(|t| 350.0 >= t.left && 350.0 < t.left + t.edge && 250.0 >= t.top && 250.0 < t.top + t.edge).unwrap();
        assert_eq!((centre.x, centre.y), (544, 355));
        // the whole world at zoom 0 in a small view: tile 0/0/0, repeated sideways when the view is wider
        let world = View::new(0.0, 0.0, 0.0, 600.0, 256.0);
        assert!(world.tiles().iter().all(|t| (t.z, t.x, t.y) == (0, 0, 0)));
        assert_eq!(world.tiles().len(), 3);
    }

    #[test]
    fn photos_at_the_same_place_are_one_stack() {
        let view = View::new(48.14, 11.58, 12.0, 800.0, 600.0);
        let pins = [pin(1, 48.14, 11.58), pin(2, 48.14, 11.58), pin(3, 48.14, 11.58), pin(4, 48.20, 11.70), pin(5, 48.14, 11.58)];
        let s = stacks(&pins, &view, STACK_RADIUS);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].ids, vec![1, 2, 3, 5]);
        assert_eq!(s[1].ids, vec![4]);
        assert!((s[0].x - 400.0).abs() < 1e-6 && (s[0].y - 300.0).abs() < 1e-6);
        assert!((s[0].lat - 48.14).abs() < 1e-6 && (s[0].lon - 11.58).abs() < 1e-6);
        // the same place stays one stack at the deepest zoom and with no radius at all
        let deep = View::new(48.14, 11.58, MAX_ZOOM, 800.0, 600.0);
        assert_eq!(stacks(&pins[..3], &deep, 0.0).len(), 1);
    }

    #[test]
    fn nearby_photos_stack_when_zoomed_out_and_separate_when_zoomed_in() {
        // Marienplatz and the English Garden, about 2.5 km apart
        let pins = [pin(1, 48.1374, 11.5755), pin(2, 48.1593, 11.6030)];
        let far = View::new(48.14, 11.58, 6.0, 800.0, 600.0);
        assert_eq!(stacks(&pins, &far, STACK_RADIUS).len(), 1);
        let near = View::new(48.14, 11.58, 13.0, 800.0, 600.0);
        assert_eq!(stacks(&pins, &near, STACK_RADIUS).len(), 2);
        // the stack sits between its photos
        let s = &stacks(&pins, &far, STACK_RADIUS)[0];
        assert!(s.lat > 48.1374 && s.lat < 48.1593 && s.lon > 11.5755 && s.lon < 11.6030);
        // nonsense radius: the default
        assert_eq!(stacks(&pins, &far, f64::NAN).len(), 1);
    }

    #[test]
    fn at_most_the_limit_and_only_photos_with_a_position() {
        let many = || (0..250).map(|i| pin(i, if i % 5 == 0 { f64::NAN } else { 10.0 + i as f64 * 0.01 }, 20.0));
        // a limit of its own, and 0 for the default
        assert_eq!(limit(many(), 50).0.len(), 50);
        assert_eq!(limit(many(), 1000), (many().filter(|p| p.lat.is_finite()).collect::<Vec<_>>(), 0));
        let (kept, dropped) = limit(many(), 0);
        assert_eq!((kept.len(), dropped), (MAX_PHOTOS, 100));
        assert_eq!(kept[0].id, 1);
        assert!(kept.iter().all(|p| p.id % 5 != 0));
        // nonsense limits are made usable
        assert_eq!((photo_limit(0), photo_limit(7), photo_limit(usize::MAX)), (MAX_PHOTOS, 7, MOST_PHOTOS));
        // stacks never hold more than the highest limit either, whatever they are given
        let all: Vec<Pin> = (0..1500).map(|i| pin(i, 1.0, 1.0)).collect();
        let s = stacks(&all, &View::world(800.0, 600.0), STACK_RADIUS);
        assert_eq!(s.iter().map(|s| s.ids.len()).sum::<usize>(), MOST_PHOTOS);
        assert!(stacks(&[], &View::world(800.0, 600.0), STACK_RADIUS).is_empty());
    }
}
