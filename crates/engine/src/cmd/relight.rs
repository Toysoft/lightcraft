//! Relight commands on the active photo: the lights and the switches. The sliders are develop
//! controls (`relight.*`, `relightLight.<i>.*`), set with `develop.set`.

use lightcraft_develop::{DevelopSettings, MAX_RELIGHT_LIGHTS, Relight, RelightKind, RelightLight};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, f64_or, has_active, ok, point, str_param};
use crate::{Result, Session};

/// How far off the photo a light may be placed (normalized coordinates).
const PLACE: (f64, f64) = (-1.0, 2.0);

const KINDS: &str = "point|directional|spot";

fn kind_param(p: &Value, c: &str) -> Result<Option<RelightKind>> {
    match str_param(p, "kind") {
        None => Ok(None),
        Some(name) => RelightKind::ALL
            .into_iter()
            .find(|k| serde_json::to_value(k).is_ok_and(|v| v == name))
            .map(Some)
            .ok_or_else(|| bad(c, format!("unknown light kind `{name}` ({KINDS})"))),
    }
}

/// `position` from the params: finite, and not absurdly far off the photo.
fn position_param(p: &Value, c: &str) -> Result<Option<Point>> {
    if p.get("position").is_none_or(Value::is_null) {
        return Ok(None);
    }
    let at = point(p, "position").ok_or_else(|| bad(c, "`position` must be [x, y], normalized to the photo"))?;
    if !(at.x.is_finite() && at.y.is_finite()) {
        return Err(bad(c, "`position` must be two finite numbers"));
    }
    Ok(Some(Point::new(at.x.clamp(PLACE.0, PLACE.1), at.y.clamp(PLACE.0, PLACE.1))))
}

/// Change the active photo's Relight settings as one history step.
fn edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut Relight) -> Result<Value>) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d: DevelopSettings = (*s.develop_of(id).unwrap_or_default()).clone();
    let out = f(&mut d.relight)?;
    s.set_develop(id, d, label)?;
    Ok(out)
}

