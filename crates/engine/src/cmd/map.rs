//! The map view's contents: the photos of the current view (the ones in the grid and the filmstrip) that have a GPS
//! position, at most a limit of them (Settings ▸ Interface ▸ Map; [`lightcraft_map::MAX_PHOTOS`] by default),
//! grouped into stacks for a map view (photos at the same place, or too close to tell apart at that zoom, are one
//! stack). The UI draws exactly what [`shown`] and [`lightcraft_map::stacks`] return, and `map.photos` gives agents
//! the same answer (given the same `limit`).
//!
//! Sources: own design.

use serde_json::{Value, json};

use lightcraft_catalog::PhotoId;
use lightcraft_map::{MAX_PHOTOS, Pin, STACK_RADIUS, View, photo_limit};

use super::{CommandSpec, always, cmd, f64_or};
use crate::{Result, Session};

/// Which of the photos asked for are on the map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The photos on the map, in the order they were asked for (at most the limit).
    pub pins: Vec<Pin>,
    /// How many photos were asked for.
    pub total: usize,
    /// How many of them have a position.
    pub located: usize,
}

impl Shown {
    /// Photos with a position that are not shown because of the limit.
    pub fn over_limit(&self) -> usize {
        self.located.saturating_sub(self.pins.len())
    }

    /// The view that shows all of them in `width` × `height` points.
    pub fn fit(&self, width: f64, height: f64) -> View {
        let at: Vec<(f64, f64)> = self.pins.iter().map(|p| (p.lat, p.lon)).collect();
        View::fit(&at, width, height, 80.0)
    }
}

/// The photos of `ids` that go on the map: those with a valid GPS position, the first `max` of them (see
/// [`lightcraft_map::photo_limit`]).
pub fn shown(s: &Session, ids: &[PhotoId], max: usize) -> Shown {
    let located = ids.iter().filter_map(|id| {
        let (lat, lon) = s.catalog.photo(*id)?.meta.gps?;
        Some(Pin { id: id.0, lat, lon })
    });
    let (pins, dropped) = lightcraft_map::limit(located, max);
    Shown { located: pins.len().saturating_add(dropped), pins, total: ids.len() }
}

