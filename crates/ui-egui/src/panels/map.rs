//! Map: the photos of the current view (the ones in the filmstrip) that have a GPS position, on a map of the
//! world. At most the limit set in Settings ▸ Interface ▸ Map are shown ([`lightcraft_map::MAX_PHOTOS`] by default); photos at the same place (or too close to tell apart
//! at the current zoom) are one stack: the first photo's picture with the number of photos in its corner. Every
//! marker has a tip that points at its position. A click on a marker selects its photos, which marks them in
//! the filmstrip.
//!
//! The map is the offline basemap (`lightcraft-map`, Natural Earth) unless online tiles are switched on
//! (`map.onlineTiles`); then OpenStreetMap tiles are drawn over it as they arrive
//! (`lightcraft_engine::map_tiles`). Drag pans, the wheel or a pinch zooms about the pointer, a double click
//! zooms in. The filmstrip (View ▸ Filmstrip) sits below the map: it lists the photos the map shows.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::cmd::map::Shown;
use lightcraft_engine::map_tiles::{MapTiles, OSM_ATTRIBUTION, TileKey};
use lightcraft_map::basemap::{self, Basemap, Colors};
use lightcraft_map::{STACK_RADIUS, Stack, View};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::state::MapState;
use crate::theme::Tokens;
use crate::widgets::{register, text_button};

const HEADER_H: f32 = 44.0;
const PAD: f32 = 20.0;
/// A marker's picture (points).
const MARKER: f32 = 52.0;
/// The white frame around it, and the tip below that points at the position (height, half its width).
const FRAME: f32 = 2.0;
const TIP: f32 = 9.0;
const TIP_HALF: f32 = 7.0;
/// The disc with a stack's number of photos.
const BADGE: f32 = 19.0;
/// Tile textures kept, and tiles decoded per frame (a decode is about a millisecond).
const MAX_TILE_TEXTURES: usize = 320;
const DECODES_PER_FRAME: usize = 8;
/// How many zoom levels up a coarser tile is looked for, to stand in for one that is still loading.
const STAND_IN_LEVELS: u8 = 6;
/// The basemap is drawn at most this many pixels per point (it has no fine detail to lose).
const MAX_DENSITY: f32 = 2.0;

/// The Map view's pictures: the rendered basemap and the tiles' textures.
#[derive(Default)]
pub struct MapCache {
    /// (what it was rendered for, the picture).
    base: Option<(BaseKey, egui::TextureHandle)>,
    /// A tile's texture and the frame it was last drawn in; `None`: its picture could not be read.
    tiles: HashMap<TileKey, (Option<egui::TextureHandle>, u64)>,
    /// The photos on the map, by library revision, the generation of the visible photos and the limit.
    shown: Option<(u64, u64, usize, std::sync::Arc<Shown>)>,
}

#[derive(Clone, Copy, PartialEq)]
struct BaseKey {
    view: View,
    pixels: [usize; 2],
    dark: bool,
}

impl MapCache {
    /// The visible photos that go on the map (looked up again only when the library or the list changes: it can
    /// be the whole library).
    fn shown(&mut self, session: &mut lightcraft_engine::Session, max: usize) -> std::sync::Arc<Shown> {
        let (generation, ids) = session.visible_shared();
        let rev = session.catalog.revision;
        match &self.shown {
            Some((r, g, m, shown)) if *r == rev && *g == generation && *m == max => shown.clone(),
            _ => {
                let shown = std::sync::Arc::new(lightcraft_engine::cmd::map::shown(session, &ids, max));
                self.shown = Some((rev, generation, max, shown.clone()));
                shown
            }
        }
    }
}

/// The view `state` describes, for a map of `size` points.
fn view_of(state: &MapState, size: egui::Vec2) -> View {
    View::new(state.lat, state.lon, state.zoom, f64::from(size.x), f64::from(size.y))
}

fn store(state: &mut MapState, view: &View) {
    (state.lat, state.lon, state.zoom) = (view.lat, view.lon, view.zoom);
}

