//! Vector icons drawn in code on a 20×20 design grid (1.5 px strokes, rounded caps).
//! Most are our own work. The arms that call `pen.svg` stroke path data adapted from Lucide
//! (https://lucide.dev, ISC, `assets/licenses/ISC-Lucide.txt`), each commented with the Lucide icon name.
//! See assets/ATTRIBUTION.md. Never trace icons from Adobe (or any non-permissive) products.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Sidebar,
    Back,
    Forward,
    Search,
    Filter,
    Bell,
    Share,
    Help,
    Cloud,
    Sliders,
    Crop,
    Eraser,
    Mask,
    Eye,
    /// Eye with a slash (hidden).
    EyeOff,
    Presets,
    Versions,
    Activity,
    More,
    Tag,
    Info,
    GridPhoto,
    GridSquare,
    Single,
    Compare,
    Survey,
    Sort,
    Star,
    StarFilled,
    Circle,
    FlagPick,
    FlagReject,
    Gear,
    Filmstrip,
    BeforeAfter,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    ChevronUp,
    ChevronDoubleLeft,
    ChevronDoubleRight,
    Plus,
    Minus,
    Album,
    SmartAlbum,
    Stack,
    Folder,
    /// Edit (a pencil).
    Pencil,
    Photos,
    Clock,
    Calendar,
    Trash,
    Curve,
    ProfileGrid,
    Close,
    Check,
    Brush,
    Linear,
    Radial,
    Sky,
    Subject,
    Picker,
    Rotate,
    /// A curved line with an arrow at both ends: drag to rotate, either way (the crop tool's pointer).
    RotateDrag,
    Flip,
    Invert,
    Dots,
    Video,
    Heart,
    Edited,
    /// Targeted adjustment: a ring with a centre dot and up/down arrows (drag vertically).
    Target,
    /// Community chat (opens the ArtCraft Discord): a speech bubble with three dots. Our own
    /// generic drawing, not any service's logo.
    Chat,
    /// Face boxes on/off: four corner brackets round a small face.
    FaceBox,
    /// Play (a triangle pointing right).
    Play,
    /// Pause (two bars).
    Pause,
    /// Colour grading, all three wheels: three small circles.
    GradeAll,
    /// Colour grading, shadows wheel: a filled circle.
    GradeShadows,
    /// Colour grading, midtones wheel: a half-filled circle.
    GradeMidtones,
    /// Colour grading, highlights wheel: an open circle with rays.
    GradeHighlights,
    /// Appearance Mode ▸ Auto (follow the system): a monitor on its stand.
    Monitor,
    /// Appearance Mode ▸ Light: a sun (a disc with eight rays).
    Sun,
    /// Appearance Mode ▸ Dark: a crescent moon.
    Moon,
    /// Red Eye removal: an eye with a solid pupil (the plain [`Icon::Eye`] means "visible").
    RedEye,
    /// Color Mixer: a painter's palette.
    Palette,
    /// Object mask: a pointer clicking into a dashed selection.
    Object,
    /// Describe (text-prompt) mask: a speech bubble with lines of text.
    Describe,
    /// Background mask: a person in front of a ridge, in a frame.
    Background,
    /// People mask: two people.
    People,
    /// Landscape mask: a snow-capped mountain.
    Landscape,
    /// Luminance Range mask: a disc, half light and half dark.
    Luminance,
    /// Depth Range mask: stacked layers, near to far.
    Depth,
}

impl Icon {
    /// Every icon, in declaration order (contact sheets, tests).
    pub const ALL: [Icon; 87] = [
        Icon::Sidebar,
        Icon::Back,
        Icon::Forward,
        Icon::Search,
        Icon::Filter,
        Icon::Bell,
        Icon::Share,
        Icon::Help,
        Icon::Cloud,
        Icon::Sliders,
        Icon::Crop,
        Icon::Eraser,
        Icon::Mask,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Presets,
        Icon::Versions,
        Icon::Activity,
        Icon::More,
        Icon::Tag,
        Icon::Info,
        Icon::GridPhoto,
        Icon::GridSquare,
        Icon::Single,
        Icon::Compare,
        Icon::Survey,
        Icon::Sort,
        Icon::Star,
        Icon::StarFilled,
        Icon::Circle,
        Icon::FlagPick,
        Icon::FlagReject,
        Icon::Gear,
        Icon::Filmstrip,
        Icon::BeforeAfter,
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::ChevronLeft,
        Icon::TriangleLeft,
        Icon::Plus,
        Icon::Minus,
        Icon::Album,
        Icon::SmartAlbum,
        Icon::Stack,
        Icon::Folder,
        Icon::Pencil,
        Icon::Photos,
        Icon::Clock,
        Icon::Calendar,
        Icon::Trash,
        Icon::Curve,
        Icon::ProfileGrid,
        Icon::Close,
        Icon::Check,
        Icon::Brush,
        Icon::Linear,
        Icon::Radial,
        Icon::Sky,
        Icon::Subject,
        Icon::Picker,
        Icon::Rotate,
        Icon::RotateDrag,
        Icon::Flip,
        Icon::Invert,
        Icon::Dots,
        Icon::Video,
        Icon::Heart,
        Icon::Edited,
        Icon::Target,
        Icon::Chat,
        Icon::FaceBox,
        Icon::GradeAll,
        Icon::GradeShadows,
        Icon::GradeMidtones,
        Icon::GradeHighlights,
        Icon::Monitor,
        Icon::Sun,
        Icon::Moon,
        Icon::RedEye,
        Icon::Palette,
        Icon::Object,
        Icon::Describe,
        Icon::Background,
        Icon::People,
        Icon::Landscape,
        Icon::Luminance,
        Icon::Depth,
    ];
}

