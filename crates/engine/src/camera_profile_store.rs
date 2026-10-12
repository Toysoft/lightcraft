//! Named camera profiles. The saved edit holds an immutable, content-addressed id, never a path.
use crate::camera_profiles::{self, CameraProfile, ResolvedProfile};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub model: String,
    pub name: String,
    pub removed: bool,
}

pub fn valid_id(id: &str) -> bool {
    id == "builtin" || (id.len() == 35 && id.starts_with("cp-") && id.get(3..).is_some_and(|s| s.bytes().all(|c| c.is_ascii_hexdigit())))
}

fn profile_id(profile: &CameraProfile) -> Result<String, String> {
    let json = serde_json::to_string(profile).map_err(|e| e.to_string())?;
    Ok(format!("cp-{}", lightcraft_preview::Hasher128::new().str(&json).finish()))
}

fn path(dir: &Path, id: &str, removed: bool) -> Result<PathBuf, String> {
    if !valid_id(id) || id == "builtin" {
        return Err("invalid local camera profile id".into());
    }
    Ok(dir.join(format!("{id}.{}", if removed { "removed" } else { "json" })))
}

fn read_at(dir: &Path, id: &str, removed: bool) -> Result<CameraProfile, String> {
    let p = camera_profiles::load(&path(dir, id, removed)?)?;
    if profile_id(&p)? != id {
        return Err("camera profile contents no longer match its saved identity".into());
    }
    Ok(p)
}

pub fn list_at(dir: &Path, model: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Ok(files) = std::fs::read_dir(dir) {
        for file in files.flatten().take(2048) {
            let p = file.path();
            let Some(id) = p.file_stem().and_then(|s| s.to_str()).filter(|s| valid_id(s) && *s != "builtin") else { continue };
            let removed = p.extension().is_some_and(|s| s == "removed");
            if !removed && !p.extension().is_some_and(|s| s == "json") {
                continue;
            }
            if let Ok(profile) = read_at(dir, id, removed)
                && (model.is_empty() || profile.model == model)
            {
                entries.push(Entry { id: id.into(), model: profile.model, name: profile.name, removed });
            }
        }
    }
    entries.sort_by(|a, b| (&a.model, &a.name, &a.id).cmp(&(&b.model, &b.name, &b.id)));
    entries
}

pub fn list(model: &str) -> Vec<Entry> {
    let directory = camera_profiles::dir();
    let mut cache = listings().lock().unwrap_or_else(PoisonError::into_inner);
    if cache.as_ref().is_none_or(|(d, _, _)| *d != directory) {
        let entries = directory.as_ref().map(|d| list_at(d, "")).unwrap_or_default();
        let choices = directory.as_ref().and_then(|d| defaults(d).ok()).unwrap_or_default();
        *cache = Some((directory, entries, choices));
    }
    cache.as_ref().map(|(_, entries, _)| entries.iter().filter(|e| model.is_empty() || e.model == model).cloned().collect()).unwrap_or_default()
}

fn defaults(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let file = dir.join("camera-defaults.json");
    let metadata = match std::fs::metadata(&file) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.to_string()),
    };
    if metadata.len() > 256 * 1024 {
        return Err("camera defaults file is too large".into());
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(file).map_err(|e| e.to_string())?.take(256 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("camera defaults file is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid camera defaults: {e}"))
}

type Resolution = (Option<ResolvedProfile>, Option<String>);
type Cached = std::collections::HashMap<(Option<PathBuf>, String, String), Resolution>;
fn resolutions() -> &'static Mutex<Cached> {
    static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
type Listings = Option<(Option<PathBuf>, Vec<Entry>, BTreeMap<String, String>)>;
fn listings() -> &'static Mutex<Listings> {
    static CACHE: OnceLock<Mutex<Listings>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
pub(crate) fn clear_cache() {
    *listings().lock().unwrap_or_else(PoisonError::into_inner) = None;
    resolutions().lock().unwrap_or_else(PoisonError::into_inner).clear();
}