/// The photos on the map now: the ones the grid and the filmstrip list.
fn shown(app: &mut LightcraftApp) -> std::sync::Arc<Shown> {
    let max = lightcraft_map::photo_limit(app.ui.map.limit);
    app.map.shown(&mut app.session, max)
}

/// What stands for a set of photos, to notice when the map shows different ones.
fn fingerprint(shown: &Shown) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in &shown.pins {
        (p.id, p.lat.to_bits(), p.lon.to_bits()).hash(&mut h);
    }
    // never 0: that is "nothing fitted yet"
    h.finish() | 1
}

/// `map.view`: `{lat?, lon?, zoom?, fit?}` moves the map (`fit`: to the photos on it) and returns what it shows.
pub fn view_command(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let size = app.ui.map.size;
    let size = vec2(if size[0] > 1.0 { size[0] } else { 1200.0 }, if size[1] > 1.0 { size[1] } else { 800.0 });
    let shown = shown(app);
    let mut view = view_of(&app.ui.map, size);
    if p.get("fit").and_then(Value::as_bool) == Some(true) {
        view = shown.fit(view.width, view.height);
    }
    let num = |key: &str| match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|n| n.is_finite()).map(Some).ok_or_else(|| format!("map.view: `{key}` must be a number")),
    };
    let (lat, lon, zoom) = (num("lat")?, num("lon")?, num("zoom")?);
    if lat.is_some() || lon.is_some() || zoom.is_some() {
        view = View::new(lat.unwrap_or(view.lat), lon.unwrap_or(view.lon), zoom.unwrap_or(view.zoom), view.width, view.height);
    }
    store(&mut app.ui.map, &view);
    // the photos' own fit is not applied over this
    app.ui.map.fitted = fingerprint(&shown);
    let stacks = lightcraft_map::stacks(&shown.pins, &view, STACK_RADIUS);
    Ok(json!({
        "view": view, "total": shown.total, "located": shown.located, "shown": shown.pins.len(), "overLimit": shown.over_limit(), "limit": lightcraft_map::photo_limit(app.ui.map.limit),
        "stacks": stacks.iter().map(|s| json!({"count": s.ids.len(), "ids": s.ids, "lat": s.lat, "lon": s.lon, "x": s.x, "y": s.y})).collect::<Vec<_>>(),
        "online": app.session.map_tiles.enabled(),
    }))
}

