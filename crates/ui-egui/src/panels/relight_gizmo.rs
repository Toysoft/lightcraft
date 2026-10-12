//! The little three-dimensional figures that show a light's direction on the photo and in the
//! Relight panel: a Directional light is a disc with parallel rays leaving it, a Spot a cone.
//! They are solid shapes turned in space by the light's own numbers (which way it points on the
//! photo, how steeply, how wide) and drawn flat, as seen from in front of the photo, so turning a
//! slider turns the figure: a Directional light head-on is a full disc with its rays end-on, at a
//! grazing angle a thin ellipse with long rays; a Spot pointing straight into the photo is seen
//! down its cone, as a circle around its tip.
//!
//! Original work of the LightCraft contributors: drawn in code, traced from nothing.

use egui::{Color32, Painter, Pos2, Stroke, Vec2, pos2, vec2};
use lightcraft_develop::RelightKind;

/// Segments of a drawn circle.
const SEGMENTS: usize = 40;
/// Narrowest and widest half-angle of a Spot's cone, in degrees (as the renderer has them).
const CONE: (f32, f32) = (6.0, 70.0);

/// A point in the figure's space: x and y on the screen, z towards the viewer.
type P3 = [f32; 3];

fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: P3, k: f32) -> P3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(a: P3) -> P3 {
    let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if len.is_finite() && len > 1e-6 { scale(a, 1.0 / len) } else { [0.0, 0.0, 1.0] }
}

/// A finite number, else `default`.
fn finite(v: f64, default: f32) -> f32 {
    if v.is_finite() { v.clamp(-1e6, 1e6) as f32 } else { default }
}

/// The figure of a light, as polylines around the origin (screen points, y down), to be moved to
/// the light's marker. `open` ones are drawn as they are, the others closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gizmo {
    pub lines: Vec<(Vec<Vec2>, bool)>,
}

/// A circle of radius `r` around `centre`, in the plane across `axis`, seen from the front.
fn circle(centre: P3, axis: P3, r: f32) -> Vec<Vec2> {
    // two directions across the axis (any, when the axis points at the viewer)
    let across = cross(axis, [0.0, 0.0, 1.0]);
    let u = if across[0].hypot(across[1]) > 1e-4 { unit(across) } else { [1.0, 0.0, 0.0] };
    let v = unit(cross(axis, u));
    (0..SEGMENTS)
        .map(|i| {
            let t = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let p = add(centre, add(scale(u, r * t.cos()), scale(v, r * t.sin())));
            vec2(p[0], p[1])
        })
        .collect()
}

/// An arrow from `from` to `to` with a head, as open polylines.
fn arrow(from: Vec2, to: Vec2, head: f32) -> Vec<(Vec<Vec2>, bool)> {
    let mut out = vec![(vec![from, to], false)];
    let along = to - from;
    if along.length() > head * 0.8 {
        let dir = along.normalized();
        let side = dir.rot90() * head * 0.5;
        out.push((vec![to - dir * head + side, to, to - dir * head - side], false));
    }
    out
}

impl Gizmo {
    /// The figure of a light of `kind`. `on_screen` is the way it points on the screen (for a
    /// Directional light: the way its light travels), `elevation` its degrees of tilt into the
    /// photo (0 = along the photo, 90 = straight into it, beyond that over to the other side;
    /// negative: out of the photo, towards the viewer), `cone` a Spot's width 0..100, `size` the
    /// figure's length in points. A Point light has no figure.
    pub fn new(kind: RelightKind, on_screen: Vec2, elevation: f64, cone: f64, size: f32) -> Gizmo {
        let size = if size.is_finite() { size.clamp(4.0, 400.0) } else { 40.0 };
        let dir =
            if on_screen.x.is_finite() && on_screen.y.is_finite() && on_screen.length() > 1e-4 { on_screen.normalized() } else { vec2(0.0, 1.0) };
        let tilt = finite(elevation, 45.0).clamp(-180.0, 180.0).to_radians();
        // the way the light travels: along the screen, and away from the viewer by its tilt
        let axis: P3 = unit([dir.x * tilt.cos(), dir.y * tilt.cos(), -tilt.sin()]);
        let flat = |p: P3| vec2(p[0], p[1]);
        let mut lines = Vec::new();
        match kind {
            RelightKind::Point => {}
            RelightKind::Directional => {
                // a disc the light leaves, and rays from its centre and rim
                let r = size * 0.36;
                let rim = circle([0.0; 3], axis, r);
                let travel = flat(scale(axis, size));
                lines.push((rim.clone(), true));
                lines.extend(arrow(Vec2::ZERO, travel, size * 0.18));
                for k in 0..4 {
                    if let Some(start) = rim.get(k * SEGMENTS / 4 + SEGMENTS / 8) {
                        lines.extend(arrow(*start, *start + travel * 0.7, size * 0.14));
                    }
                }
            }
            RelightKind::Spot => {
                // a cone from the lamp: its mouth, its two edges as seen, and its axis
                let half = (CONE.0 + (CONE.1 - CONE.0) * (finite(cone, 40.0) / 100.0).clamp(0.0, 1.0)).to_radians();
                // (the cone is as long as it can be without its mouth outgrowing the figure)
                let length = size * half.cos().max(0.35);
                let r = (length * half.tan()).min(size);
                let mouth_centre = scale(axis, length);
                let mouth = circle(mouth_centre, axis, r);
                // the edges touch the mouth where it is widest across the way it points on the screen
                let centre = flat(mouth_centre);
                let across = if centre.length() > 1e-3 { centre.normalized().rot90() } else { vec2(1.0, 0.0) };
                let far = |sign: f32| {
                    mouth
                        .iter()
                        .copied()
                        .max_by(|a, b| ((*a - centre).dot(across) * sign).total_cmp(&((*b - centre).dot(across) * sign)))
                        .unwrap_or(centre)
                };
                lines.push((mouth.clone(), true));
                // seen from behind its tip, a cone has no edges: only its mouth around it
                if centre.length() > r * 0.35 {
                    lines.push((vec![far(1.0), Vec2::ZERO, far(-1.0)], false));
                }
                lines.extend(arrow(Vec2::ZERO, centre, size * 0.16));
            }
        }
        Gizmo { lines }
    }