fn light_index(r: &Relight, p: &Value, c: &str) -> Result<usize> {
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(c, "missing `index`"))?;
    usize::try_from(i).ok().filter(|i| *i < r.lights.len()).ok_or_else(|| bad(c, "no such light"))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("relight.enable", "Relight", [], None, "{enabled?: bool (default: toggle)}", has_active, |s, p| {
            edit(s, "relight.enable", "Relight", |r| {
                r.enabled = bool_or(p, "enabled", !r.enabled);
                Ok(json!({"enabled": r.enabled}))
            })
        }),
        cmd!(
            "relight.addLight",
            "Add Light",
            [],
            None,
            "{kind?: point|directional|spot (default point), position?: [x,y] normalized to the uncropped photo (default the centre), intensity?: 0..200, hue?: 0..360, sat?: 0..100, angle?: 0..360 (clockwise from the top), elevation?: -180..180, depth?, falloff?, cone?: 0..100} — switches Relight on; returns {index}",
            has_active,
            |s, p| {
                let c = "relight.addLight";
                let d = RelightLight::default();
                let number = |key: &str, default: f64, max: f64| {
                    let v = f64_or(p, key, default);
                    if v.is_finite() { v.clamp(0.0, max) } else { default }
                };
                let light = RelightLight {
                    kind: kind_param(p, c)?.unwrap_or(d.kind),
                    position: position_param(p, c)?.unwrap_or(d.position),
                    intensity: number("intensity", d.intensity, 200.0),
                    hue: number("hue", d.hue, 360.0),
                    sat: number("sat", d.sat, 100.0),
                    angle: number("angle", d.angle, 360.0),
                    elevation: Some(f64_or(p, "elevation", d.elevation)).filter(|v| v.is_finite()).map_or(d.elevation, |v| v.clamp(-180.0, 180.0)),
                    depth: number("depth", d.depth, 100.0),
                    falloff: number("falloff", d.falloff, 100.0),
                    cone: number("cone", d.cone, 100.0),
                };
                edit(s, c, "Add Light", |r| {
                    if r.lights.len() >= MAX_RELIGHT_LIGHTS {
                        return Err(bad(c, format!("a photo takes at most {MAX_RELIGHT_LIGHTS} lights")));
                    }
                    r.lights.push(light);
                    r.enabled = true;
                    Ok(json!({"index": r.lights.len() - 1}))
                })
            }
        ),
        cmd!(
            "relight.updateLight",
            "Change Light",
            [],
            None,
            "{index, kind?: point|directional|spot, position?: [x,y] normalized} — what the sliders don't cover; the sliders are `relightLight.<index>.*` controls",
            has_active,
            |s, p| {
                let c = "relight.updateLight";
                let (kind, position) = (kind_param(p, c)?, position_param(p, c)?);
                if kind.is_none() && position.is_none() {
                    return Err(bad(c, "nothing to change (give `kind` or `position`)"));
                }
                let label = if kind.is_some() { "Light Type" } else { "Move Light" };
                edit(s, c, label, |r| {
                    let i = light_index(r, p, c)?;
                    if let Some(light) = r.lights.get_mut(i) {
                        light.kind = kind.unwrap_or(light.kind);
                        light.position = position.unwrap_or(light.position);
                    }
                    Ok(Value::Null)
                })
            }
        ),
        cmd!("relight.deleteLight", "Remove Light", [], None, "{index}", has_active, |s, p| {
            let c = "relight.deleteLight";
            edit(s, c, "Remove Light", |r| {
                let i = light_index(r, p, c)?;
                r.lights.remove(i);
                Ok(json!({"lights": r.lights.len()}))
            })
        }),
        cmd!("relight.shadows", "Relight Shadows", [], None, "{on?: bool (default: toggle)}", has_active, |s, p| {
            edit(s, "relight.shadows", "Relight Shadows", |r| {
                r.shadows = bool_or(p, "on", !r.shadows);
                Ok(json!({"on": r.shadows}))
            })
        }),
        cmd!(
            "relight.analyze",
            "Analyze Scene",
            [],
            None,
            "{} — look at the photo with the scene geometry model (MoGe-2: surface normals and depth) and keep what it saw with the photo, so Relight shades real shapes instead of a relief guessed from brightness. Needs the model (relight.model, relight.model.download). In the app it runs in the background → {started} or {done}",
            has_active,
            |s, _| {
                let c = "relight.analyze";
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let done = s.relief_analyze(id).map_err(|e| bad(c, e))?;
                Ok(if done { json!({"done": true}) } else { json!({"started": true}) })
            }
        ),
        cmd!(
            "relight.clearGeometry",
            "Remove Scene Analysis",
            [],
            None,
            "{} — forget the analyzed scene: the relief is estimated from the photo's brightness again",
            has_active,
            |s, _| {
                edit(s, "relight.clearGeometry", "Remove Scene Analysis", |r| {
                    r.geometry = None;
                    Ok(Value::Null)
                })
            }
        ),
        cmd!(
            query "relight.model",
            "Scene Geometry Model",
            [],
            None,
            "{} → {available, installed, downloading, done, total, error, analyzing, dir}: the MoGe-2 model Relight analyzes scenes with (optional; 134 MB, MIT License)",
            super::always,
            |s, _| {
                let mut status = serde_json::to_value(s.relief.status()).unwrap_or(Value::Null);
                if let Some(o) = status.as_object_mut() {
                    o.insert("dir".into(), json!(s.relief.dir.as_ref().map(|d| d.display().to_string())));
                }
                Ok(status)
            }
        ),
        cmd!(
            query "relight.model.download",
            "Download Scene Geometry Model",
            [],
            None,
            "{} — download the MoGe-2 model (134 MB, MIT License) in the background; watch relight.model, relight.model.cancel stops it (it resumes later) → {started, installed, downloading}",
            super::always,
            |s, _| {
                let started = s.relief.start_download(&s.activity).map_err(|e| bad("relight.model.download", e))?;
                Ok(json!({"started": started, "installed": s.relief.installed(), "downloading": s.relief.status().downloading}))
            }
        ),
        cmd!(query "relight.model.cancel", "Cancel Scene Geometry Model Download", [], None, "{} → {cancelled}", super::always, |s, _| {
            Ok(json!({"cancelled": s.relief.cancel_download()}))
        }),
        cmd!("relight.reset", "Reset Relight", [], None, "{} — removes every light and switches Relight off", has_active, |s, _| {
            edit(s, "relight.reset", "Reset Relight", |r| {
                *r = Relight::default();
                Ok(Value::Null)
            })?;
            ok()
        }),
    ]
}
