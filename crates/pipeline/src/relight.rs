//! Relight: lights added to the photo, in scene-linear light.
//!
//! The scene's relief comes from one of two places. A photo that was analyzed carries its geometry
//! in its settings ([`RelightGeometry`]: surface normals and depth as a model saw them, MoGe-2 in
//! `lightcraft-relief`); the normals are used as they are, joined with the slopes of the depth,
//! and the depth places every surface in front of or behind the lights and casts the shadows.
//! Without it there is no depth to go by, so the relief is estimated from the photo itself: its
//! tone-compressed luminance, smoothed, is read as a height field (brighter = nearer) whose
//! slopes give surface normals. That smoothing is wide and not edge-aware on purpose: an outline
//! in the photo becomes a gentle roll of the relief, not a cliff that a light would trace as a
//! bright or dark rim. Each [`RelightLight`] shades that relief (diffuse
//! light, an optional specular "shine", optional cast shadows marched through the height field);
//! the sum, plus the photo's own light scaled by Ambient, is a gain that multiplies the photo.
//!
//! The relief and the gain are always computed on a small render of the *whole, uncropped* frame
//! ([`PROXY_EDGE`] px on its long edge) and upsampled to the output with the photo's luminance as
//! a guide, so that edges stay where the photo has them. Because that proxy does not depend on
//! the output's size, crop or window, a thumbnail, a zoomed window and a full-size export are lit
//! alike, and lights keep their place on the photo when it is cropped.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_color::perceptual::hsv_to_rgb;
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_color::{REC2020, SRGB, luminance_2020};
use lightcraft_develop::{DevelopSettings, MAX_RELIGHT_LIGHTS, Relight, RelightGeometry, RelightKind, RelightLight};
use lightcraft_geom::{Affine, Point};
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};

use crate::geometry::Frame;
use crate::{Plan, SourceInfo, for_rows, hash_of};

/// Long edge, in pixels, of the render of the whole frame the relief and the lighting are computed on.
pub const PROXY_EDGE: usize = 512;

/// Height of the brightest relief above the darkest, in units of the image's long edge, when the
/// relief is estimated from the photo's brightness.
const RELIEF: f32 = 0.12;

/// Height of the nearest surface above the farthest when the depth is known ([`RelightGeometry`])
/// but its slopes say nothing about its scale (a flat scene), and the least and most that scale
/// may be otherwise.
const RELIEF_SEEN: f32 = 0.3;
const RELIEF_SEEN_RANGE: (f32, f32) = (0.05, 2.5);

/// Slopes of the stored depth steeper than this (height per long edge) are the steps between
/// things, not surfaces: they say nothing about the scale of the depth.
const STEP: f32 = 4.0;

/// How much of the normal is taken from the slope of the depth, the rest from the model's normals:
/// the two agree on smooth surfaces, and the depth adds the steps between things.
const DEPTH_SHARE: f32 = 0.25;

/// Steepest slope of the relief (rise over run): steeper ones are eased towards it.
const MAX_SLOPE: f32 = 0.9;

/// Steps of a shadow ray.
const SHADOW_STEPS: usize = 28;

/// How far a Directional light's shadow ray travels (long-edge units).
const SHADOW_REACH: f32 = 0.5;

/// Largest gain a pixel can get (hostile settings, or many lights on one spot).
const MAX_GAIN: f32 = 64.0;

/// Range of the luminance guide over which two neighbouring proxy pixels count as the same surface.
const GUIDE_RANGE: f32 = 0.08;

/// How many surfaces / gain fields of recent renders are kept.
const MEMO: usize = 4;

/// The relief estimated for one photo and framing.
#[derive(Debug)]
pub struct Surface {
    /// Tone-compressed luminance of the proxy (`Y / (Y + mean)`), unfiltered: the upsampling guide.
    guide: Plane,
    /// Relief height, 0..1.
    height: Plane,
    /// Unit surface normals (z towards the viewer).
    normal: Rgb32f,
    /// Height of the highest relief (`height` = 1) in long-edge units.
    relief: f32,
    /// Mean luminance of the proxy (the guide's normalisation).
    mean: f32,
    /// Proxy long edge in pixels.
    long: f32,
    /// Normalized oriented-image coordinates → proxy pixels.
    norm_to_proxy: Affine,
    /// Oriented image size (any unit; only the ratio matters).
    ow: f64,
    oh: f64,
}

/// The lighting of one photo: a gain per proxy pixel, upsampled by [`apply`].
#[derive(Debug)]
pub struct Field {
    surface: Arc<Surface>,
    gain: Rgb32f,
}

#[derive(Default)]
struct Memo {
    surfaces: Vec<(u64, Arc<Surface>)>,
    fields: Vec<(u64, Arc<Field>)>,
}

static RECENT: Mutex<Memo> = Mutex::new(Memo { surfaces: Vec::new(), fields: Vec::new() });

fn recent<T: Clone>(pick: impl FnOnce(&mut Memo) -> &mut Vec<(u64, T)>, key: u64) -> Option<T> {
    let mut memo = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    pick(&mut memo).iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
}

fn remember<T>(pick: impl FnOnce(&mut Memo) -> &mut Vec<(u64, T)>, key: u64, value: T) {
    let mut memo = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    let list = pick(&mut memo);
    list.retain(|(k, _)| *k != key);
    list.push((key, value));
    while list.len() > MEMO {
        list.remove(0);
    }
}

/// What the lighting of `s` depends on besides the relief: every Relight setting but the geometry.
fn lights_key(s: &DevelopSettings) -> String {
    let r = &s.relight;
    format!("{:?}", (r.enabled, r.ambient, r.softness, r.shine, r.shadows, r.shadow_softness, &r.lights))
}

