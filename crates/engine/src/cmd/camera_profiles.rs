use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session, camera_profile_store as store};
use serde_json::{Value, json};

fn directory() -> Result<std::path::PathBuf> {
    crate::camera_profiles::dir().ok_or_else(|| bad("cameraProfile", "camera profile folder is unavailable"))
}
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    let paths = if let Some(paths) = p.get("paths").and_then(Value::as_array) {
        if paths.len() > 300 {
            return Err(bad("cameraProfile.start", "choose at most 300 paths"));
        }
        paths
            .iter()
            .map(|p| p.as_str().map(std::path::PathBuf::from).ok_or_else(|| bad("cameraProfile.start", "paths must be strings")))
            .collect::<Result<Vec<_>>>()?
    } else {
        let ids = if p["scope"] == "collection" { s.visible_cloned() } else { s.targets(p) };
        ids.iter()
            .filter_map(|id| s.catalog.photo(*id))
            .filter_map(|p| match &p.source {
                lightcraft_catalog::Source::File { path } if p.develops_raw() => Some(std::path::PathBuf::from(path)),
                _ => None,
            })
            .take(10_001)
            .collect()
    };
    s.calibration.start(paths, &s.activity).map_err(|e| bad("cameraProfile.start", e))?;
    Ok(json!({"started": true}))
}
fn install(s: &mut Session, p: &Value) -> Result<Value> {
    let model = str_param(p, "model").ok_or_else(|| bad("cameraProfile.install", "missing model"))?;
    let name = str_param(p, "name").ok_or_else(|| bad("cameraProfile.install", "missing name"))?;
    let report = s.calibration.snapshot();
    if report.running {
        return Err(bad("cameraProfile.install", "wait for validation to finish"));
    }
    let result =
        report.results.iter().find(|r| r.profile.model == model).ok_or_else(|| bad("cameraProfile.install", "no validated result for this model"))?;
    if !result.improves() && p["allowWorse"] != true {
        return Err(bad("cameraProfile.install", "held-out error did not improve; explicit allowWorse is required"));
    }
    let entry = store::install_at(&directory()?, &result.profile, name).map_err(|e| bad("cameraProfile.install", e))?;
    Ok(json!(entry))
}
fn choose(s: &mut Session, p: &Value) -> Result<Value> {
    let choice = str_param(p, "profile").ok_or_else(|| bad("cameraProfile.choose", "missing profile"))?;
    let entry = if choice.is_empty() || choice == "builtin" {
        None
    } else {
        Some(
            store::list("")
                .into_iter()
                .find(|e| e.id == choice && !e.removed)
                .ok_or_else(|| bad("cameraProfile.choose", "profile is not installed"))?,
        )
    };
    let mut ops = Vec::new();
    for id in s.targets(p) {
        let photo = s.catalog.photo(id).ok_or_else(|| bad("cameraProfile.choose", "photo missing"))?;
        if !photo.develops_raw() {
            continue;
        }
        if entry.as_ref().is_some_and(|e| !store::matches_camera(&photo.meta.camera, &e.model)) {
            return Err(bad("cameraProfile.choose", "the selection includes another camera model"));
        }
        let mut settings = (*photo.develop).clone();
        settings.profile.camera = choice.into();
        if let Some(op) = s.develop_op(id, settings, "Camera profile") {
            ops.push(op);
        }
    }
    let count = ops.len();
    s.commit("Camera profile", lightcraft_catalog::Op::Batch { ops })?;
    Ok(json!({"changed": count}))
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "cameraProfile.start",
            "Build Camera Profiles",
            [],
            None,
            "{paths?: [RAW or folder], scope?: selection|collection} — background, local only; at most 300 RAWs",
            always,
            start
        ),
        cmd!(query "cameraProfile.status", "Camera Calibration Status", [], None, "{} — progress, usable model counts and held-out RMS", always, |s, _| Ok(json!(s.calibration.snapshot()))),
        cmd!("cameraProfile.cancel", "Cancel Camera Calibration", [], None, "{}", always, |s, _| {
            s.calibration.cancel();
            Ok(json!({}))
        }),
        cmd!(
            "cameraProfile.install",
            "Install Camera Profile",
            [],
            None,
            "{model, name, allowWorse?: false} — install a validated result, without changing the model default",
            always,
            install
        ),
        cmd!(
            "cameraProfile.choose",
            "Choose Starting Colour Profile",
            [],
            None,
            "{profile: ''|builtin|cp-…, ids?: [photo ids]} — empty uses model default; undoable",
            always,
            choose
        ),
        cmd!(query "cameraProfile.list", "List Camera Profiles", [], None, "{model?: string}", always, |_, p| Ok(json!(store::list(str_param(p, "model").unwrap_or(""))))),
        cmd!("cameraProfile.default", "Set Camera Model Default", [], None, "{model, profile: ''|builtin|cp-…}", always, |_, p| {
            let model = str_param(p, "model").ok_or_else(|| bad("cameraProfile.default", "missing model"))?;
            let id = str_param(p, "profile").ok_or_else(|| bad("cameraProfile.default", "missing profile"))?;
            store::set_default_at(&directory()?, model, id).map_err(|e| bad("cameraProfile.default", e))?;
            Ok(json!({}))
        }),
        cmd!(
            "cameraProfile.remove",
            "Remove or Restore Camera Profile",
            [],
            None,
            "{profile: cp-…, removed?: true} — reversible; missing choices fall back to built-in",
            always,
            |_, p| {
                let id = str_param(p, "profile").ok_or_else(|| bad("cameraProfile.remove", "missing profile"))?;
                store::set_removed_at(&directory()?, id, p["removed"] != false).map_err(|e| bad("cameraProfile.remove", e))?;
                Ok(json!({}))
            }
        ),
        cmd!("cameraProfile.reload", "Reload Camera Profiles", [], None, "{}", always, |_, _| {
            crate::camera_profiles::reload();
            Ok(json!({}))
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
    use std::sync::{Arc, Mutex};
    #[test]
    fn choice_is_undoable_round_trips_sidecar_and_redecodes_without_reusing_stale_results() {
        let mut session = Session::new();
        let id = PhotoId(1);
        let mut photo = Photo::new(id, Source::File { path: "camera.ARW".into() }, "camera.ARW", "ARW", 32, 32, "2026-10-12");
        photo.kind = MediaKind::Raw;
        session.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        session.media.file_loader = Some(Arc::new(|_, _| Ok((lightcraft_raster::Rgb32f::new(32, 32), Default::default()))));
        let recorded = calls.clone();
        session.media.selected_loader = Some(Arc::new(move |_, _, selection, _| {
            recorded.lock().unwrap().push(selection.to_string());
            Ok((lightcraft_raster::Rgb32f::new(32, 32), None, Default::default()))
        }));
        let old = session.render_job(id, 32, 32, false, true).unwrap().run();
        session.accept(&old);
        let old_key = crate::media::decoded_key(session.catalog.photo(id).unwrap());
        session.execute("cameraProfile.choose", &json!({"ids":[1], "profile":"builtin"})).unwrap();
        let new_key = crate::media::decoded_key(session.catalog.photo(id).unwrap());
        assert_ne!(old_key, new_key);
        let photo = session.catalog.photo(id).unwrap();
        let json = serde_json::to_string(photo.as_ref()).unwrap();
        let roundtrip: Photo = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.develop.profile.camera, "builtin");
        let packet = crate::sidecar::sidecar_packet(photo, &session.catalog);
        let parsed = crate::sidecar::parse_sidecar(&packet, crate::crs::Target::RawAbsolute).unwrap();
        let Some(crate::sidecar::DevelopPatch::Full(settings)) = parsed.develop else { panic!("missing full settings") };
        assert_eq!(settings.profile.camera, "builtin");
        session.accept(&old);
        let new = session.render_job(id, 32, 32, false, true).unwrap().run();
        assert!(new.rendered.is_ok());
        assert_eq!(*calls.lock().unwrap(), vec!["builtin"]);
        session.accept(&new);
        session.execute("edit.undo", &json!({})).unwrap();
        assert!(session.catalog.photo(id).unwrap().develop.profile.camera.is_empty());
        assert_eq!(crate::media::decoded_key(session.catalog.photo(id).unwrap()), old_key);
    }
}