    /// The farthest the figure reaches from its origin.
    pub fn reach(&self) -> f32 {
        self.lines.iter().flat_map(|(points, _)| points).map(|p| p.length()).fold(0.0, f32::max)
    }

    /// Draw the figure at `at`: a dark edge under a line of `color`, so it reads on any photo.
    pub fn paint(&self, painter: &Painter, at: Pos2, color: Color32, width: f32) {
        for stroke in [Stroke::new(width + 2.0, Color32::from_black_alpha(120)), Stroke::new(width, color)] {
            for (points, closed) in &self.lines {
                let moved: Vec<Pos2> = points.iter().map(|p| pos2(at.x + p.x, at.y + p.y)).filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
                if moved.len() < 2 {
                    continue;
                }
                painter.add(if *closed { egui::Shape::closed_line(moved, stroke) } else { egui::Shape::line(moved, stroke) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Width and height of a polyline's bounding box.
    fn extent(points: &[Vec2]) -> Vec2 {
        let (lo, hi) = points.iter().fold((vec2(f32::MAX, f32::MAX), vec2(f32::MIN, f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        hi - lo
    }

    fn directional(elevation: f64) -> Gizmo {
        Gizmo::new(RelightKind::Directional, vec2(1.0, 0.0), elevation, 0.0, 50.0)
    }

    fn spot(elevation: f64, cone: f64) -> Gizmo {
        Gizmo::new(RelightKind::Spot, vec2(1.0, 0.0), elevation, cone, 50.0)
    }

    // A Directional light head-on is a full disc with its rays end-on; at a grazing angle a thin ellipse with long rays
    #[test]
    fn a_directional_light_turns_with_its_elevation() {
        let (head_on, grazing, between) = (directional(90.0), directional(0.0), directional(45.0));
        let disc = |g: &Gizmo| extent(&g.lines[0].0);
        assert!((disc(&head_on).x - disc(&head_on).y).abs() < 0.5 && disc(&head_on).x > 30.0, "a circle: {:?}", disc(&head_on));
        assert!(disc(&grazing).x < 0.5 && disc(&grazing).y > 30.0, "seen edge-on: {:?}", disc(&grazing));
        assert!(disc(&between).x > 5.0 && disc(&between).x < disc(&between).y - 5.0, "an ellipse: {:?}", disc(&between));
        // the central ray: its length on screen shrinks as the light turns into the photo
        let ray = |g: &Gizmo| g.lines[1].0[1].length();
        assert!(ray(&grazing) > 49.0 && ray(&head_on) < 0.5 && (ray(&between) - 50.0 * 45f32.to_radians().cos()).abs() < 0.5);
        assert!(ray(&grazing) > ray(&between) && ray(&between) > ray(&head_on));
    }

    // The figure points the way the light does on the screen
    #[test]
    fn the_figure_points_where_the_light_points() {
        for (dir, name) in [(vec2(1.0, 0.0), "right"), (vec2(0.0, 1.0), "down"), (vec2(-0.6, -0.8), "up and left")] {
            for kind in [RelightKind::Directional, RelightKind::Spot] {
                let g = Gizmo::new(kind, dir, 30.0, 40.0, 50.0);
                // the last arrow drawn is the axis (Spot) or a ray (Directional): it runs along `dir`
                let (shaft, _) = g.lines.iter().rev().find(|(points, closed)| points.len() == 2 && !closed).unwrap();
                let along = (shaft[1] - shaft[0]).normalized();
                assert!(along.dot(dir.normalized()) > 0.99, "{kind:?} {name}: {along:?}");
            }
        }
    }

    // A Spot's cone opens with Cone, and pointing straight into the photo it is seen down its length
    #[test]
    fn a_spot_is_a_cone_that_opens_and_turns() {
        let mouth = |g: &Gizmo| extent(&g.lines[0].0);
        let (narrow, wide) = (spot(20.0, 0.0), spot(20.0, 100.0));
        assert!(mouth(&wide).y > mouth(&narrow).y * 3.0, "narrow {:?}, wide {:?}", mouth(&narrow), mouth(&wide));
        assert_eq!(
            narrow.lines.iter().filter(|(points, closed)| points.len() == 3 && !closed && points[1] == Vec2::ZERO).count(),
            1,
            "its two edges meet at the lamp"
        );
        // straight into the photo: the mouth is a circle around the lamp, and there are no edges to see
        let down = spot(90.0, 40.0);
        assert!((mouth(&down).x - mouth(&down).y).abs() < 0.5);
        let centre = down.lines[0].0.iter().fold(Vec2::ZERO, |a, p| a + *p) / down.lines[0].0.len() as f32;
        assert!(centre.length() < 0.5, "{centre:?}");
        assert!(!down.lines.iter().any(|(points, closed)| points.len() == 3 && !closed && points[1] == Vec2::ZERO));
        // along the photo: seen from the side, the mouth is a line across the way it points
        assert!(mouth(&spot(0.0, 40.0)).x < 0.5);
    }

    // Elevation is a full turn: past 90° the figure comes over to the other side, below 0° it points out at the viewer
    #[test]
    fn elevation_turns_the_figure_all_the_way_round() {
        // the central ray of a Directional light pointing right on the screen
        let ray = |elevation: f64| directional(elevation).lines[1].0[1];
        assert!(ray(30.0).x > 40.0 && ray(150.0).x < -40.0, "over the top it points the other way: {:?} {:?}", ray(30.0), ray(150.0));
        assert!((ray(30.0).x - ray(-30.0).x).abs() < 0.01, "seen from the front, into and out of the photo look alike in length");
        assert!((ray(180.0).x + 50.0).abs() < 0.5 && (ray(-180.0).x + 50.0).abs() < 0.5);
        // … but not in shape: the disc's near and far halves swap, so the two are not the same figure
        assert_ne!(directional(30.0), directional(-30.0));
        assert_ne!(spot(40.0, 40.0), spot(-40.0, 40.0));
        for elevation in [-180.0, -135.0, -90.0, -45.0, 0.0, 135.0, 180.0] {
            for g in [directional(elevation), spot(elevation, 40.0)] {
                assert!(
                    g.reach() > 4.0 && g.reach() <= 50.0 * 1.45 && g.lines.iter().flat_map(|(p, _)| p).all(|p| p.x.is_finite() && p.y.is_finite()),
                    "{elevation}"
                );
            }
        }
    }

    // The figure stays inside its size, and a Point light has none
    #[test]
    fn the_figure_fits_its_size() {
        for elevation in [0.0, 15.0, 45.0, 80.0, 90.0] {
            for cone in [0.0, 40.0, 100.0] {
                for kind in [RelightKind::Directional, RelightKind::Spot] {
                    let reach = Gizmo::new(kind, vec2(0.3, -1.0), elevation, cone, 50.0).reach();
                    assert!(reach > 4.0 && reach <= 50.0 * 1.45, "{kind:?} at {elevation}° cone {cone}: {reach}");
                }
            }
        }
        assert!(Gizmo::new(RelightKind::Point, vec2(1.0, 0.0), 45.0, 40.0, 50.0).lines.is_empty());
    }

    // Numbers no slider gives make a figure all the same, never a panic or a point at infinity
    #[test]
    fn hostile_numbers_make_a_finite_figure() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300] {
            for dir in [vec2(f32::NAN, 0.0), vec2(0.0, 0.0), vec2(f32::INFINITY, 1.0)] {
                for kind in [RelightKind::Directional, RelightKind::Spot] {
                    let g = Gizmo::new(kind, dir, v, v, v as f32);
                    assert!(
                        !g.lines.is_empty() && g.lines.iter().flat_map(|(points, _)| points).all(|p| p.x.is_finite() && p.y.is_finite()),
                        "{kind:?} {v} {dir:?}"
                    );
                }
            }
        }
    }
}