/// What a render's Relight depends on (`None`: nothing, it is off), for stage-cache keys.
pub fn key(s: &DevelopSettings) -> Option<u64> {
    active(s).then(|| hash_of((lights_key(s), s.relight.geometry.as_deref())))
}

/// Whether `s` relights the photo at all.
pub fn active(s: &DevelopSettings) -> bool {
    s.relight.is_active()
}

/// A finite number that stays finite as an `f32`, else `default`.
fn finite(v: f64, default: f64) -> f32 {
    (if v.is_finite() { v.clamp(-1e9, 1e9) } else { default }) as f32
}

/// `v / 100` clamped to 0..1 (`default` for a value that is not a number).
fn unit(v: f64, default: f64) -> f32 {
    (finite(v, default) / 100.0).clamp(0.0, 1.0)
}

/// What identifies the pixels of `src` without reading them all: its size and a grid of samples.
fn fingerprint(src: &Rgb32f) -> u64 {
    const GRID: usize = 24;
    let mut bits = Vec::with_capacity(GRID * GRID * 3 + 2);
    bits.push(src.width as u32);
    bits.push(src.height as u32);
    for j in 0..GRID {
        for i in 0..GRID {
            let (x, y) = (i * src.width / GRID, j * src.height / GRID);
            if let Some(p) = src.data.get(y * src.width + x) {
                bits.extend(p.map(f32::to_bits));
            }
        }
    }
    hash_of(bits)
}