struct Pen<'a> {
    p: &'a Painter,
    r: Rect,
    s: Stroke,
    k: f32,
    /// (cos, sin) of a turn about the grid's centre, clockwise on screen ([`paint_turned`]).
    turn: Option<(f32, f32)>,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        let (x, y) = match self.turn {
            Some((c, s)) => (10.0 + (x - 10.0) * c - (y - 10.0) * s, 10.0 + (x - 10.0) * s + (y - 10.0) * c),
            None => (x, y),
        };
        pos2(self.r.min.x + x * self.k, self.r.min.y + y * self.k)
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::line(v, self.s));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::closed_line(v, self.s));
    }
    fn fill(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::convex_polygon(v, c, Stroke::NONE));
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.p.circle_stroke(self.pt(x, y), r * self.k, self.s);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.p.circle_filled(self.pt(x, y), r * self.k, self.s.color);
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32) {
        self.p.rect_stroke(Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1)), round * self.k, self.s, egui::StrokeKind::Middle);
    }
    fn rect_fill(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32) {
        self.p.rect_filled(Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1)), round * self.k, self.s.color);
    }
    /// Stroke SVG path data (`d` attributes) drawn on a 24×24 grid, scaled onto ours. Open subpaths get round
    /// caps, near-zero-length ones (`h.01`) become dots. Malformed data draws what parsed before the error.
    fn svg(&self, paths: &[&str]) {
        let k24 = 20.0 / 24.0;
        let cap = self.s.width / 2.0 / self.k;
        for d in paths {
            for (pts, closed) in svg_subpaths(d) {
                let pts: Vec<(f32, f32)> = pts.iter().map(|(x, y)| (x * k24, y * k24)).collect();
                let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { continue };
                let (lo, hi) = pts
                    .iter()
                    .fold(((f32::MAX, f32::MAX), (f32::MIN, f32::MIN)), |(lo, hi), &(x, y)| ((lo.0.min(x), lo.1.min(y)), (hi.0.max(x), hi.1.max(y))));
                if hi.0 - lo.0 < 0.2 && hi.1 - lo.1 < 0.2 {
                    self.dot(first.0, first.1, cap * 1.15);
                } else if closed {
                    let n = if pts.len() > 2 && (first.0 - last.0).hypot(first.1 - last.1) < 1e-3 { pts.len() - 1 } else { pts.len() };
                    self.closed(pts.get(..n).unwrap_or(&pts));
                } else {
                    self.line(&pts);
                    self.dot(first.0, first.1, cap);
                    self.dot(last.0, last.1, cap);
                }
            }
        }
    }
    /// Fill a polygon that is star-shaped about `centre` (every vertex visible from it) as one fan mesh, so no
    /// anti-aliasing seams show between its triangles, then stroke the outline for a smooth edge.
    fn fill_star_shaped(&self, centre: (f32, f32), pts: &[(f32, f32)]) {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(self.pt(centre.0, centre.1), self.s.color);
        for (x, y) in pts {
            mesh.colored_vertex(self.pt(*x, *y), self.s.color);
        }
        let n = pts.len() as u32;
        for i in 0..n {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
        }
        self.p.add(Shape::mesh(mesh));
        self.closed(pts);
    }
    /// A circle drawn as `dashes` equal dashes with equal gaps.
    fn dashed_circle(&self, cx: f32, cy: f32, r: f32, dashes: u32) {
        let step = 360.0 / dashes.max(1) as f32;
        for i in 0..dashes {
            let a = i as f32 * step - 90.0 + step / 4.0;
            self.arc(cx, cy, r, a, a + step / 2.0);
        }
    }
    fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let n = 24;
        let v: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.line(&v);
    }
}

fn star_pts(cx: f32, cy: f32, r: f32) -> Vec<(f32, f32)> {
    (0..10)
        .map(|i| {
            let a = (-90.0 + i as f32 * 36.0f32).to_radians();
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            (cx + rr * a.cos(), cy + rr * a.sin())
        })
        .collect()
}

/// Paint `icon` centred in `rect` (square area used) with `color`.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    paint_with(p, rect, icon, color, None);
}

/// [`paint`], turned `radians` clockwise about its centre. Only for icons drawn with lines, arcs
/// and circles (rectangles stay square to the screen).
pub fn paint_turned(p: &Painter, rect: Rect, icon: Icon, color: Color32, radians: f32) {
    paint_with(p, rect, icon, color, Some((radians.cos(), radians.sin())));
}

