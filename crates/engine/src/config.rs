//! Where LightCraft keeps per-user data on this machine: the config folder (app settings, camera
//! profiles, face and denoise models) and the cache folder (map tiles).
//!
//! Hosts that have a file system (the desktop app, the CLI, the MCP server) share these defaults so a model
//! installed from one is there for the others. Nothing here is applied automatically: a [`Session`] has no
//! face- or denoise-models folder until a host asks for one ([`Session::with_default_face_models`],
//! [`Session::with_default_denoise_models`]), so tests stay hermetic.

use std::path::PathBuf;

use crate::Session;

/// LightCraft's per-user config folder (`…/LightCraft`), if the platform tells us where.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/LightCraft"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("LightCraft"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("lightcraft"))
    }
}

/// LightCraft's per-user cache folder, for data that can be fetched or rebuilt again (map tiles):
/// `~/Library/Caches/LightCraft` on macOS, `%LOCALAPPDATA%\LightCraft\Cache` on Windows,
/// `$XDG_CACHE_HOME/lightcraft` (else `~/.cache/lightcraft`) elsewhere.
pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/LightCraft"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|a| PathBuf::from(a).join("LightCraft").join("Cache"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|c| c.join("lightcraft"))
    }
}

/// Where map tiles are cached: `$LIGHTCRAFT_MAP_CACHE` if set, else `<cache>/map-tiles` (one
/// folder per tile source inside; see docs/map.md).
pub fn default_map_cache_dir() -> Option<PathBuf> {
    std::env::var_os("LIGHTCRAFT_MAP_CACHE").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| cache_dir().map(|d| d.join("map-tiles")))
}

/// Where face models are kept: `$LIGHTCRAFT_FACE_MODELS` if set, else `<config>/models`.
pub fn default_face_models_dir() -> Option<PathBuf> {
    std::env::var_os("LIGHTCRAFT_FACE_MODELS").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| config_dir().map(|d| d.join("models")))
}

/// Where opt-in denoise models are kept: `$LIGHTCRAFT_DENOISE_MODELS` if set, else `<config>/denoise-models`.
pub fn default_denoise_models_dir() -> Option<PathBuf> {
    std::env::var_os("LIGHTCRAFT_DENOISE_MODELS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| config_dir().map(|d| d.join("denoise-models")))
}

impl Session {
    /// Keep face models in the shared default folder ([`default_face_models_dir`]).
    pub fn with_default_face_models(mut self) -> Self {
        self.face_models_dir = default_face_models_dir();
        self
    }

    /// Cache map tiles in the shared default folder ([`default_map_cache_dir`]).
    pub fn with_default_map_cache(mut self) -> Self {
        self.map.cache_root = default_map_cache_dir();
        self.map.reset_service();
        self
    }

    /// Keep denoise models in the shared default folder ([`default_denoise_models_dir`]).
    pub fn with_default_denoise_models(mut self) -> Self {
        self.set_denoise_models_dir(default_denoise_models_dir());
        self
    }

    /// Point AI denoise at `dir` (or at no folder), dropping what was learned about the old one.
    pub fn set_denoise_models_dir(&mut self, dir: Option<PathBuf>) {
        self.denoise.models_dir = dir;
        self.denoise.touch();
    }
}