fn photos(s: &mut Session, p: &Value) -> Result<Value> {
    // the same photos the Map view shows: the ones in the filmstrip, unless some are named
    let named = p.get("ids").is_some_and(Value::is_array) || p.get("id").is_some_and(Value::is_u64);
    let ids = if named { s.targets(p) } else { s.visible_cloned() };
    let max = photo_limit(p.get("limit").and_then(Value::as_u64).map_or(MAX_PHOTOS, |n| usize::try_from(n).unwrap_or(usize::MAX)));
    let shown = shown(s, &ids, max);
    let (width, height) = (f64_or(p, "width", 1200.0), f64_or(p, "height", 800.0));
    let view = match (p.get("lat").and_then(Value::as_f64), p.get("lon").and_then(Value::as_f64)) {
        (Some(lat), Some(lon)) => View::new(lat, lon, f64_or(p, "zoom", 2.0), width, height),
        _ => shown.fit(width, height),
    };
    let stacks: Vec<Value> = lightcraft_map::stacks(&shown.pins, &view, f64_or(p, "radius", STACK_RADIUS))
        .into_iter()
        .map(|st| json!({"count": st.ids.len(), "ids": st.ids, "lat": st.lat, "lon": st.lon, "x": st.x, "y": st.y}))
        .collect();
    Ok(json!({
        "total": shown.total, "located": shown.located, "shown": shown.pins.len(), "overLimit": shown.over_limit(),
        "limit": max, "view": view, "stacks": stacks,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "map.photos",
        "Photos on the Map",
        [],
        None,
        "{ids?, limit?, lat?, lon?, zoom?, width?, height?, radius?} → {total, located, shown, overLimit, limit, view: {lat, lon, zoom, width, height}, stacks: [{count, ids, lat, lon, x, y}]} — what the Map view shows: the photos of the current view (the ones in the grid and the filmstrip; or `ids`) that have a GPS position, the first `limit` of them (default 100, at most 1000; the Map view uses the limit set in Settings ▸ Interface ▸ Map), grouped into stacks for a map `width` × `height` points (default 1200 × 800) centred on `lat` / `lon` at `zoom` (0–19); without a centre the view is fitted to the photos. Photos at the same place, or closer than `radius` points on screen (default 44), are one stack; `x` / `y` are the stack's place in the view",
        always,
        photos
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_catalog::Op;

    fn place(s: &mut Session, id: PhotoId, gps: Option<(f64, f64)>) {
        let mut meta = s.catalog.photo(id).unwrap().meta.clone();
        meta.gps = gps;
        s.commit("test", Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
    }

    fn located_session() -> (Session, Vec<PhotoId>) {
        let mut s = Session::with_demo();
        let ids = s.visible_cloned();
        for id in &ids {
            place(&mut s, *id, None);
        }
        (s, ids)
    }

    #[test]
    fn the_map_shows_the_photos_of_the_view_that_have_a_position() {
        let (mut s, ids) = located_session();
        assert!(ids.len() >= 5, "the demo library has {} photos", ids.len());
        // two at the same spot in Munich, one in Berlin, one with a position that is not on the globe
        place(&mut s, ids[0], Some((48.1374, 11.5755)));
        place(&mut s, ids[1], Some((48.1374, 11.5755)));
        place(&mut s, ids[2], Some((52.52, 13.405)));
        place(&mut s, ids[3], Some((f64::NAN, 400.0)));
        // every photo of the view counts, whatever is selected
        s.execute("library.select", &json!({"ids": [ids[2].0]})).unwrap();
        let r = s.execute("map.photos", &json!({})).unwrap();
        assert_eq!(r["total"].as_u64(), Some(ids.len() as u64), "{r}");
        assert_eq!(
            (r["located"].as_u64(), r["shown"].as_u64(), r["overLimit"].as_u64(), r["limit"].as_u64()),
            (Some(3), Some(3), Some(0), Some(100)),
            "{r}"
        );
        let stacks = r["stacks"].as_array().unwrap();
        assert_eq!(stacks.len(), 2, "{r}");
        assert_eq!(stacks[0]["ids"], json!([ids[0].0, ids[1].0]));
        assert_eq!(stacks[0]["count"].as_u64(), Some(2));
        assert_eq!(stacks[1]["ids"], json!([ids[2].0]));
        // the fitted view holds both places
        let (w, h) = (r["view"]["width"].as_f64().unwrap(), r["view"]["height"].as_f64().unwrap());
        for st in stacks {
            let (x, y) = (st["x"].as_f64().unwrap(), st["y"].as_f64().unwrap());
            assert!(x > 0.0 && x < w && y > 0.0 && y < h, "{st}");
        }
        // named photos instead: one stack of one
        let r = s.execute("map.photos", &json!({"ids": [ids[2].0]})).unwrap();
        assert_eq!((r["total"].as_u64(), r["shown"].as_u64(), r["stacks"].as_array().map(Vec::len)), (Some(1), Some(1), Some(1)), "{r}");
        // nothing with a position: an empty map of the world, not an error
        let r = s.execute("map.photos", &json!({"ids": [ids[4].0, 987_654_321u64]})).unwrap();
        assert_eq!((r["total"].as_u64(), r["located"].as_u64(), r["stacks"].as_array().map(Vec::len)), (Some(2), Some(0), Some(0)), "{r}");
    }

    #[test]
    fn a_given_view_decides_what_stacks_and_nonsense_views_are_made_usable() {
        let (mut s, ids) = located_session();
        place(&mut s, ids[0], Some((48.1374, 11.5755)));
        place(&mut s, ids[1], Some((48.1593, 11.6030)));
        let two = json!([ids[0].0, ids[1].0]);
        let far = s.execute("map.photos", &json!({"ids": two, "lat": 48.14, "lon": 11.58, "zoom": 5})).unwrap();
        assert_eq!(far["stacks"].as_array().map(Vec::len), Some(1), "{far}");
        let near = s.execute("map.photos", &json!({"ids": two, "lat": 48.14, "lon": 11.58, "zoom": 14})).unwrap();
        assert_eq!(near["stacks"].as_array().map(Vec::len), Some(2), "{near}");
        let odd = s
            .execute("map.photos", &json!({"ids": two, "lat": 1e300, "lon": -1e300, "zoom": -40, "width": -3, "height": 1e300, "radius": -1}))
            .unwrap();
        let v = &odd["view"];
        assert!(v["lat"].as_f64().is_some_and(|x| x.abs() <= 90.0) && v["lon"].as_f64().is_some_and(|x| x.abs() <= 180.0), "{odd}");
        assert!(v["zoom"].as_f64().is_some_and(|z| (0.0..=19.0).contains(&z)) && v["width"].as_f64() == Some(1.0), "{odd}");
    }

    #[test]
    fn no_more_than_a_hundred_photos_go_on_the_map() {
        let mut s = Session::with_demo();
        let first = s.visible_cloned()[0];
        // 130 photos with a position: virtual copies of the first one
        place(&mut s, first, Some((10.0, 20.0)));
        let ids: Vec<PhotoId> = std::iter::repeat_n(first, 130).collect();
        let sh = shown(&s, &ids, MAX_PHOTOS);
        assert_eq!((sh.total, sh.located, sh.pins.len(), sh.over_limit()), (130, 130, MAX_PHOTOS, 30));
        let all: Vec<u64> = ids.iter().map(|i| i.0).collect();
        let r = s.execute("map.photos", &json!({"ids": all})).unwrap();
        assert_eq!((r["shown"].as_u64(), r["overLimit"].as_u64()), (Some(100), Some(30)), "{r}");
        assert_eq!(r["stacks"][0]["count"].as_u64(), Some(100), "all at one place: one stack");
        assert_eq!(r["stacks"].as_array().map(Vec::len), Some(1));
        // another limit, and nonsense ones made usable
        let r = s.execute("map.photos", &json!({"ids": all, "limit": 120})).unwrap();
        assert_eq!((r["shown"].as_u64(), r["overLimit"].as_u64(), r["limit"].as_u64()), (Some(120), Some(10), Some(120)), "{r}");
        let r = s.execute("map.photos", &json!({"ids": all, "limit": 0})).unwrap();
        assert_eq!(r["limit"].as_u64(), Some(100), "{r}");
        let r = s.execute("map.photos", &json!({"ids": all, "limit": u64::MAX})).unwrap();
        assert_eq!((r["shown"].as_u64(), r["limit"].as_u64()), (Some(130), Some(1000)), "{r}");
    }
}