fn paint_with(p: &Painter, rect: Rect, icon: Icon, color: Color32, turn: Option<(f32, f32)>) {
    let side = rect.width().min(rect.height());
    // All disclosure and navigation chevrons share a 16-point size and stroke.
    let side = if matches!(
        icon,
        Icon::Back
            | Icon::Forward
            | Icon::ChevronDown
            | Icon::ChevronUp
            | Icon::ChevronLeft
            | Icon::ChevronRight
            | Icon::ChevronDoubleLeft
            | Icon::ChevronDoubleRight
    ) {
        side.min(16.0)
    } else {
        side
    };
    let r = Rect::from_center_size(rect.center(), vec2(side, side));
    let k = side / 20.0;
    let pen = Pen { p, r, s: Stroke::new((1.5 * k).max(1.0), color), k, turn };
    use Icon::*;
    match icon {
        Sidebar => {
            pen.rect(3.0, 4.0, 17.0, 16.0, 1.5);
            pen.line(&[(8.0, 4.0), (8.0, 16.0)]);
        }
        Monitor => {
            pen.rect(2.5, 3.5, 17.5, 13.5, 1.5);
            pen.line(&[(10.0, 13.5), (10.0, 16.5)]);
            pen.line(&[(6.5, 16.5), (13.5, 16.5)]);
        }
        Sun => {
            pen.circle(10.0, 10.0, 3.2);
            for i in 0..8 {
                let a = (i as f32 * 45.0).to_radians();
                let (c, s) = (a.cos(), a.sin());
                pen.line(&[(10.0 + 5.4 * c, 10.0 + 5.4 * s), (10.0 + 7.4 * c, 10.0 + 7.4 * s)]);
            }
        }
        Moon => {
            // outer arc of the disc, then the inner arc of the bite, meeting at the horns
            pen.arc(10.0, 10.0, 6.5, 2.9, 263.8);
            pen.arc(13.4, 6.4, 5.0, 214.9, 51.8);
        }
        Search => {
            pen.circle(8.5, 8.5, 5.0);
            pen.line(&[(12.2, 12.2), (16.5, 16.5)]);
        }
        Filter => pen.closed(&[(3.0, 4.0), (17.0, 4.0), (11.5, 10.5), (11.5, 16.0), (8.5, 14.5), (8.5, 10.5)]),
        Bell => {
            // Lucide `bell`
            pen.svg(&["M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9", "M10.3 21a1.94 1.94 0 0 0 3.4 0"]);
        }
        Share => {
            pen.line(&[(6.0, 8.0), (4.0, 8.0), (4.0, 17.0), (16.0, 17.0), (16.0, 8.0), (14.0, 8.0)]);
            pen.line(&[(10.0, 12.0), (10.0, 2.5)]);
            pen.line(&[(6.5, 6.0), (10.0, 2.5), (13.5, 6.0)]);
        }
        Help => {
            pen.circle(10.0, 10.0, 7.5);
            pen.arc(10.0, 8.0, 2.5, 180.0, 400.0);
            pen.line(&[(10.0, 10.5), (10.0, 11.8)]);
            pen.dot(10.0, 14.2, 0.9);
        }
        Cloud => {
            // Lucide `cloud`
            pen.svg(&["M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"]);
        }
        Sliders => {
            for (y, x) in [(5.0, 12.0), (10.0, 7.0), (15.0, 13.0)] {
                pen.line(&[(3.0, y), (x - 2.0, y)]);
                pen.line(&[(x + 2.0, y), (17.0, y)]);
                pen.circle(x, y, 2.0);
            }
        }
        Crop => {
            // Lucide `crop`
            pen.svg(&["M6 2v14a2 2 0 0 0 2 2h14", "M18 22V8a2 2 0 0 0-2-2H2"]);
        }
        Eraser => {
            // Lucide `eraser`
            pen.svg(&["m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21", "M22 21H7", "m5 11 9 9"]);
        }
        Mask => {
            // a solid region inside a dashed selection ring
            pen.dashed_circle(10.0, 10.0, 7.5, 10);
            pen.dot(10.0, 10.0, 3.6);
        }
        Eye => {
            // Lucide `eye`
            pen.svg(&["M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"]);
            pen.circle(10.0, 10.0, 2.5);
        }
        GradeAll => {
            pen.circle(10.0, 6.0, 3.2);
            pen.circle(5.5, 13.5, 3.2);
            pen.circle(14.5, 13.5, 3.2);
        }
        GradeShadows => {
            pen.circle(10.0, 10.0, 6.0);
            pen.dot(10.0, 10.0, 6.0);
        }
        GradeMidtones => {
            pen.circle(10.0, 10.0, 6.0);
            let half: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let a = (90.0 + 180.0 * i as f32 / 12.0).to_radians();
                    (10.0 + 6.0 * a.cos(), 10.0 + 6.0 * a.sin())
                })
                .collect();
            pen.fill(&half, pen.s.color);
        }
        GradeHighlights => {
            pen.circle(10.0, 10.0, 4.0);
            for i in 0..8 {
                let a = (i as f32 * 45.0).to_radians();
                pen.line(&[(10.0 + 6.0 * a.cos(), 10.0 + 6.0 * a.sin()), (10.0 + 8.0 * a.cos(), 10.0 + 8.0 * a.sin())]);
            }
        }
        EyeOff => {
            // Lucide `eye-off`
            pen.svg(&[
                "M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49",
                "M14.084 14.158a3 3 0 0 1-4.242-4.242",
                "M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143",
                "m2 2 20 20",
            ]);
        }
        Presets => {
            // a fan of swatches (Lucide `swatch-book`)
            pen.svg(&[
                "M11 17a4 4 0 0 1-8 0V5a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2Z",
                "M16.7 13H19a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H7",
                "M7 17h.01",
                "m11 8 2.3-2.3a2.4 2.4 0 0 1 3.404.004L18.6 7.6a2.4 2.4 0 0 1 .026 3.434L9.9 19.8",
            ]);
        }
        Versions => {
            // a card with older ones stacked behind (Lucide `gallery-vertical-end`)
            pen.svg(&["M7 2h10", "M5 6h14", "M5 10h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2Z"]);
        }
        Activity => {
            // History & Activity: a clock with a turn-back arrow (Lucide `history`)
            pen.svg(&["M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8", "M3 3v5h5", "M12 7v5l4 2"]);
        }
        More | Dots => {
            for x in [5.0, 10.0, 15.0] {
                pen.dot(x, 10.0, 1.3);
            }
        }
        Tag => {
            pen.closed(&[(3.0, 3.5), (10.0, 3.5), (17.0, 10.5), (10.5, 17.0), (3.0, 10.0)]);
            pen.dot(7.0, 7.5, 1.2);
        }
        Info => {
            pen.circle(10.0, 10.0, 7.5);
            pen.line(&[(10.0, 9.0), (10.0, 14.0)]);
            pen.dot(10.0, 6.3, 0.9);
        }
        GridPhoto => {
            pen.rect_fill(3.0, 4.0, 10.0, 9.5, 0.5);
            pen.rect_fill(11.0, 4.0, 17.0, 9.5, 0.5);
            pen.rect_fill(3.0, 10.5, 8.0, 16.0, 0.5);
            pen.rect_fill(9.0, 10.5, 17.0, 16.0, 0.5);
        }
        GridSquare => {
            pen.rect_fill(3.5, 4.0, 9.5, 9.5, 0.5);
            pen.rect_fill(10.5, 4.0, 16.5, 9.5, 0.5);
            pen.rect_fill(3.5, 10.5, 9.5, 16.0, 0.5);
            pen.rect_fill(10.5, 10.5, 16.5, 16.0, 0.5);
        }
        Single => pen.rect_fill(3.0, 5.0, 17.0, 15.0, 0.5),
        Survey => {
            pen.rect(2.5, 3.5, 9.0, 9.0, 1.0);
            pen.rect(11.0, 3.5, 17.5, 9.0, 1.0);
            pen.rect(2.5, 11.0, 9.0, 16.5, 1.0);
            pen.rect(11.0, 11.0, 17.5, 16.5, 1.0);
        }
        Compare => {
            pen.rect_fill(2.0, 5.0, 8.8, 15.0, 0.5);
            pen.rect_fill(11.2, 5.0, 18.0, 15.0, 0.5);
        }
        Sort => {
            pen.line(&[(3.0, 5.0), (17.0, 5.0)]);
            pen.line(&[(3.0, 10.0), (13.0, 10.0)]);
            pen.line(&[(3.0, 15.0), (9.0, 15.0)]);
        }
        Star => pen.closed(&star_pts(10.0, 10.5, 7.5)),
        StarFilled => pen.fill_star_shaped((10.0, 10.5), &star_pts(10.0, 10.5, 7.5)),
        Circle => pen.circle(10.0, 10.0, 6.0),
        FlagPick => {
            // Lucide `flag`
            pen.svg(&["M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z", "M4 22v-7"]);
        }
        FlagReject => {
            // the flag struck through (Lucide `flag-off`)
            pen.svg(&["M8 2c3 0 5 2 8 2s4-1 4-1v11", "M4 22V4", "M4 15s1-1 4-1 5 2 8 2", "m2 2 20 20"]);
        }
        Gear => {
            // a toothed cog (Lucide `settings`), so it no longer reads as a sun
            pen.svg(&[
                "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z",
            ]);
            pen.circle(10.0, 10.0, 2.5);
        }
        Filmstrip => {
            // Lucide `film`
            pen.svg(&[
                "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z",
                "M7 3v18",
                "M3 7.5h4",
                "M3 12h18",
                "M3 16.5h4",
                "M17 3v18",
                "M17 7.5h4",
                "M17 16.5h4",
            ]);
        }
        BeforeAfter => {
            pen.rect(3.0, 4.0, 17.0, 16.0, 1.0);
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
            pen.rect_fill(10.0, 4.0, 17.0, 16.0, 0.5);
        }
        ChevronDown => pen.line(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]),
        Forward | ChevronRight => pen.line(&[(7.5, 5.0), (12.5, 10.0), (7.5, 15.0)]),
        Back | ChevronLeft => pen.line(&[(12.5, 5.0), (7.5, 10.0), (12.5, 15.0)]),
        ChevronDoubleLeft => {
            pen.line(&[(9.0, 5.0), (4.0, 10.0), (9.0, 15.0)]);
            pen.line(&[(16.0, 5.0), (11.0, 10.0), (16.0, 15.0)]);
        }
        ChevronDoubleRight => {
            pen.line(&[(4.0, 5.0), (9.0, 10.0), (4.0, 15.0)]);
            pen.line(&[(11.0, 5.0), (16.0, 10.0), (11.0, 15.0)]);
        }
        ChevronUp => pen.line(&[(5.0, 12.5), (10.0, 7.5), (15.0, 12.5)]),
        Plus => {
            pen.line(&[(10.0, 4.0), (10.0, 16.0)]);
            pen.line(&[(4.0, 10.0), (16.0, 10.0)]);
        }
        Minus => pen.line(&[(4.0, 10.0), (16.0, 10.0)]),
        Album => {
            pen.rect(3.0, 5.0, 15.0, 17.0, 1.0);
            pen.line(&[(6.0, 2.5), (17.5, 2.5), (17.5, 14.0)]);
        }
        Stack => {
            pen.rect(2.5, 7.0, 13.5, 17.5, 1.0);
            pen.line(&[(5.0, 4.5), (16.0, 4.5), (16.0, 14.5)]);
            pen.line(&[(7.5, 2.0), (18.5, 2.0), (18.5, 11.5)]);
        }
        SmartAlbum => {
            pen.rect(3.0, 5.0, 15.0, 17.0, 1.0);
            pen.line(&[(6.0, 2.5), (17.5, 2.5), (17.5, 14.0)]);
            // a small funnel: the album is a saved filter
            pen.closed(&[(5.5, 8.5), (12.5, 8.5), (10.0, 11.5), (10.0, 14.5), (8.0, 13.5), (8.0, 11.5)]);
        }
        Folder => pen.closed(&[(2.5, 5.0), (8.0, 5.0), (9.5, 7.0), (17.5, 7.0), (17.5, 15.5), (2.5, 15.5)]),
        Pencil => {
            pen.closed(&[(4.0, 16.0), (4.5, 12.5), (13.5, 3.5), (16.5, 6.5), (7.5, 15.5)]);
            pen.line(&[(11.5, 5.5), (14.5, 8.5)]);
        }
        Photos => {
            pen.rect(2.5, 4.0, 17.5, 16.0, 1.5);
            pen.line(&[(2.5, 13.5), (7.0, 9.5), (11.0, 13.0), (13.5, 11.0), (17.5, 14.5)]);
            pen.dot(13.5, 7.5, 1.3);
        }
        Clock => {
            pen.circle(10.0, 10.0, 7.0);
            pen.line(&[(10.0, 5.5), (10.0, 10.0), (13.5, 11.5)]);
        }
        Calendar => {
            // a page with a binding bar, two rings and a marked day
            pen.rect(3.0, 4.5, 17.0, 17.0, 1.5);
            pen.line(&[(3.0, 8.5), (17.0, 8.5)]);
            pen.line(&[(7.0, 2.5), (7.0, 6.0)]);
            pen.line(&[(13.0, 2.5), (13.0, 6.0)]);
            pen.rect(11.0, 11.5, 14.0, 14.5, 0.5);
        }
        Trash => {
            pen.line(&[(3.0, 5.5), (17.0, 5.5)]);
            pen.line(&[(8.0, 5.5), (8.0, 3.0), (12.0, 3.0), (12.0, 5.5)]);
            pen.closed(&[(5.0, 5.5), (15.0, 5.5), (14.0, 17.0), (6.0, 17.0)]);
        }
        Curve => {
            pen.rect(3.0, 3.0, 17.0, 17.0, 1.5);
            let pts: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let y = t * t * (3.0 - 2.0 * t);
                    (4.5 + 11.0 * t, 15.5 - 11.0 * y)
                })
                .collect();
            pen.line(&pts);
        }
        ProfileGrid => {
            pen.rect_fill(3.5, 3.5, 9.0, 9.0, 0.5);
            pen.rect_fill(11.0, 3.5, 16.5, 9.0, 0.5);
            pen.rect_fill(3.5, 11.0, 9.0, 16.5, 0.5);
            pen.circle(14.0, 14.0, 2.5);
            pen.line(&[(15.8, 15.8), (17.5, 17.5)]);
        }
        Close => {
            pen.line(&[(5.0, 5.0), (15.0, 15.0)]);
            pen.line(&[(15.0, 5.0), (5.0, 15.0)]);
        }
        Check => pen.line(&[(4.0, 10.5), (8.5, 15.0), (16.0, 5.5)]),
        Brush => {
            // Lucide `brush`
            pen.svg(&[
                "m9.06 11.9 8.07-8.06a2.85 2.85 0 1 1 4.03 4.03l-8.06 8.08",
                "M7.07 14.94c-1.66 0-3 1.35-3 3.02 0 1.33-2.5 1.52-2 2.02 1.08 1.1 2.49 2.02 4 2.02 2.2 0 4-1.8 4-4.04a3.01 3.01 0 0 0-3-3.02z",
            ]);
        }
        Linear => {
            // the gradient's guides as drawn on the photo: start and end lines, a dashed centre line and its pin
            pen.line(&[(2.5, 4.0), (17.5, 4.0)]);
            pen.line(&[(2.5, 16.0), (17.5, 16.0)]);
            for x in [2.5, 6.5, 13.5] {
                pen.line(&[(x, 10.0), (x + 2.0, 10.0)]);
            }
            pen.circle(10.0, 10.0, 1.6);
        }
        Radial => {
            // the outer edge solid, the feathered inner edge dashed, and the centre pin
            pen.circle(10.0, 10.0, 7.5);
            pen.dashed_circle(10.0, 10.0, 4.3, 6);
            pen.dot(10.0, 10.0, 1.1);
        }
        Sky => {
            // a cloud in front of the sun (Lucide `cloud-sun`)
            pen.svg(&[
                "M12 2v2",
                "m4.93 4.93 1.41 1.41",
                "M20 12h2",
                "m19.07 4.93-1.41 1.41",
                "M15.947 12.65a4 4 0 0 0-5.925-4.128",
                "M13 22H7a5 5 0 1 1 4.9-6H13a3 3 0 0 1 0 6Z",
            ]);
        }
        Subject => {
            pen.circle(10.0, 7.0, 3.0);
            pen.arc(10.0, 18.0, 6.5, 200.0, 340.0);
            pen.rect(2.5, 2.5, 17.5, 17.5, 2.0);
        }
        Play => pen.fill(&[(6.0, 4.0), (6.0, 16.0), (16.0, 10.0)], color),
        Pause => {
            pen.rect_fill(5.0, 4.0, 8.5, 16.0, 0.5);
            pen.rect_fill(11.5, 4.0, 15.0, 16.0, 0.5);
        }
        FaceBox => {
            pen.line(&[(2.5, 7.0), (2.5, 2.5), (7.0, 2.5)]);
            pen.line(&[(13.0, 2.5), (17.5, 2.5), (17.5, 7.0)]);
            pen.line(&[(2.5, 13.0), (2.5, 17.5), (7.0, 17.5)]);
            pen.line(&[(13.0, 17.5), (17.5, 17.5), (17.5, 13.0)]);
            pen.circle(10.0, 8.5, 2.4);
            pen.arc(10.0, 16.0, 4.5, 215.0, 325.0);
        }
        Picker => {
            // an eyedropper (Lucide `pipette`)
            pen.svg(&[
                "m2 22 1-1h3l9-9",
                "M3 21v-3l9-9",
                "m15 6 3.4-3.4a2.1 2.1 0 1 1 3 3L18 9l.4.4a2.1 2.1 0 1 1-3 3l-3.8-3.8a2.1 2.1 0 1 1 3-3l.4.4Z",
            ]);
        }
        Rotate => {
            // Lucide `rotate-ccw`
            pen.svg(&["M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8", "M3 3v5h5"]);
        }
        RotateDrag => {
            // the upper arc of a circle, an arrowhead at each end pointing along it
            pen.arc(10.0, 14.0, 7.5, 200.0, 340.0);
            pen.line(&[(0.8, 9.6), (3.0, 11.9), (6.1, 11.0)]);
            pen.line(&[(19.2, 9.6), (17.0, 11.9), (13.9, 11.0)]);
        }
        Flip => {
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
            pen.closed(&[(8.0, 5.0), (8.0, 15.0), (3.0, 15.0)]);
            pen.fill(&[(12.0, 5.0), (12.0, 15.0), (17.0, 15.0)], color);
        }
        Invert => {
            pen.circle(10.0, 10.0, 7.0);
            pen.p.add(Shape::convex_polygon(
                (0..=16)
                    .map(|i| {
                        pen.pt(
                            10.0 + 7.0 * ((90.0 + i as f32 * 180.0 / 16.0f32).to_radians()).cos(),
                            10.0 + 7.0 * ((90.0 + i as f32 * 180.0 / 16.0f32).to_radians()).sin(),
                        )
                    })
                    .collect(),
                color,
                Stroke::NONE,
            ));
        }
        Video => {
            pen.rect(2.5, 5.0, 13.0, 15.0, 1.5);
            pen.closed(&[(13.0, 8.5), (17.5, 6.0), (17.5, 14.0), (13.0, 11.5)]);
        }
        Heart => {
            // Lucide `heart`
            pen.svg(&[
                "M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z",
            ]);
        }
        Edited => {
            // Lucide `square-pen`
            pen.svg(&[
                "M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7",
                "M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z",
            ]);
        }
        Target => {
            pen.circle(8.0, 10.0, 5.0);
            pen.dot(8.0, 10.0, 1.3);
            pen.line(&[(15.5, 3.5), (15.5, 16.5)]);
            pen.line(&[(13.5, 5.5), (15.5, 3.5), (17.5, 5.5)]);
            pen.line(&[(13.5, 14.5), (15.5, 16.5), (17.5, 14.5)]);
        }
        Chat => {
            // rounded bubble with a tail at the lower left
            pen.closed(&[
                (4.5, 3.5),
                (15.5, 3.5),
                (17.0, 5.0),
                (17.0, 12.0),
                (15.5, 13.5),
                (9.0, 13.5),
                (5.5, 16.8),
                (5.5, 13.5),
                (4.5, 13.5),
                (3.0, 12.0),
                (3.0, 5.0),
            ]);
            for x in [6.8, 10.0, 13.2] {
                pen.dot(x, 8.5, 1.1);
            }
        }
        Object => {
            // Lucide `square-dashed-mouse-pointer`
            pen.svg(&[
                "M12.034 12.681a.498.498 0 0 1 .647-.647l9 3.5a.5.5 0 0 1-.033.943l-3.444 1.068a1 1 0 0 0-.66.66l-1.067 3.443a.5.5 0 0 1-.943.033z",
                "M5 3a2 2 0 0 0-2 2",
                "M19 3a2 2 0 0 1 2 2",
                "M5 21a2 2 0 0 1-2-2",
                "M9 3h1",
                "M9 21h2",
                "M14 3h1",
                "M3 9v1",
                "M21 9v2",
                "M3 14v1",
            ]);
        }
        Describe => {
            // Lucide `message-square-text`
            pen.svg(&["M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z", "M13 8H7", "M17 12H7"]);
        }
        Background => {
            // our own: the Subject frame with a ridge behind the person
            pen.rect(2.5, 2.5, 17.5, 17.5, 2.0);
            pen.line(&[(2.5, 11.5), (5.5, 8.0), (7.3, 10.0)]);
            pen.line(&[(12.7, 10.0), (14.5, 8.0), (17.5, 11.5)]);
            pen.circle(10.0, 9.0, 2.3);
            pen.arc(10.0, 17.5, 4.8, 200.0, 340.0);
        }
        People => {
            // Lucide `users`
            pen.svg(&[
                "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2",
                "M5 7a4 4 0 1 0 8 0a4 4 0 1 0 -8 0Z",
                "M22 21v-2a4 4 0 0 0-3-3.87",
                "M16 3.13a4 4 0 0 1 0 7.75",
            ]);
        }
        Landscape => {
            // Lucide `mountain-snow`
            pen.svg(&["m8 3 4 8 5-5 5 15H2L8 3z", "M4.14 15.08c2.62-1.57 5.24-1.43 7.86.42 2.74 1.94 5.49 2 8.23.19"]);
        }
        Luminance => {
            // a disc, its right half solid (as Lucide `contrast`, filled)
            pen.circle(10.0, 10.0, 7.5);
            let half: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let a = (-90.0 + 180.0 * i as f32 / 16.0f32).to_radians();
                    (10.0 + 5.0 * a.cos(), 10.0 + 5.0 * a.sin())
                })
                .collect();
            pen.fill(&half, color);
        }
        Depth => {
            // Lucide `layers`
            pen.svg(&[
                "m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z",
                "m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65",
                "m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65",
            ]);
        }
        RedEye => {
            // the eye of Lucide `eye` with a solid pupil
            pen.svg(&["M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"]);
            pen.dot(10.0, 10.0, 3.0);
        }
        Palette => {
            // Lucide `palette`
            pen.svg(&[
                "M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z",
                "M13.5 6.5h.01",
                "M17.5 10.5h.01",
                "M8.5 7.5h.01",
                "M6.5 12.5h.01",
            ]);
        }
    }
}