/// The lighting of `src` under `s`, or `None` when Relight changes nothing.
pub fn resolve(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Option<Arc<Field>> {
    if !active(s) || src.width == 0 || src.height == 0 {
        return None;
    }
    let frame = Frame::with_lens(src.width, src.height, s, false, info.lens.as_ref());
    let (pw, ph) = frame.fit(PROXY_EDGE, PROXY_EDGE);
    let softness = unit(s.relight.softness, 40.0);
    let geometry = s.relight.geometry.as_deref();
    let surface_key = hash_of((fingerprint(src), format!("{frame:?}"), softness.to_bits(), pw, ph, geometry));
    let surface = match recent(|m| &mut m.surfaces, surface_key) {
        Some(found) => found,
        None => {
            let proxy = frame.sample(src, pw, ph);
            // (geometry that can't be read is no geometry: the relief is estimated instead)
            let seen = geometry.and_then(|g| Surface::seen(&proxy, &frame, softness, g));
            let built = Arc::new(seen.unwrap_or_else(|| Surface::estimate(&proxy, &frame, softness)));
            remember(|m| &mut m.surfaces, surface_key, built.clone());
            built
        }
    };
    let field_key = hash_of((surface_key, lights_key(s)));
    if let Some(found) = recent(|m| &mut m.fields, field_key) {
        return Some(found);
    }
    let field = Arc::new(Field { gain: shade(&surface, &s.relight), surface });
    remember(|m| &mut m.fields, field_key, field.clone());
    Some(field)
}

/// Luminance of a scene-linear pixel as the guide sees it: never negative, never not a number.
#[inline]
fn guide_luminance(c: [f32; 3]) -> f32 {
    let y = luminance_2020(c);
    if y.is_finite() { y.max(0.0) } else { 0.0 }
}

#[inline]
fn guide_of(y: f32, mean: f32) -> f32 {
    y / (y + mean)
}

/// Bilinear sample of `p` at continuous pixel coordinates (pixel centres at +0.5), edges extended.
#[inline]
fn bilinear(p: &Plane, x: f32, y: f32) -> f32 {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let top = p.get_clamped(x0, y0) + (p.get_clamped(x0 + 1, y0) - p.get_clamped(x0, y0)) * tx;
    let bottom = p.get_clamped(x0, y0 + 1) + (p.get_clamped(x0 + 1, y0 + 1) - p.get_clamped(x0, y0 + 1)) * tx;
    top + (bottom - top) * ty
}

impl Surface {
    /// Estimate the relief of `proxy` (the whole uncropped frame, sampled by `frame`). `softness` 0..1.
    fn estimate(proxy: &Rgb32f, frame: &Frame, softness: f32) -> Surface {
        let (w, h) = (proxy.width, proxy.height);
        let long = w.max(h).max(1) as f32;
        let (guide, mean) = Surface::guide(proxy);
        let sigma = ((0.01 + 0.05 * softness) * long).max(0.8);
        let height = gaussian(&guide, sigma);
        let mut normal = Rgb32f::new(w, h);
        for_rows(&mut normal.data, w, |y, row| {
            let y = y as isize;
            for (x, n) in row.iter_mut().enumerate() {
                let x = x as isize;
                // slope of the relief per long-edge unit (central differences, edges extended)
                let gx = (height.get_clamped(x + 1, y) - height.get_clamped(x - 1, y)) * 0.5 * long * RELIEF;
                let gy = (height.get_clamped(x, y + 1) - height.get_clamped(x, y - 1)) * 0.5 * long * RELIEF;
                // ease steep slopes towards MAX_SLOPE: no part of the relief turns its back on a light
                let ease = 1.0 / (1.0 + (gx * gx + gy * gy) / (MAX_SLOPE * MAX_SLOPE)).sqrt();
                let (gx, gy) = (gx * ease, gy * ease);
                let k = 1.0 / (gx * gx + gy * gy + 1.0).sqrt();
                *n = if k.is_finite() { [-gx * k, -gy * k, k] } else { [0.0, 0.0, 1.0] };
            }
        });
        Surface { guide, height, normal, relief: RELIEF, mean, long, norm_to_proxy: frame.norm_to_out(w, h), ow: frame.ow, oh: frame.oh }
    }

    /// The upsampling guide of `proxy` and the mean luminance it is normalized by.
    fn guide(proxy: &Rgb32f) -> (Plane, f32) {
        let luminance = proxy.map(guide_luminance);
        let total: f64 = luminance.data.iter().map(|v| *v as f64).sum();
        let mean = ((total / luminance.data.len().max(1) as f64) as f32).max(1e-6);
        (luminance.map(|y| guide_of(y, mean)), mean)
    }

    /// The relief of `proxy` as a model saw it: `geometry`'s normals and depth, resampled to the
    /// proxy. `softness` 0..1 smooths them. `None` when the geometry can't be read.
    fn seen(proxy: &Rgb32f, frame: &Frame, softness: f32, geometry: &RelightGeometry) -> Option<Surface> {
        let (gw, gh) = geometry.size()?;
        let samples = geometry.decode()?;
        let plane = |f: &dyn Fn(&lightcraft_develop::ReliefSample) -> f32| Plane { width: gw, height: gh, data: samples.iter().map(f).collect() };
        let (nx, ny, near) = (plane(&|s| s.nx), plane(&|s| s.ny), plane(&|s| s.height));
        let (w, h) = (proxy.width, proxy.height);
        let long = w.max(h).max(1) as f32;
        let (guide, mean) = Surface::guide(proxy);
        let mut surface = Surface {
            guide,
            height: Plane::new(w, h),
            normal: Rgb32f::new(w, h),
            relief: RELIEF_SEEN,
            mean,
            long,
            norm_to_proxy: frame.norm_to_out(w, h),
            ow: frame.ow,
            oh: frame.oh,
        };
        // the geometry lies over the unflipped image; the proxy may be flipped against it
        let to_norm = frame.out_to_norm(w, h);
        let (right, down) = (surface.direction(90.0), surface.direction(180.0));
        let at = |x: usize, y: usize| {
            let n = to_norm.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
            ((n.x * gw as f64) as f32, (n.y * gh as f64) as f32)
        };
        for_rows(&mut surface.height.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                let (gx, gy) = at(x, y);
                let sampled = bilinear(&near, gx, gy);
                *v = if sampled.is_finite() { sampled.clamp(0.0, 1.0) } else { 0.0 };
            }
        });
        let mut tilt = Rgb32f::new(w, h);
        for_rows(&mut tilt.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                let (gx, gy) = at(x, y);
                let (ix, iy) = (bilinear(&nx, gx, gy), bilinear(&ny, gx, gy));
                // along the proxy's own axes
                *v = [ix * right[0] + iy * down[0], ix * right[1] + iy * down[1], 0.0];
            }
        });
        let sigma = 0.02 * softness * long;
        if sigma > 0.5 {
            surface.height = gaussian(&surface.height, sigma);
            tilt = gaussian(&tilt, sigma);
        }
        let height = &surface.height;
        // the model's normal at a proxy pixel, and the slope of the stored depth there (height per long edge)
        let model_at = |x: usize, y: usize| {
            let [tx, ty, _] = tilt.get(x, y);
            let (tx, ty) = if tx.is_finite() && ty.is_finite() { (tx, ty) } else { (0.0, 0.0) };
            // a normal leaning further than a unit vector can is brought back onto the sphere
            let lean = (tx * tx + ty * ty).sqrt();
            let (tx, ty) = if lean > 0.99 { (tx * 0.99 / lean, ty * 0.99 / lean) } else { (tx, ty) };
            [tx, ty, (1.0 - tx * tx - ty * ty).max(0.0).sqrt()]
        };
        let slope_at = |x: usize, y: usize| {
            let (xi, yi) = (x as isize, y as isize);
            [
                (height.get_clamped(xi + 1, yi) - height.get_clamped(xi - 1, yi)) * 0.5 * long,
                (height.get_clamped(xi, yi + 1) - height.get_clamped(xi, yi - 1)) * 0.5 * long,
            ]
        };
        // The depth has no scale of its own, the normals do: a surface whose normal leans by so much
        // rises by so much. The scale that makes the depth's slopes agree with the normals (least
        // squares over the surfaces, the steps between things left out) is the scene's relief, so
        // that where a light is and which way a surface faces describe the same shape.
        let (mut agree, mut steepness) = (0f64, 0f64);
        for y in 0..h {
            for x in 0..w {
                let (g, n) = (slope_at(x, y), model_at(x, y));
                let steep = g[0] * g[0] + g[1] * g[1];
                if height.get(x, y) <= 0.0 || steep > STEP * STEP || n[2] < 0.2 {
                    continue;
                }
                // (a normal (nx, ny, nz) belongs to a surface rising by −nx / nz and −ny / nz)
                agree += f64::from(g[0] * -n[0] / n[2] + g[1] * -n[1] / n[2]);
                steepness += f64::from(steep);
            }
        }
        let relief = if steepness > 1e-3 { (agree / steepness) as f32 } else { RELIEF_SEEN };
        let relief = if relief.is_finite() { relief.clamp(RELIEF_SEEN_RANGE.0, RELIEF_SEEN_RANGE.1) } else { RELIEF_SEEN };
        for_rows(&mut surface.normal.data, w, |y, row| {
            for (x, n) in row.iter_mut().enumerate() {
                let model = model_at(x, y);
                // the slope of the depth, eased like an estimated relief's
                let [gx, gy] = slope_at(x, y).map(|g| g * relief);
                let ease = 1.0 / (1.0 + (gx * gx + gy * gy) / (MAX_SLOPE * MAX_SLOPE)).sqrt();
                let (gx, gy) = (gx * ease, gy * ease);
                let k = 1.0 / (gx * gx + gy * gy + 1.0).sqrt();
                let slope = [-gx * k, -gy * k, k];
                let joined: [f32; 3] = std::array::from_fn(|i| model[i] * (1.0 - DEPTH_SHARE) + slope[i] * DEPTH_SHARE);
                let len = dot(joined, joined).sqrt();
                *n = if len.is_finite() && len > 1e-6 { joined.map(|c| c / len) } else { [0.0, 0.0, 1.0] };
            }
        });
        surface.relief = relief;
        Some(surface)
    }

    /// A direction on the photo (`angle` degrees clockwise from its top) as a unit vector in the
    /// proxy's pixel grid, which may be flipped against the photo.
    fn direction(&self, angle: f32) -> [f32; 2] {
        let a = angle.to_radians();
        let screen = [a.sin(), -a.cos()];
        let long = self.ow.max(self.oh).max(1e-9);
        // a short step that way in normalized coordinates (which are not square), from the centre
        let step = Point::new(0.5 + 0.05 * screen[0] as f64 * long / self.ow.max(1e-9), 0.5 + 0.05 * screen[1] as f64 * long / self.oh.max(1e-9));
        let (from, to) = (self.norm_to_proxy.apply(Point::new(0.5, 0.5)), self.norm_to_proxy.apply(step));
        let (dx, dy) = ((to.x - from.x) as f32, (to.y - from.y) as f32);
        let len = (dx * dx + dy * dy).sqrt();
        if len.is_finite() && len > 1e-6 { [dx / len, dy / len] } else { screen }
    }
}