/// `map.onlineTiles`: `{enabled?}` switches OpenStreetMap tiles on or off (no `enabled`: the other way).
pub fn online_command(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let on = p.get("enabled").and_then(Value::as_bool).unwrap_or(!app.ui.map.online);
    if on && !MapTiles::available() {
        return Err("map.onlineTiles: this build cannot load online map tiles; the offline map is shown".into());
    }
    app.ui.map.online = on;
    // a choice was made: the map does not ask any more
    app.ui.map.asked = true;
    app.session.map_tiles.set_enabled(on);
    Ok(json!({"enabled": on, "status": app.session.map_tiles.status()}))
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // the saved choice is what counts (a settings file may have switched it on before this session)
    if app.session.map_tiles.enabled() != (app.ui.map.online && MapTiles::available()) {
        app.session.map_tiles.set_enabled(app.ui.map.online);
    }
    let shown = shown(app);
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let title = ui.painter().text(pos2(head.left() + PAD, head.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Map"), t.semibold(15.0), t.text);
    // the filmstrip below the map: its photos are the ones on the map, and a click on a marker marks them there
    let full = ui.available_rect_before_wrap();
    let film_h = if app.ui.filmstrip { t.film_h.min(full.height() / 2.0) } else { 0.0 };
    let area = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - film_h));
    if film_h > 0.0 {
        super::detail::filmstrip(app, ui, Rect::from_min_max(pos2(full.left(), area.bottom()), full.max));
    }
    if shown.total == 0 {
        return super::empty_message(ui, area, "No photos", "Photos with a GPS position appear on the map");
    }
    if shown.pins.is_empty() {
        return super::empty_message(ui, area, "No location in these photos", "Photos with a GPS position appear on the map");
    }

    // the buttons, from the right
    let mut fit = false;
    let mut zoom_by = 0.0;
    let mut buttons = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(title.right() + 14.0, head.top()), pos2(head.right() - PAD, head.bottom())))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    buttons.spacing_mut().item_spacing.x = 6.0;
    if MapTiles::available() {
        let online = text_button(&mut buttons, "map-online", "OpenStreetMap", app.ui.map.online).on_hover_text(crate::i18n::tr(
            "Show the detailed OpenStreetMap map. Its tiles are loaded from openstreetmap.org, which learns the area you look at; your photos stay on this computer.",
        ));
        if online.clicked() {
            let _ = app.run("map.onlineTiles", json!({}));
        }
    }
    fit |= text_button(&mut buttons, "map-fit", "Fit", false).on_hover_text(crate::i18n::tr("Show all photos on the map")).clicked();
    if text_button(&mut buttons, "map-zoom-in", "+", false).on_hover_text(crate::i18n::tr("Zoom in")).clicked() {
        zoom_by = 1.0;
    }
    if text_button(&mut buttons, "map-zoom-out", "−", false).on_hover_text(crate::i18n::tr("Zoom out")).clicked() {
        zoom_by = -1.0;
    }
    // what is on the map, and what is not
    let mut summary = crate::i18n::tr_format!("{n} photo{}", if shown.pins.len() == 1 { "" } else { "s" }, n = shown.pins.len());
    let unplaced = shown.total.saturating_sub(shown.located);
    if unplaced > 0 {
        summary.push_str(" · ");
        summary.push_str(&crate::i18n::tr_format!("{n} without a location", n = unplaced));
    }
    if shown.over_limit() > 0 {
        summary.push_str(" · ");
        summary.push_str(&crate::i18n::tr_format!(
            "the first {limit} of {n} with a location",
            limit = lightcraft_map::photo_limit(app.ui.map.limit),
            n = shown.located
        ));
    }
    let room = (buttons.min_rect().left() - title.right() - 28.0).max(0.0);
    let summary = super::people::fit(ui.painter(), &summary, t.font(13.0), room);
    ui.painter().text(pos2(title.right() + 14.0, head.center().y), Align2::LEFT_CENTER, summary, t.font(13.0), t.text_dim);

    let resp = ui.allocate_rect(area, Sense::click_and_drag());
    register(ui.ctx(), "map:canvas", area);
    if area.width() < 8.0 || area.height() < 8.0 {
        return;
    }
    app.ui.map.size = [area.width(), area.height()];
    let mut view = view_of(&app.ui.map, area.size());
    // other photos than last time: show all of them
    let print = fingerprint(&shown);
    if app.ui.map.fitted != print || fit {
        view = shown.fit(view.width, view.height);
        app.ui.map.fitted = print;
    }

    // moving the map
    let mut moving = false;
    if resp.dragged() {
        let d = resp.drag_delta();
        view = view.panned(f64::from(d.x), f64::from(d.y));
        moving = d != egui::Vec2::ZERO;
    }
    let centre = area.center();
    let local = |p: egui::Pos2| (f64::from(p.x - area.left()), f64::from(p.y - area.top()));
    if zoom_by != 0.0 {
        let (x, y) = local(centre);
        view = view.zoomed_about(x, y, (view.zoom + zoom_by).round());
    }
    if let Some(at) = resp.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let steps = f64::from(scroll) / 120.0 + f64::from(pinch).log2();
        if steps.is_finite() && steps != 0.0 {
            let (x, y) = local(at);
            view = view.zoomed_about(x, y, view.zoom + steps.clamp(-2.0, 2.0));
            moving = true;
        }
        if resp.double_clicked() {
            let (x, y) = local(at);
            view = view.zoomed_about(x, y, view.zoom + 1.0);
        }
    }

    let painter = ui.painter_at(area);
    // under online tiles the offline map is the light one: a dark gap between light tiles would flash
    let dark = is_dark(&t) && !app.session.map_tiles.enabled();
    // the offline map: the whole map when tiles are off, and what shows under tiles still on their way
    if !tiles_cover(app, &view) {
        base(app, ui.ctx(), &painter, area, &view, dark, moving);
    }
    let covered = tiles(app, ui.ctx(), &painter, area, &view);
    if moving {
        // one more frame once it stops, for the sharp basemap
        ui.ctx().request_repaint();
    }

    // the photos
    let stacks = lightcraft_map::stacks(&shown.pins, &view, STACK_RADIUS);
    let ppp = ui.ctx().pixels_per_point();
    // which of them are selected (the selection can be the whole library; the map has a hundred photos at most)
    let on_map: HashSet<u64> = shown.pins.iter().map(|p| p.id).collect();
    let selection = &app.session.selection;
    let selected: HashSet<u64> = selection.ids.iter().chain(selection.active.iter()).map(|id| id.0).filter(|id| on_map.contains(id)).collect();
    // back to front: the first stack (the first photo's) ends up on top
    for (i, stack) in stacks.iter().enumerate().rev() {
        marker(app, ui, &painter, area, &selected, i, stack, ppp);
    }
    app.renderer.want_variants(stacks.len());

    // whose map this is
    let online = app.session.map_tiles.enabled();
    let credit = if online { OSM_ATTRIBUTION } else { basemap::ATTRIBUTION };
    let g = painter.layout_no_wrap(credit.to_string(), t.font(11.0), Color32::WHITE);
    let pill = Rect::from_min_size(area.right_bottom() - g.size() - vec2(14.0, 10.0), g.size() + vec2(8.0, 4.0));
    painter.rect_filled(pill, 3.0, Color32::from_black_alpha(150));
    painter.galley(pill.min + vec2(4.0, 2.0), g, Color32::WHITE);
    if online && app.session.map_tiles.status().last_error.is_some() && !covered {
        let note = crate::i18n::tr("The online map could not be loaded; showing the offline map");
        let g = painter.layout_no_wrap(note.to_string(), t.font(12.0), Color32::WHITE);
        let pill = Rect::from_min_size(pos2(area.left() + 10.0, area.bottom() - g.size().y - 12.0), g.size() + vec2(12.0, 6.0));
        painter.rect_filled(pill, 3.0, Color32::from_black_alpha(170));
        painter.galley(pill.min + vec2(6.0, 3.0), g, Color32::WHITE);
    }
    store(&mut app.ui.map, &view);
    if !app.ui.map.asked && !app.ui.map.online && MapTiles::available() {
        ask_online(app, ui, area);
    }
}