pub fn resolve(model: &str, selection: &str) -> Resolution {
    let model = model.trim();
    let key = (camera_profiles::dir(), model.to_string(), selection.to_string());
    let mut cache = resolutions().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    let result = resolve_uncached(model, selection, key.0.clone());
    if cache.len() >= 256 {
        cache.clear();
    }
    cache.insert(key, result.clone());
    result
}

/// Missing explicitly chosen profiles use the built-in profile, not another local profile.
fn resolve_uncached(model: &str, selection: &str, dir: Option<PathBuf>) -> Resolution {
    let selected = if selection.is_empty() {
        dir.as_ref().and_then(|d| defaults(d).ok().and_then(|values| values.get(model).cloned())).unwrap_or_default()
    } else {
        selection.into()
    };
    if selected.is_empty() {
        return (camera_profiles::resolved_legacy(model), None);
    }
    let builtin = || camera_profiles::bundled(model).map(|p| ResolvedProfile { profile: Arc::new(p), local: false });
    if selected == "builtin" {
        return (builtin(), None);
    }
    if let Some(profile) = dir.as_ref().and_then(|d| read_at(d, &selected, false).ok()).filter(|p| p.model == model) {
        return (Some(ResolvedProfile { profile: Arc::new(profile), local: true }), None);
    }
    (builtin(), Some(selected))
}

pub fn install_at(dir: &Path, profile: &CameraProfile, name: &str) -> Result<Entry, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err("use a profile name of 1–120 characters".into());
    }
    let mut profile = profile.clone();
    profile.name = name.into();
    if !profile.valid() {
        return Err("not a usable camera profile".into());
    }
    let id = profile_id(&profile)?;
    let file = path(dir, &id, false)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|e| e.to_string())?;
    if file.exists() {
        if read_at(dir, &id, false)? != profile {
            return Err("an installed profile with this id differs".into());
        }
    } else {
        let removed = path(dir, &id, true)?;
        if removed.exists() {
            // Reinstalling identical data restores its entry instead of leaving both an
            // active file and a tombstone that would make a later removal fail.
            if read_at(dir, &id, true)? != profile {
                return Err("the removed profile with this id differs".into());
            }
            std::fs::rename(removed, &file).map_err(|e| e.to_string())?;
        } else {
            lightcraft_catalog::safe_file::write_atomic(&file, &bytes).map_err(|e| e.to_string())?;
        }
    }
    if camera_profiles::dir().as_deref() == Some(dir) {
        camera_profiles::reload();
    }
    Ok(Entry { id, model: profile.model, name: name.into(), removed: false })
}

pub fn set_default_at(dir: &Path, model: &str, id: &str) -> Result<(), String> {
    if model.trim().is_empty() || model.len() > 512 {
        return Err("invalid camera model".into());
    }
    if !id.is_empty() && id != "builtin" && read_at(dir, id, false)?.model != model {
        return Err("profile is for another camera".into());
    }
    let mut values = defaults(dir)?;
    if id.is_empty() {
        values.remove(model);
    } else {
        values.insert(model.into(), id.into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&values).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("too many camera defaults".into());
    }
    lightcraft_catalog::safe_file::write_atomic(&dir.join("camera-defaults.json"), &bytes).map_err(|e| e.to_string())?;
    if camera_profiles::dir().as_deref() == Some(dir) {
        camera_profiles::reload();
    }
    Ok(())
}

/// Removing is reversible: keep the exact profile outside the active JSON namespace.
pub fn set_removed_at(dir: &Path, id: &str, removed: bool) -> Result<(), String> {
    read_at(dir, id, !removed)?;
    let from = path(dir, id, !removed)?;
    let to = path(dir, id, removed)?;
    if to.exists() {
        return Err("the destination profile already exists".into());
    }
    std::fs::rename(from, to).map_err(|e| e.to_string())?;
    if camera_profiles::dir().as_deref() == Some(dir) {
        camera_profiles::reload();
    }
    Ok(())
}