/// A light as the shading loop uses it, in long-edge units of the proxy.
#[derive(Clone, Copy, Debug)]
struct LightK {
    kind: RelightKind,
    /// Where the light is (Point, Spot).
    pos: [f32; 3],
    /// Unit vector towards a Directional light, or along a Spot's beam.
    dir: [f32; 3],
    /// Colour times intensity (linear Rec.2020).
    color: [f32; 3],
    /// Distance at which a Point or Spot light has faded to about half.
    reach: f32,
    /// Cosines of a Spot's outer and inner cone half-angles.
    cos_outer: f32,
    cos_inner: f32,
}

impl LightK {
    fn new(l: &RelightLight, surface: &Surface) -> LightK {
        let d = RelightLight::default();
        let p = surface.norm_to_proxy.apply(l.position);
        // a light dragged far off the photo is still a light, but not one a hostile number places at infinity
        let place = |v: f64| if v.is_finite() { (v as f32 / surface.long).clamp(-4.0, 5.0) } else { 0.5 };
        let depth = unit(l.depth, d.depth);
        let (x, y) = (place(p.x), place(p.y));
        // a lamp hangs in front of what is under its marker: Depth is how far in front
        let under = surface.relief * bilinear(&surface.height, x * surface.long, y * surface.long);
        let elevation = finite(l.elevation, d.elevation).clamp(0.0, 90.0).to_radians();
        let [dx, dy] = surface.direction(finite(l.angle, d.angle));
        let dir = match l.kind {
            // towards the light: up out of the photo by its elevation
            RelightKind::Directional => [dx * elevation.cos(), dy * elevation.cos(), elevation.sin()],
            // along the beam: down into the photo by its elevation
            RelightKind::Spot | RelightKind::Point => [dx * elevation.cos(), dy * elevation.cos(), -elevation.sin()],
        };
        let rgb = hsv_to_rgb(finite(l.hue, d.hue), unit(l.sat, d.sat), 1.0).map(srgb_to_linear);
        let tint = SRGB.to_space(&REC2020).apply_f32(rgb);
        let intensity = (finite(l.intensity, d.intensity) / 100.0).clamp(0.0, 2.0);
        let outer = (6.0 + 64.0 * unit(l.cone, d.cone)).to_radians();
        LightK {
            kind: l.kind,
            pos: [x, y, if under.is_finite() { under } else { 0.0 } + 0.02 + 0.5 * depth],
            dir,
            color: tint.map(|c| if c.is_finite() { c.max(0.0) * intensity } else { intensity }),
            reach: 0.05 + 1.2 * unit(l.falloff, d.falloff),
            cos_outer: outer.cos(),
            cos_inner: (outer * 0.7).cos(),
        }
    }
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The gain of every proxy pixel of `surface` under `r`.
fn shade(surface: &Surface, r: &Relight) -> Rgb32f {
    let d = Relight::default();
    let (w, h) = (surface.height.width, surface.height.height);
    let lights: Vec<LightK> = r.lights.iter().take(MAX_RELIGHT_LIGHTS).map(|l| LightK::new(l, surface)).collect();
    let ambient = (1.0 + finite(r.ambient, d.ambient) / 100.0).clamp(0.0, 2.0);
    let shine = unit(r.shine, d.shine);
    let penumbra = 0.02 + 0.25 * unit(r.shadow_softness, d.shadow_softness);
    let mut gain = Rgb32f::new(w, h);
    for_rows(&mut gain.data, w, |y, row| {
        for (x, out) in row.iter_mut().enumerate() {
            let n = surface.normal.get(x, y);
            let at = [(x as f32 + 0.5) / surface.long, (y as f32 + 0.5) / surface.long, surface.relief * surface.height.get(x, y)];
            let mut g = [ambient; 3];
            for light in &lights {
                // unit vector to the light, how far it is, and how much of it arrives
                let (to_light, distance, mut arrives) = match light.kind {
                    RelightKind::Directional => (light.dir, SHADOW_REACH, 1.0),
                    RelightKind::Point | RelightKind::Spot => {
                        let v = [light.pos[0] - at[0], light.pos[1] - at[1], light.pos[2] - at[2]];
                        let dist = dot(v, v).sqrt().max(1e-4);
                        let l = v.map(|c| c / dist);
                        // full strength right under the light, whatever its height
                        let (reach2, under) = (light.reach * light.reach, light.pos[2] * light.pos[2]);
                        (l, dist, ((reach2 + under) / (reach2 + dist * dist)).min(1.5))
                    }
                };
                if light.kind == RelightKind::Spot {
                    arrives *= smooth(light.cos_outer, light.cos_inner, -dot(to_light, light.dir));
                }
                let diffuse = dot(n, to_light).max(0.0);
                if arrives <= 1e-4 || diffuse <= 0.0 {
                    continue;
                }
                let mut lit = diffuse;
                if shine > 0.0 {
                    // Blinn–Phong towards a viewer straight in front of the photo
                    let half = [to_light[0], to_light[1], to_light[2] + 1.0];
                    let len = dot(half, half).sqrt().max(1e-6);
                    lit += shine * (dot(n, half) / len).max(0.0).powi(32);
                }
                if r.shadows {
                    lit *= shadow(surface, at, to_light, distance, penumbra);
                }
                for (c, k) in g.iter_mut().zip(light.color) {
                    *c += k * lit * arrives;
                }
            }
            *out = g.map(|c| if c.is_finite() { c.clamp(0.0, MAX_GAIN) } else { 1.0 });
        }
    });
    gain
}

/// How much of a light reaches `at` (1 = all, 0 = none): a ray marched from the point towards the
/// light through the relief, which shadows it where it rises above the ray. `penumbra` widens the
/// edge of the shadow with the distance from what casts it.
fn shadow(surface: &Surface, at: [f32; 3], to_light: [f32; 3], distance: f32, penumbra: f32) -> f32 {
    let mut blocked = 0.0f32;
    for i in 1..=SHADOW_STEPS {
        // steps crowd near the point, where what casts the shadow usually is
        let f = i as f32 / SHADOW_STEPS as f32;
        let t = distance * f * f.sqrt();
        let p = [at[0] + to_light[0] * t, at[1] + to_light[1] * t, at[2] + 0.004 + to_light[2] * t];
        if p[2] > surface.relief {
            break;
        }
        let (px, py) = (p[0] * surface.long, p[1] * surface.long);
        if px < 0.0 || py < 0.0 || px > surface.height.width as f32 || py > surface.height.height as f32 {
            break;
        }
        let above = surface.relief * bilinear(&surface.height, px, py) - p[2];
        if above > 0.0 {
            blocked = blocked.max((above / (penumbra * t + 0.004)).min(1.0));
            if blocked >= 1.0 {
                break;
            }
        }
    }
    1.0 - blocked
}

impl Field {
    /// The gain at proxy coordinates (`x`, `y`) for a pixel whose guide value is `v`: the four
    /// proxy pixels around it, each weighted by how near it is and how alike in luminance, so
    /// light does not spill over an edge the proxy is too coarse to hold.
    #[inline]
    fn gain_at(&self, x: f32, y: f32, v: f32) -> [f32; 3] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let (mut sum, mut total) = ([0.0f32; 3], 0.0f32);
        for (dx, dy, near) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
            let alike = (v - self.surface.guide.get_clamped(x0 + dx, y0 + dy)) / GUIDE_RANGE;
            let weight = near * (1e-3 + 1.0 / (1.0 + alike * alike));
            let g = self.gain.get_clamped(x0 + dx, y0 + dy);
            for (s, c) in sum.iter_mut().zip(g) {
                *s += weight * c;
            }
            total += weight;
        }
        if total > 1e-9 { sum.map(|s| s / total) } else { self.gain.get_clamped(x0, y0) }
    }
}