/// The question put once, over the lower edge of the map: load the detailed map from OpenStreetMap, or stay
/// with the offline one? Either answer (or the switch above the map) ends it.
fn ask_online(app: &mut LightcraftApp, ui: &mut egui::Ui, area: Rect) {
    let t = Tokens::get(ui.ctx());
    let size = vec2((area.width() - 2.0 * PAD).clamp(40.0, 560.0), 108.0);
    let card = Rect::from_min_size(pos2(area.center().x - size.x / 2.0, area.bottom() - size.y - 34.0), size).intersect(area);
    // clicks on the card are not clicks on the map under it
    ui.interact(card, egui::Id::new("map-ask-online"), Sense::click_and_drag());
    register(ui.ctx(), "map:ask-online", card);
    let p = ui.painter_at(area);
    p.rect_filled(card, 8.0, t.canvas);
    p.rect_stroke(card, 8.0, Stroke::new(1.0, t.text_dim.gamma_multiply(0.5)), StrokeKind::Inside);
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(card.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    inner.spacing_mut().item_spacing.y = 6.0;
    inner.label(egui::RichText::new(crate::i18n::tr("Show streets and place names?")).font(t.semibold(14.0)).color(t.text));
    inner.label(
        egui::RichText::new(crate::i18n::tr(
            "The detailed map is loaded from openstreetmap.org, which learns the area you look at; your photos stay on this computer. You can change this any time above the map.",
        ))
        .font(t.font(12.5))
        .color(t.text_dim),
    );
    let mut answer = None;
    inner.horizontal(|ui| {
        if text_button(ui, "map-ask-online-yes", "Use OpenStreetMap", true).clicked() {
            answer = Some(true);
        }
        if text_button(ui, "map-ask-online-no", "Keep the Offline Map", false).clicked() {
            answer = Some(false);
        }
    });
    if let Some(enabled) = answer {
        let _ = app.run("map.onlineTiles", json!({"enabled": enabled}));
    }
}

fn is_dark(t: &Tokens) -> bool {
    let [r, g, b, _] = t.grid_bg.to_array();
    u32::from(r) + u32::from(g) + u32::from(b) < 384
}

/// Paint the offline basemap for `view` (rendered again only when the view changes; coarser while it moves).
fn base(app: &mut LightcraftApp, ctx: &egui::Context, painter: &egui::Painter, area: Rect, view: &View, dark: bool, moving: bool) {
    let density = if moving { 1.0 } else { ctx.pixels_per_point().clamp(1.0, MAX_DENSITY) };
    let px = |points: f32| ((points * density).round() as usize).clamp(1, basemap::MAX_PIXELS);
    let key = BaseKey { view: *view, pixels: [px(area.width()), px(area.height())], dark };
    // while the map moves, the sharper picture of the same view is not thrown away for a coarser one
    let reuse = app.map.base.as_ref().is_some_and(|(k, _)| k.view == key.view && k.dark == dark && (*k == key || moving));
    if !reuse {
        let colors = if dark { Colors::DARK } else { Colors::LIGHT };
        let Some(rgba) = Basemap::builtin().render(view, key.pixels[0], key.pixels[1], colors) else { return };
        let image = egui::ColorImage::from_rgba_unmultiplied(key.pixels, &rgba);
        match &mut app.map.base {
            Some((k, tex)) => {
                tex.set(image, egui::TextureOptions::LINEAR);
                *k = key;
            }
            None => app.map.base = Some((key, ctx.load_texture("map-base", image, egui::TextureOptions::LINEAR))),
        }
    }
    if let Some((_, tex)) = &app.map.base {
        painter.image(tex.id(), area, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// Whether every tile of `view` already has its picture (then the basemap under them need not be drawn).
fn tiles_cover(app: &LightcraftApp, view: &View) -> bool {
    app.session.map_tiles.enabled()
        && view.tiles().iter().all(|t| TileKey::new(t.z, t.x, t.y).is_some_and(|k| app.map.tiles.get(&k).is_some_and(|(tex, _)| tex.is_some())))
}

/// Paint the online tiles that have arrived and ask for the others. True when they cover the whole map.
fn tiles(app: &mut LightcraftApp, ctx: &egui::Context, painter: &egui::Painter, area: Rect, view: &View) -> bool {
    if !app.session.map_tiles.enabled() {
        return false;
    }
    let frame = ctx.cumulative_frame_nr();
    let mut wanted = view.tiles();
    // nearest the centre first: that is where the eye is
    let middle = (view.width / 2.0, view.height / 2.0);
    let distance = |t: &lightcraft_map::Tile| (t.left + t.edge / 2.0 - middle.0).hypot(t.top + t.edge / 2.0 - middle.1);
    wanted.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
    let keys: Vec<TileKey> = wanted.iter().filter_map(|t| TileKey::new(t.z, t.x, t.y)).collect();
    let pending = app.session.map_tiles.want(&app.session.activity, &keys);
    let mut covered = !wanted.is_empty();
    let mut decoded = 0;
    for tile in &wanted {
        let Some(key) = TileKey::new(tile.z, tile.x, tile.y) else { continue };
        // whole pixels, so neighbours meet without a seam
        let ppp = ctx.pixels_per_point();
        let snap = |v: f64| (v as f32 * ppp).round() / ppp;
        let r = Rect::from_min_max(
            area.min + vec2(snap(tile.left), snap(tile.top)),
            area.min + vec2(snap(tile.left + tile.edge), snap(tile.top + tile.edge)),
        );
        if !app.map.tiles.contains_key(&key) {
            let Some(bytes) = app.session.map_tiles.get(key) else {
                covered = false;
                stand_in(app, painter, key, r, frame);
                continue;
            };
            if decoded >= DECODES_PER_FRAME {
                covered = false;
                stand_in(app, painter, key, r, frame);
                ctx.request_repaint();
                continue;
            }
            decoded += 1;
            let tex = lightcraft_codecs::decode_thumbnail(&bytes, 512).ok().filter(|t| t.image.width > 0 && t.image.height > 0).map(|t| {
                let size = [t.image.width, t.image.height];
                let pixels = t.image.data.iter().map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])).collect();
                ctx.load_texture(format!("map-tile-{}-{}-{}", key.z, key.x, key.y), egui::ColorImage::new(size, pixels), egui::TextureOptions::LINEAR)
            });
            app.map.tiles.insert(key, (tex, frame));
        }
        let Some((tex, used)) = app.map.tiles.get_mut(&key) else { continue };
        *used = frame;
        let Some(tex) = tex else {
            covered = false;
            continue;
        };
        painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if app.map.tiles.len() > MAX_TILE_TEXTURES {
        // the ones not drawn for the longest go
        let mut by_age: Vec<(u64, TileKey)> = app.map.tiles.iter().map(|(k, (_, used))| (*used, *k)).collect();
        by_age.sort_unstable_by_key(|(used, _)| *used);
        let excess = by_age.len().saturating_sub(MAX_TILE_TEXTURES * 3 / 4);
        for (_, k) in by_age.into_iter().take(excess) {
            app.map.tiles.remove(&k);
        }
    }
    if pending > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
    covered
}

/// The tile `up` zoom levels above `key` that covers it, and the part of that tile's picture (0–1) that is `key`.
fn part_of_coarser(key: TileKey, up: u8) -> Option<(TileKey, Rect)> {
    if up == 0 || up > key.z {
        return None;
    }
    let parent = TileKey::new(key.z - up, key.x >> up, key.y >> up)?;
    let n = (1u32 << up) as f32;
    let (u, v) = ((key.x - (parent.x << up)) as f32 / n, (key.y - (parent.y << up)) as f32 / n);
    Some((parent, Rect::from_min_size(pos2(u, v), vec2(1.0 / n, 1.0 / n))))
}

/// A tile's picture is not there yet: paint what other zoom levels already have of its place into `r`. That is
/// the part of the nearest coarser tile that covers it (blurred, but the right map), or else the finer tiles
/// inside it (after zooming out). The map then sharpens tile by tile instead of showing gaps.
fn stand_in(app: &mut LightcraftApp, painter: &egui::Painter, key: TileKey, r: Rect, frame: u64) {
    for up in 1..=STAND_IN_LEVELS.min(key.z) {
        let Some((parent, part)) = part_of_coarser(key, up) else { continue };
        let Some((Some(tex), used)) = app.map.tiles.get_mut(&parent) else { continue };
        // still in use: not thrown away while it stands in
        *used = frame;
        painter.image(tex.id(), r, part, Color32::WHITE);
        return;
    }
    let half = r.size() / 2.0;
    for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
        let (Some(x), Some(y)) = (key.x.checked_mul(2).and_then(|x| x.checked_add(dx)), key.y.checked_mul(2).and_then(|y| y.checked_add(dy))) else {
            continue;
        };
        let Some(child) = key.z.checked_add(1).and_then(|z| TileKey::new(z, x, y)) else { continue };
        let Some((Some(tex), used)) = app.map.tiles.get_mut(&child) else { continue };
        *used = frame;
        let quarter = Rect::from_min_size(r.min + vec2(dx as f32 * half.x, dy as f32 * half.y), half);
        painter.image(tex.id(), quarter, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// Paint a stack's marker (the first photo's picture, a tip below it that points at the position, and the number
/// of photos when there are several) and handle its click: the stack's photos become the selection.
#[allow(clippy::too_many_arguments)]
fn marker(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    area: Rect,
    selected: &HashSet<u64>,
    index: usize,
    stack: &Stack,
    ppp: f32,
) {
    let t = Tokens::get(ui.ctx());
    let Some(first) = stack.ids.first().copied().map(PhotoId) else { return };
    // the tip's point is the position; the picture stands on it
    let at = area.min + vec2(stack.x as f32, stack.y as f32);
    let r = Rect::from_min_size(pos2(at.x - MARKER / 2.0, at.y - TIP - FRAME - MARKER), vec2(MARKER, MARKER));
    let frame = r.expand(FRAME);
    let whole = Rect::from_min_max(frame.min, pos2(frame.right(), at.y));
    if !area.intersects(whole) {
        return;
    }
    let count = stack.ids.len();
    let resp = ui.interact(whole.intersect(area), egui::Id::new(("map-stack", first.0)), Sense::click());
    register(ui.ctx(), format!("map:stack:{index}"), whole);
    let names: Vec<String> = stack.ids.iter().take(6).filter_map(|id| app.session.catalog.photo(PhotoId(*id)).map(|p| p.file_name.clone())).collect();
    resp.widget_info(|| {
        let label =
            if count == 1 { names.first().cloned().unwrap_or_default() } else { crate::i18n::tr_format!("{n} photos at this place", n = count) };
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label)
    });
    // a stack: the cards of the photos under the first one
    for k in (1..count.min(3)).rev() {
        let card = r.translate(vec2(4.0 * k as f32, -4.0 * k as f32));
        painter.rect_filled(card, 3.0, Color32::from_gray(if k == 1 { 205 } else { 165 }));
        painter.rect_stroke(card, 3.0, Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Outside);
    }
    // the frame and its tip are one shape: the accent colour when a photo of the stack is selected
    let marked = stack.ids.iter().any(|id| selected.contains(id));
    let paper = if marked { t.accent } else { Color32::WHITE };
    let edge = Stroke::new(1.0, Color32::from_black_alpha(110));
    painter.rect_stroke(frame, 4.0, edge, StrokeKind::Outside);
    // (the tip starts inside the frame, which then covers its upper edge)
    let top = frame.bottom() - 2.0;
    painter.add(egui::Shape::convex_polygon(vec![pos2(at.x - TIP_HALF, top), pos2(at.x + TIP_HALF, top), at], paper, edge));
    painter.rect_filled(frame, 4.0, paper);
    painter.rect_filled(r, 2.0, t.canvas);
    // the filmstrip's size: a photo has one thumbnail, and two sizes asked for in turn would render it for ever
    super::grid::request_thumb(app, first, (256.0 * ppp.min(2.0) / 2.0) as usize * 2, 10);
    if let Some(tex) = app.renderer.thumb(first) {
        // the middle square of the picture
        let (w, h) = (tex.size[0].max(1) as f32, tex.size[1].max(1) as f32);
        let (u, v) = if w > h { ((1.0 - h / w) / 2.0, 0.0) } else { (0.0, (1.0 - w / h) / 2.0) };
        painter.image(tex.tex.id(), r, Rect::from_min_max(pos2(u, v), pos2(1.0 - u, 1.0 - v)), Color32::WHITE);
    }
    if resp.hovered() && !marked {
        painter.rect_stroke(frame, 4.0, Stroke::new(2.0, t.text_dim), StrokeKind::Outside);
    }
    if count > 1 {
        // how many: white on a black disc in the picture's lower right corner (wider for three digits)
        let g = painter.layout_no_wrap(count.to_string(), t.semibold(11.0), Color32::WHITE);
        let size = vec2((g.size().x + 8.0).max(BADGE), BADGE);
        let badge = Rect::from_min_size(r.right_bottom() - size - vec2(3.0, 3.0), size);
        painter.rect_filled(badge, BADGE / 2.0, Color32::BLACK);
        painter.rect_stroke(badge, BADGE / 2.0, Stroke::new(1.0, Color32::from_white_alpha(170)), StrokeKind::Outside);
        painter.galley(badge.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    let clicked = resp.clicked();
    resp.on_hover_ui(|ui| {
        if count > 1 {
            ui.label(crate::i18n::tr_format!("{n} photos at this place", n = count));
        }
        for name in &names {
            ui.label(name);
        }
        if count > names.len() {
            ui.label(crate::i18n::tr_format!("and {n} more", n = count - names.len()));
        }
        ui.label(egui::RichText::new(format!("{:.5}, {:.5}", stack.lat, stack.lon)).color(t.text_dim));
    });
    if clicked {
        // its photos are the selection now: the filmstrip marks them and scrolls to the first
        let _ = app.run("library.select", json!({"ids": stack.ids, "active": first.0}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_follows_the_state_and_nonsense_in_it_is_made_usable() {
        let mut state = MapState::default();
        let world = view_of(&state, vec2(800.0, 600.0));
        assert!(world.zoom >= 0.0 && world.zoom < 3.0, "the default is the world: {world:?}");
        state.lat = f64::NAN;
        state.lon = 1e300;
        state.zoom = -f64::INFINITY;
        let v = view_of(&state, vec2(f32::NAN, -4.0));
        assert!(lightcraft_map::valid(v.lat, v.lon) && v.zoom.is_finite() && v.width >= 1.0 && v.height >= 1.0, "{v:?}");
        store(&mut state, &View::new(48.0, 11.0, 9.0, 800.0, 600.0));
        assert!((state.lat - 48.0).abs() < 1e-9 && (state.lon - 11.0).abs() < 1e-9 && state.zoom == 9.0, "{state:?}");
    }

    #[test]
    fn a_coarser_tile_stands_in_with_the_part_that_covers_the_missing_one() {
        // tile 13/4358/2843 is the lower left quarter of 12/2179/1421 …
        let key = TileKey::new(13, 4358, 2843).unwrap();
        let (parent, part) = part_of_coarser(key, 1).unwrap();
        assert_eq!((parent.z, parent.x, parent.y), (12, 2179, 1421));
        assert_eq!(part, Rect::from_min_max(pos2(0.0, 0.5), pos2(0.5, 1.0)));
        // … and a sixteenth of the tile two levels up
        let (parent, part) = part_of_coarser(key, 2).unwrap();
        assert_eq!((parent.z, parent.x, parent.y), (11, 1089, 710));
        assert_eq!(part, Rect::from_min_size(pos2(0.5, 0.75), vec2(0.25, 0.25)));
        // nothing above the world, and no level 0 steps
        assert!(part_of_coarser(key, 14).is_none() && part_of_coarser(key, 0).is_none());
        assert!(part_of_coarser(TileKey::new(0, 0, 0).unwrap(), 1).is_none());
    }

    #[test]
    fn different_photos_or_positions_have_different_fingerprints() {
        let pin = |id, lat| lightcraft_map::Pin { id, lat, lon: 5.0 };
        let a = Shown { pins: vec![pin(1, 1.0), pin(2, 2.0)], total: 2, located: 2 };
        let moved = Shown { pins: vec![pin(1, 1.0), pin(2, 2.5)], total: 2, located: 2 };
        let other = Shown { pins: vec![pin(1, 1.0), pin(3, 2.0)], total: 2, located: 2 };
        assert_eq!(fingerprint(&a), fingerprint(&a.clone()));
        assert_ne!(fingerprint(&a), fingerprint(&moved));
        assert_ne!(fingerprint(&a), fingerprint(&other));
        assert_ne!(fingerprint(&Shown::default()), 0);
    }
}