/// Catalog metadata stores make and model together; require an exact model suffix boundary.
pub fn matches_camera(camera: &str, model: &str) -> bool {
    camera == model || camera.strip_suffix(model).is_some_and(|prefix| prefix.ends_with(' '))
}

/// Stable identity for offline proxies. A named profile and its missing-profile fallback
/// must never share a proxy, nor may a new model default reuse the old default's pixels.
pub fn proxy_choice(camera: &str, selection: &str) -> String {
    let entries = list("");
    let selected = if selection.is_empty() {
        let cache = listings().lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .as_ref()
            .and_then(|(_, _, choices)| choices.iter().find(|(model, _)| matches_camera(camera, model)).map(|(_, id)| id.clone()))
            .unwrap_or_default()
    } else {
        selection.into()
    };
    if selected.is_empty() || selected == "builtin" {
        return selected;
    }
    if entries.iter().any(|e| e.id == selected && !e.removed && matches_camera(camera, &e.model)) { selected } else { format!("{selected}:missing") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_missing_explicit_choice_never_uses_another_local_profile() {
        let directory = std::env::temp_dir().join(format!("lc-profile-fallback-{}", std::process::id()));
        let local = CameraProfile::new("ILCE-7M4", 9, 1234, lightcraft_color::Mat3::IDENTITY, None);
        camera_profiles::save(&local, &directory).unwrap();
        let missing = "cp-00000000000000000000000000000000";
        let (profile, diagnostic) = resolve_uncached("ILCE-7M4", missing, Some(directory.clone()));
        assert_eq!(diagnostic.as_deref(), Some(missing));
        assert!(!profile.unwrap().local);
        let (profile, diagnostic) = resolve_uncached("ILCE-7M4", "builtin", Some(directory.clone()));
        assert!(!profile.unwrap().local);
        assert!(diagnostic.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn named_profiles_coexist_remove_restore_and_preserve_corrupt_defaults() {
        let dir = std::env::temp_dir().join(format!("lc-named-profiles-{}", std::process::id()));
        let profile = CameraProfile::new("Test Camera", 12, 3000, lightcraft_color::Mat3::IDENTITY, None);
        let a = install_at(&dir, &profile, "Standard").unwrap();
        let b = install_at(&dir, &profile, "Vivid").unwrap();
        assert_ne!(a.id, b.id);
        assert!(valid_id(&a.id));
        assert_eq!(list_at(&dir, "Test Camera").len(), 2);
        set_default_at(&dir, "Test Camera", &a.id).unwrap();
        assert_eq!(defaults(&dir).unwrap()["Test Camera"], a.id);
        assert!(set_default_at(&dir, "Other Camera", &a.id).is_err());
        set_removed_at(&dir, &a.id, true).unwrap();
        assert!(read_at(&dir, &a.id, false).is_err());
        assert_eq!(list_at(&dir, "Test Camera").iter().filter(|e| e.removed).count(), 1);
        assert_eq!(install_at(&dir, &profile, "Standard").unwrap().id, a.id);
        assert_eq!(list_at(&dir, "Test Camera").len(), 2);
        assert!(list_at(&dir, "Test Camera").iter().all(|e| !e.removed));
        set_removed_at(&dir, &a.id, true).unwrap();
        set_removed_at(&dir, &a.id, false).unwrap();
        assert_eq!(read_at(&dir, &a.id, false).unwrap().name, "Standard");
        assert!(install_at(&dir, &profile, "\n").is_err());
        assert!(set_removed_at(&dir, "../../escape", true).is_err());
        std::fs::write(dir.join("camera-defaults.json"), b"broken").unwrap();
        assert!(set_default_at(&dir, "Test Camera", "builtin").is_err());
        assert_eq!(std::fs::read(dir.join("camera-defaults.json")).unwrap(), b"broken");
        let mut altered = read_at(&dir, &a.id, false).unwrap();
        altered.name = "Changed externally".into();
        std::fs::write(path(&dir, &a.id, false).unwrap(), serde_json::to_vec(&altered).unwrap()).unwrap();
        assert!(read_at(&dir, &a.id, false).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