/// Light the image of `p` (scene-linear, the plan's output size) in place.
pub fn apply(img: &mut Rgb32f, p: &Plan<'_>) {
    let Some(field) = &p.relight else { return };
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let to_proxy = field.surface.norm_to_proxy * p.frame.out_to_norm(w, h);
    let mean = field.surface.mean;
    for_rows(&mut img.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let q = to_proxy.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
            if !(q.x.is_finite() && q.y.is_finite()) {
                continue;
            }
            let g = field.gain_at(q.x as f32, q.y as f32, guide_of(guide_luminance(*px), mean));
            for (c, k) in px.iter_mut().zip(g) {
                *c *= k;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PixelWindow, RenderRequest, render};
    use lightcraft_raster::Rgba8;

    const W: usize = 480;
    const H: usize = 320;

    fn scene() -> Rgb32f {
        lightcraft_scenes::demo_library()[0].render(480, 320)
    }

    fn shot(s: &DevelopSettings, w: usize, h: usize) -> Rgba8 {
        render(&scene(), &SourceInfo::default(), s, &RenderRequest::fit(w, h)).image
    }

    fn lit(lights: Vec<RelightLight>) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.relight.enabled = true;
        s.relight.lights = lights;
        s
    }

    fn point_at(x: f64, y: f64) -> RelightLight {
        RelightLight { kind: RelightKind::Point, position: Point::new(x, y), falloff: 15.0, depth: 15.0, ..Default::default() }
    }

    /// Mean of the three channels over a square of half-side `r` around (`cx`, `cy`).
    fn mean_around(img: &Rgba8, cx: usize, cy: usize, r: usize) -> f64 {
        let (mut sum, mut n) = (0.0, 0.0);
        for y in cy.saturating_sub(r)..(cy + r).min(img.height) {
            for x in cx.saturating_sub(r)..(cx + r).min(img.width) {
                let p = img.get(x, y);
                sum += p[0] as f64 + p[1] as f64 + p[2] as f64;
                n += 3.0;
            }
        }
        sum / n
    }

    fn mean_difference(a: &Rgba8, b: &Rgba8) -> f64 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        let sum: u64 = a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..3).map(move |k| p[k].abs_diff(q[k]) as u64)).sum();
        sum as f64 / (a.data.len() * 3) as f64
    }

    // Given Relight off, or on with nothing to add, the render is the one without it
    #[test]
    fn relight_that_adds_nothing_changes_nothing() {
        let plain = shot(&DevelopSettings::default(), W, H);
        let mut off = lit(vec![point_at(0.3, 0.4)]);
        off.relight.enabled = false;
        assert_eq!(shot(&off, W, H).data, plain.data, "switched off");
        assert_eq!(shot(&lit(vec![]), W, H).data, plain.data, "on, without a light");
        assert!(resolve(&scene(), &SourceInfo::default(), &off).is_none());
    }

    // Given a point light, the photo is brighter near it than without it, and far from it barely changes
    #[test]
    fn a_point_light_brightens_the_photo_around_it() {
        let plain = shot(&DevelopSettings::default(), W, H);
        let relit = shot(&lit(vec![point_at(0.25, 0.5)]), W, H);
        let (near, far) = ((W / 4, H / 2), (W * 9 / 10, H / 2));
        let gained = |at: (usize, usize)| mean_around(&relit, at.0, at.1, 20) - mean_around(&plain, at.0, at.1, 20);
        assert!(gained(near) > 12.0, "near the light: +{:.1}", gained(near));
        assert!(gained(near) > 4.0 * gained(far).max(0.0) + 4.0, "near +{:.1}, far +{:.1}", gained(near), gained(far));
        assert!(relit.data.iter().zip(&plain.data).all(|(a, b)| (0..3).all(|k| a[k] as i32 >= b[k] as i32 - 1)), "a light never darkens");
    }

    // Given the light is moved, the bright place moves with it
    #[test]
    fn the_light_is_where_it_was_placed() {
        let left = shot(&lit(vec![point_at(0.2, 0.5)]), W, H);
        let right = shot(&lit(vec![point_at(0.8, 0.5)]), W, H);
        assert!(mean_around(&left, W / 5, H / 2, 24) > mean_around(&right, W / 5, H / 2, 24) + 8.0);
        assert!(mean_around(&right, W * 4 / 5, H / 2, 24) > mean_around(&left, W * 4 / 5, H / 2, 24) + 8.0);
    }

    // Given Ambient −100 and no light, the photo's own light is gone; +100 brightens it
    #[test]
    fn ambient_scales_the_light_the_photo_had() {
        let plain = shot(&DevelopSettings::default(), W, H);
        let mut s = lit(vec![]);
        s.relight.ambient = -100.0;
        let dark = shot(&s, W, H);
        assert!(mean_around(&dark, W / 2, H / 2, 200) < 0.25 * mean_around(&plain, W / 2, H / 2, 200));
        s.relight.ambient = 100.0;
        assert!(mean_around(&shot(&s, W, H), W / 2, H / 2, 200) > mean_around(&plain, W / 2, H / 2, 200) + 5.0);
    }

    // Given a light's kind, colour, shine or the shadows change, so does the render
    #[test]
    fn every_setting_of_a_light_matters() {
        let base = lit(vec![RelightLight { kind: RelightKind::Spot, position: Point::new(0.4, 0.4), ..Default::default() }]);
        let reference = shot(&base, W, H);
        let changed = |name: &str, edit: &dyn Fn(&mut DevelopSettings)| {
            let mut s = base.clone();
            edit(&mut s);
            let d = mean_difference(&shot(&s, W, H), &reference);
            assert!(d > 0.05, "{name}: mean difference {d:.3}");
        };
        changed("kind", &|s| s.relight.lights[0].kind = RelightKind::Directional);
        changed("point", &|s| s.relight.lights[0].kind = RelightKind::Point);
        changed("intensity", &|s| s.relight.lights[0].intensity = 180.0);
        changed("colour", &|s| s.relight.lights[0].sat = 90.0);
        changed("angle", &|s| s.relight.lights[0].angle = 120.0);
        changed("elevation", &|s| s.relight.lights[0].elevation = 85.0);
        changed("depth", &|s| s.relight.lights[0].depth = 90.0);
        changed("falloff", &|s| s.relight.lights[0].falloff = 5.0);
        changed("cone", &|s| s.relight.lights[0].cone = 5.0);
        changed("softness", &|s| s.relight.softness = 100.0);
        changed("shine", &|s| s.relight.shine = 100.0);
        changed("shadows", &|s| {
            s.relight.lights[0] = RelightLight { kind: RelightKind::Directional, elevation: 12.0, ..Default::default() };
            s.relight.shadows = true;
        });
    }

    // Given cast shadows, they only ever take light away
    #[test]
    fn shadows_only_darken() {
        let mut s = lit(vec![RelightLight { kind: RelightKind::Directional, elevation: 10.0, ..Default::default() }]);
        let open = shot(&s, W, H);
        s.relight.shadows = true;
        let shadowed = shot(&s, W, H);
        assert!(shadowed.data.iter().zip(&open.data).all(|(a, b)| (0..3).all(|k| a[k] as i32 <= b[k] as i32 + 1)));
        assert!(mean_difference(&shadowed, &open) > 0.05);
        // a wider penumbra is a different shadow
        s.relight.shadow_softness = 0.0;
        assert!(mean_difference(&shot(&s, W, H), &shadowed) > 0.01);
    }

    // Given a small preview and a large render, they are lit alike (the lighting does not depend on the output size)
    #[test]
    fn a_preview_is_lit_like_a_large_render() {
        let s = lit(vec![point_at(0.3, 0.35), RelightLight { kind: RelightKind::Directional, sat: 60.0, ..Default::default() }]);
        let (small, large) = (shot(&s, 240, 160), shot(&s, 960, 640));
        for (fx, fy) in [(0.2, 0.3), (0.5, 0.5), (0.8, 0.7), (0.3, 0.8)] {
            let a = mean_around(&small, (240.0 * fx) as usize, (160.0 * fy) as usize, 8);
            let b = mean_around(&large, (960.0 * fx) as usize, (640.0 * fy) as usize, 32);
            assert!((a - b).abs() < 4.0, "at ({fx}, {fy}): small {a:.1}, large {b:.1}");
        }
    }

    // Given a window of the output, it is lit as the same pixels of the whole render are
    #[test]
    fn a_window_is_lit_like_the_whole_frame() {
        let mut s = lit(vec![point_at(0.45, 0.5), RelightLight { kind: RelightKind::Spot, position: Point::new(0.7, 0.3), ..Default::default() }]);
        s.relight.shadows = true;
        s.relight.shine = 40.0;
        let full = shot(&s, 960, 640);
        let win = PixelWindow { x: 300, y: 180, w: 400, h: 300 };
        let req = RenderRequest { window: Some(win), ..RenderRequest::fit(960, 640) };
        let part = render(&scene(), &SourceInfo::default(), &s, &req).image;
        assert_eq!((part.width, part.height), (win.w, win.h));
        let (mut max, mut sum, mut n) = (0u8, 0.0, 0.0);
        for y in 64..win.h - 64 {
            for x in 64..win.w - 64 {
                let (a, b) = (full.get(win.x + x, win.y + y), part.get(x, y));
                for k in 0..3 {
                    max = max.max(a[k].abs_diff(b[k]));
                    sum += a[k].abs_diff(b[k]) as f64;
                    n += 1.0;
                }
            }
        }
        assert!(max <= 3 && sum / n <= 0.3, "max {max}, mean {:.3}", sum / n);
    }

    // Given a crop, a light stays on the same part of the photo
    #[test]
    fn a_light_keeps_its_place_when_the_photo_is_cropped() {
        let mut s = lit(vec![point_at(0.6, 0.6)]);
        let whole = shot(&s, W, H);
        let plain_whole = shot(&DevelopSettings::default(), W, H);
        s.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.4, y0: 0.4, x1: 0.8, y1: 0.8 };
        let cropped = shot(&s, W, H);
        let plain_cropped = shot(&DevelopSettings { crop: s.crop, ..Default::default() }, W, H);
        // the light is at the middle of the crop: as much is gained there as at its place in the whole photo
        let gain_whole = mean_around(&whole, W * 6 / 10, H * 6 / 10, 6) - mean_around(&plain_whole, W * 6 / 10, H * 6 / 10, 6);
        let gain_crop = mean_around(&cropped, W / 2, H / 2, 15) - mean_around(&plain_cropped, W / 2, H / 2, 15);
        assert!(gain_whole > 8.0 && (gain_whole - gain_crop).abs() < 0.5 * gain_whole, "whole +{gain_whole:.1}, cropped +{gain_crop:.1}");
    }

    // Given a flipped photo, a directional light still comes from the side of the photo it was set to
    #[test]
    fn a_flip_takes_the_lighting_with_the_photo() {
        let mut s = lit(vec![RelightLight { kind: RelightKind::Directional, angle: 90.0, elevation: 20.0, ..Default::default() }]);
        let upright = shot(&s, W, H);
        s.crop.flip_h = true;
        let flipped = shot(&s, W, H);
        // the flipped render is the mirror image of the upright one: same photo, same light on it
        let mirrored = upright.flip_h();
        assert!(mean_difference(&flipped, &mirrored) < 1.0, "{:.3}", mean_difference(&flipped, &mirrored));
    }

    /// The geometry of a dome in the middle of the frame, in front of a flat, far wall.
    fn dome() -> std::sync::Arc<RelightGeometry> {
        let (w, h) = (96usize, 64usize);
        let samples: Vec<lightcraft_develop::ReliefSample> = (0..w * h)
            .map(|i| {
                // square in long-edge units, centred
                let (x, y) = (((i % w) as f32 + 0.5 - w as f32 / 2.0) / 24.0, ((i / w) as f32 + 0.5 - h as f32 / 2.0) / 24.0);
                let r2 = x * x + y * y;
                if r2 < 1.0 {
                    lightcraft_develop::ReliefSample { nx: x * 0.9, ny: y * 0.9, height: (1.0 - r2).sqrt() }
                } else {
                    lightcraft_develop::ReliefSample { nx: 0.0, ny: 0.0, height: 0.0 }
                }
            })
            .collect();
        std::sync::Arc::new(RelightGeometry::encode(w, h, &samples).unwrap())
    }

    /// A flat grey photo: nothing in it says anything about its shape.
    fn grey() -> Rgb32f {
        Rgb32f::filled(480, 320, [0.18, 0.18, 0.18])
    }

    /// Mean gain of the lighting over the proxy pixels inside normalized `[x0, x1] × [y0, y1]`.
    fn gain_in(s: &DevelopSettings, x0: f32, x1: f32, y0: f32, y1: f32) -> f32 {
        let field = resolve(&grey(), &SourceInfo::default(), s).unwrap();
        let (w, h) = (field.gain.width, field.gain.height);
        let (mut sum, mut n) = (0.0, 0.0);
        for y in (y0 * h as f32) as usize..(y1 * h as f32) as usize {
            for x in (x0 * w as f32) as usize..(x1 * w as f32) as usize {
                sum += field.gain.get(x, y)[1];
                n += 1.0;
            }
        }
        sum / n
    }

    // Given an analyzed scene, a light from the left lights the side of a dome that faces left, not the other
    #[test]
    fn stored_geometry_is_lit_by_its_normals() {
        let mut s = lit(vec![RelightLight { kind: RelightKind::Directional, angle: 270.0, elevation: 15.0, ..Default::default() }]);
        s.relight.softness = 0.0;
        s.relight.geometry = Some(dome());
        // the dome spans x 0.25..0.75 of the frame
        let (left, right, wall) = (gain_in(&s, 0.3, 0.42, 0.4, 0.6), gain_in(&s, 0.58, 0.7, 0.4, 0.6), gain_in(&s, 0.02, 0.12, 0.1, 0.9));
        assert!(left > right + 0.4, "left side {left:.2}, right side {right:.2}");
        assert!(left > wall + 0.2 && wall > right, "left {left:.2}, wall {wall:.2}, right {right:.2}");
        // from the right, the other way round
        s.relight.lights[0].angle = 90.0;
        assert!(gain_in(&s, 0.58, 0.7, 0.4, 0.6) > gain_in(&s, 0.3, 0.42, 0.4, 0.6) + 0.4);
        // and from above, the top
        s.relight.lights[0].angle = 0.0;
        assert!(gain_in(&s, 0.45, 0.55, 0.25, 0.4) > gain_in(&s, 0.45, 0.55, 0.6, 0.75) + 0.4);
    }

    // Given an analyzed scene with cast shadows, the dome shadows the wall behind it, away from the light
    #[test]
    fn stored_geometry_casts_shadows_from_its_depth() {
        let mut s = lit(vec![RelightLight { kind: RelightKind::Directional, angle: 270.0, elevation: 20.0, ..Default::default() }]);
        s.relight.softness = 0.0;
        s.relight.geometry = Some(dome());
        let open = gain_in(&s, 0.78, 0.86, 0.42, 0.58);
        s.relight.shadows = true;
        s.relight.shadow_softness = 0.0;
        let shadowed = gain_in(&s, 0.78, 0.86, 0.42, 0.58);
        assert!(shadowed < open - 0.1, "right of the dome: {open:.2} without shadows, {shadowed:.2} with");
        // the wall on the light's side stays lit
        let lit_side = gain_in(&s, 0.08, 0.18, 0.42, 0.58);
        assert!(lit_side > shadowed + 0.1, "lit side {lit_side:.2}, shadow {shadowed:.2}");
    }

    // Given a flipped photo, the analyzed scene flips with it
    #[test]
    fn stored_geometry_follows_a_flip() {
        let mut s = lit(vec![RelightLight { kind: RelightKind::Directional, angle: 270.0, elevation: 15.0, ..Default::default() }]);
        s.relight.softness = 0.0;
        s.relight.geometry = Some(dome());
        let upright = (gain_in(&s, 0.3, 0.42, 0.4, 0.6), gain_in(&s, 0.58, 0.7, 0.4, 0.6));
        s.crop.flip_h = true;
        // the light comes from the photo's left, which is now drawn on the right: so is the lit side
        let flipped = (gain_in(&s, 0.3, 0.42, 0.4, 0.6), gain_in(&s, 0.58, 0.7, 0.4, 0.6));
        assert!((upright.0 - flipped.1).abs() < 0.05 && (upright.1 - flipped.0).abs() < 0.05, "{upright:?} against {flipped:?}");
    }

    // Given geometry that can't be read, the relief is estimated as if there were none
    #[test]
    fn damaged_geometry_falls_back_to_the_estimate() {
        let mut s = lit(vec![point_at(0.3, 0.4)]);
        let estimated = shot(&s, W, H);
        for (width, height, data) in
            [(96, 64, "not base64 at all !!".to_string()), (0, 0, String::new()), (u32::MAX, 7, dome().data.clone()), (95, 64, dome().data.clone())]
        {
            s.relight.geometry = Some(std::sync::Arc::new(RelightGeometry { width, height, data }));
            assert_eq!(shot(&s, W, H).data, estimated.data, "{width} × {height}");
        }
        s.relight.geometry = Some(dome());
        assert!(mean_difference(&shot(&s, W, H), &estimated) > 0.05, "readable geometry is used");
    }

    // Given settings no slider could produce, the render neither panics nor leaves the picture unusable
    #[test]
    fn hostile_numbers_render_a_finite_picture() {
        let wild = |v: f64| RelightLight {
            kind: RelightKind::Spot,
            position: Point::new(v, -v),
            intensity: v,
            hue: v,
            sat: v,
            angle: v,
            elevation: v,
            depth: v,
            falloff: v,
            cone: v,
        };
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300, 0.0] {
            let mut s = lit((0..40).map(|_| wild(v)).collect());
            s.relight.ambient = v;
            s.relight.softness = v;
            s.relight.shine = v;
            s.relight.shadow_softness = v;
            s.relight.shadows = true;
            let image = shot(&s, 120, 80);
            assert_eq!((image.width, image.height), (120, 80), "{v}");
            let field = resolve(&scene(), &SourceInfo::default(), &s);
            assert!(field.is_none_or(|f| f.gain.data.iter().flatten().all(|g| g.is_finite() && (0.0..=MAX_GAIN).contains(g))), "{v}");
        }
        // a source of one pixel, and an empty one
        let dot = Rgb32f::filled(1, 1, [0.2, 0.2, 0.2]);
        assert!(resolve(&dot, &SourceInfo::default(), &lit(vec![point_at(0.5, 0.5)])).is_some());
        assert!(resolve(&Rgb32f::new(0, 0), &SourceInfo::default(), &lit(vec![point_at(0.5, 0.5)])).is_none());
    }
}