/// A tiny SVG path-data reader: `M L H V C S Q T A Z` (absolute and relative), curves flattened to polylines.
/// Returns each subpath's points and whether it was closed. Never panics; stops at the first malformed token.
fn svg_subpaths(d: &str) -> Vec<(Vec<(f32, f32)>, bool)> {
    const STEPS: usize = 12;
    let mut lex = Lex { b: d.as_bytes(), i: 0 };
    let mut out: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    // the previous curve's kind and last control point, for S / T reflection
    let mut last_ctrl: Option<(u8, f32, f32)> = None;
    let mut cmd = 0u8;
    loop {
        if let Some(c) = lex.cmd() {
            cmd = c;
        } else if !lex.at_number() || cmd == 0 || cmd.eq_ignore_ascii_case(&b'z') {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        let mut ctrl = None;
        // a drawing command right after `Z` starts a new subpath at the closed one's start
        if cur.is_empty() && !cmd.eq_ignore_ascii_case(&b'm') && !cmd.eq_ignore_ascii_case(&b'z') {
            cur.push((x, y));
        }
        let step = (|| -> Option<()> {
            match cmd.to_ascii_uppercase() {
                b'M' => {
                    let (nx, ny) = (lex.num()? + ox, lex.num()? + oy);
                    if !cur.is_empty() {
                        out.push((std::mem::take(&mut cur), false));
                    }
                    cur.push((nx, ny));
                    (x, y, sx, sy) = (nx, ny, nx, ny);
                    // further coordinate pairs after a moveto are linetos
                    cmd = if rel { b'l' } else { b'L' };
                }
                b'L' => {
                    (x, y) = (lex.num()? + ox, lex.num()? + oy);
                    cur.push((x, y));
                }
                b'H' => {
                    x = lex.num()? + ox;
                    cur.push((x, y));
                }
                b'V' => {
                    y = lex.num()? + oy;
                    cur.push((x, y));
                }
                b'C' | b'S' => {
                    let (x1, y1) = if cmd.eq_ignore_ascii_case(&b'C') {
                        (lex.num()? + ox, lex.num()? + oy)
                    } else {
                        match last_ctrl {
                            Some((b'C', cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                            _ => (x, y),
                        }
                    };
                    let (x2, y2, ex, ey) = (lex.num()? + ox, lex.num()? + oy, lex.num()? + ox, lex.num()? + oy);
                    for i in 1..=STEPS {
                        let t = i as f32 / STEPS as f32;
                        let u = 1.0 - t;
                        let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        cur.push((a * x + b * x1 + c * x2 + e * ex, a * y + b * y1 + c * y2 + e * ey));
                    }
                    ctrl = Some((b'C', x2, y2));
                    (x, y) = (ex, ey);
                }
                b'Q' | b'T' => {
                    let (x1, y1) = if cmd.eq_ignore_ascii_case(&b'Q') {
                        (lex.num()? + ox, lex.num()? + oy)
                    } else {
                        match last_ctrl {
                            Some((b'Q', cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                            _ => (x, y),
                        }
                    };
                    let (ex, ey) = (lex.num()? + ox, lex.num()? + oy);
                    for i in 1..=STEPS {
                        let t = i as f32 / STEPS as f32;
                        let u = 1.0 - t;
                        cur.push((u * u * x + 2.0 * u * t * x1 + t * t * ex, u * u * y + 2.0 * u * t * y1 + t * t * ey));
                    }
                    ctrl = Some((b'Q', x1, y1));
                    (x, y) = (ex, ey);
                }
                b'A' => {
                    let (rx, ry, rot) = (lex.num()?, lex.num()?, lex.num()?);
                    let (large, sweep) = (lex.flag()?, lex.flag()?);
                    let (ex, ey) = (lex.num()? + ox, lex.num()? + oy);
                    arc_to(&mut cur, (x, y), (rx, ry), rot, large, sweep, (ex, ey));
                    (x, y) = (ex, ey);
                }
                b'Z' => {
                    if !cur.is_empty() {
                        out.push((std::mem::take(&mut cur), true));
                    }
                    (x, y) = (sx, sy);
                }
                _ => return None,
            }
            Some(())
        })();
        if step.is_none() {
            break;
        }
        last_ctrl = ctrl;
    }
    if !cur.is_empty() {
        out.push((cur, false));
    }
    out
}

/// The path-data tokenizer behind [`svg_subpaths`].
struct Lex<'a> {
    b: &'a [u8],
    i: usize,
}

impl Lex<'_> {
    fn skip(&mut self) {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace() || *c == b',') {
            self.i += 1;
        }
    }
    fn cmd(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.b.get(self.i)?;
        // `e` / `E` only ever appear inside numbers, which `num` consumes whole
        c.is_ascii_alphabetic().then(|| {
            self.i += 1;
            c
        })
    }
    fn at_number(&mut self) -> bool {
        self.skip();
        self.b.get(self.i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }
    fn num(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        if matches!(self.b.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut dot = false;
        while let Some(c) = self.b.get(self.i) {
            match c {
                b'0'..=b'9' => {}
                // a second `.` starts the next number (`.4.4`)
                b'.' if !dot => dot = true,
                b'e' | b'E' if self.i > start => {
                    self.i += 1;
                    if matches!(self.b.get(self.i), Some(b'-' | b'+')) {
                        self.i += 1;
                    }
                    continue;
                }
                _ => break,
            }
            self.i += 1;
        }
        std::str::from_utf8(self.b.get(start..self.i)?).ok()?.parse::<f32>().ok().filter(|v| v.is_finite())
    }
    /// An arc flag: a single `0` or `1`, which may run straight into the next number (`a1 1 0 011 1`).
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.b.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Append an SVG elliptical arc (endpoint parameterisation, SVG 1.1 §F.6.5) as a polyline.
fn arc_to(out: &mut Vec<(f32, f32)>, p0: (f32, f32), r: (f32, f32), rot_deg: f32, large: bool, sweep: bool, p1: (f32, f32)) {
    let (mut rx, mut ry) = (r.0.abs(), r.1.abs());
    if rx < 1e-6 || ry < 1e-6 || (p0.0 - p1.0).hypot(p0.1 - p1.1) < 1e-6 {
        out.push(p1);
        return;
    }
    let (s, c) = rot_deg.to_radians().sin_cos();
    let (dx, dy) = ((p0.0 - p1.0) / 2.0, (p0.1 - p1.1) / 2.0);
    let (x1, y1) = (c * dx + s * dy, -s * dx + c * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let coef = if den > 0.0 { (num / den).max(0.0).sqrt() * sign } else { 0.0 };
    let (cxp, cyp) = (coef * rx * y1 / ry, -coef * ry * x1 / rx);
    let (cx, cy) = (c * cxp - s * cyp + (p0.0 + p1.0) / 2.0, s * cxp + c * cyp + (p0.1 + p1.1) / 2.0);
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((x1 - cxp) / rx, (y1 - cyp) / ry);
    let (vx, vy) = ((-x1 - cxp) / rx, (-y1 - cyp) / ry);
    let t0 = angle(1.0, 0.0, ux, uy);
    let mut dt = angle(ux, uy, vx, vy);
    let tau = std::f32::consts::TAU;
    if !sweep && dt > 0.0 {
        dt -= tau;
    } else if sweep && dt < 0.0 {
        dt += tau;
    }
    let n = ((dt.abs() / (tau / 32.0)).ceil() as usize).clamp(2, 64);
    for i in 1..=n {
        let t = t0 + dt * i as f32 / n as f32;
        let (st, ct) = t.sin_cos();
        out.push((c * rx * ct - s * ry * st + cx, s * rx * ct + c * ry * st + cy));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_arrowheads_have_both_halves() {
        for icon in [Icon::Back, Icon::Forward] {
            for scale in [1.0, 2.0] {
                let mut view = crate::headless::HeadlessView::new();
                let raw = crate::headless::HeadlessView::raw_input(vec2(64.0, 64.0), scale, 0.0, vec![]);
                view.run(raw, |ui| {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
                    paint(ui.painter(), Rect::from_center_size(pos2(32.0, 32.0), vec2(18.6, 18.6)), icon, Color32::WHITE);
                });
                let image = view.paint(&Default::default());
                let middle = image.pixels.len() / 2;
                let upper: u64 = image.pixels[..middle].iter().map(|p| u64::from(p.r())).sum();
                let lower: u64 = image.pixels[middle..].iter().map(|p| u64::from(p.r())).sum();
                assert!(upper > 0 && lower > 0, "the arrow must render");
                assert!(upper.abs_diff(lower) < upper / 20, "arrowhead must be symmetric at scale {scale}: {upper} above, {lower} below");
            }
        }
    }

    #[test]
    fn svg_path_reader_handles_arcs_packed_numbers_and_closes() {
        // a full circle from two arcs, closed
        let subs = svg_subpaths("M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0Z");
        assert_eq!(subs.len(), 1);
        let (pts, closed) = &subs[0];
        assert!(*closed);
        assert!(pts.iter().all(|(x, y)| ((x - 12.0).hypot(y - 12.0) - 3.0).abs() < 0.01), "every point on the circle");
        // packed relative numbers (`.4.4`), an implicit lineto after `m`, and a dot (`h.01`)
        let subs = svg_subpaths("m15 6 3.4-3.4l.4.4M7 17h.01");
        assert_eq!(subs.len(), 2);
        let (a, b) = (&subs[0].0, &subs[1].0);
        assert_eq!(a.len(), 3);
        assert!((a[1].0 - 18.4).abs() < 1e-4 && (a[1].1 - 2.6).abs() < 1e-4);
        assert!((a[2].0 - 18.8).abs() < 1e-4 && (a[2].1 - 3.0).abs() < 1e-4);
        assert!((b[1].0 - 7.01).abs() < 1e-4);
        // a subpath after `Z` restarts from the closed one's start
        let subs = svg_subpaths("M2 2h4v4zl1 1");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].0[0], (2.0, 2.0));
    }

    #[test]
    fn svg_path_reader_never_panics_on_junk() {
        for d in [
            "",
            "Z",
            "M",
            "M1",
            "M1 2 L",
            "a",
            "A1 1 0 2 1 3 3",
            "M0 0A0 0 0 1 1 5 5",
            "M0 0a1e40 1 0 1 1 2 2",
            "M1e 2",
            "M0 0c1",
            "q",
            "M--1 2",
            "M0 0 L NaN 1",
            "✓M0 0",
        ] {
            let _ = svg_subpaths(d);
        }
        // and garbage of every byte pattern
        let mut seed = 0x2545_f491_u32;
        for _ in 0..2000 {
            let s: String = (0..24)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    *b"MmLlHhVvCcSsQqTtAaZz0123456789.-, e".get(seed as usize % 35).unwrap_or(&b' ') as char
                })
                .collect();
            for (pts, _) in svg_subpaths(&s) {
                assert!(pts.iter().all(|(x, y)| x.is_finite() && y.is_finite()), "{s:?} gave non-finite points");
            }
        }
    }

    #[test]
    fn every_icon_draws_something() {
        for icon in Icon::ALL {
            let mut view = crate::headless::HeadlessView::new();
            let raw = crate::headless::HeadlessView::raw_input(vec2(32.0, 32.0), 1.0, 0.0, vec![]);
            view.run(raw, |ui| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
                paint(ui.painter(), Rect::from_min_size(pos2(8.0, 8.0), vec2(16.0, 16.0)), icon, Color32::WHITE);
            });
            let image = view.paint(&Default::default());
            let lit = image.pixels.iter().filter(|p| p.r() > 60).count();
            assert!(lit > 6, "{icon:?} drew only {lit} pixels");
        }
    }

    /// `ICON_SHEET=out.ppm cargo test -p lightcraft-ui-egui icon_contact_sheet -- --ignored` writes every icon, labelled,
    /// at 16 pt and 1×/2× plus 48 pt, light on dark, for reviewing the set by eye.
    #[test]
    #[ignore]
    fn icon_contact_sheet() {
        let Ok(path) = std::env::var("ICON_SHEET") else { return };
        let cols = 6;
        let (cw, ch) = (200.0, 76.0);
        let rows = Icon::ALL.len().div_ceil(cols);
        let size = vec2(cols as f32 * cw, rows as f32 * ch);
        let mut view = crate::headless::HeadlessView::new();
        let raw = crate::headless::HeadlessView::raw_input(size, 2.0, 0.0, vec![]);
        view.run(raw, |ui| {
            let p = ui.painter();
            p.rect_filled(ui.max_rect(), 0.0, Color32::from_gray(38));
            for (i, icon) in Icon::ALL.iter().enumerate() {
                let o = pos2((i % cols) as f32 * cw, (i / cols) as f32 * ch);
                paint(p, Rect::from_min_size(o + vec2(6.0, 6.0), vec2(48.0, 48.0)), *icon, Color32::from_gray(225));
                paint(p, Rect::from_min_size(o + vec2(62.0, 22.0), vec2(16.0, 16.0)), *icon, Color32::from_gray(200));
                p.text(o + vec2(6.0, 58.0), egui::Align2::LEFT_TOP, format!("{icon:?}"), egui::FontId::proportional(11.0), Color32::from_gray(160));
            }
        });
        let img = view.paint(&Default::default());
        let mut out = format!("P6 {} {} 255\n", img.size[0], img.size[1]).into_bytes();
        for px in &img.pixels {
            out.extend([px.r(), px.g(), px.b()]);
        }
        std::fs::write(path, out).expect("write sheet");
    }
}
