//! Scene geometry from one photo: MoGe-2 (Wang et al., Microsoft, 2025) inference in pure Rust on
//! candle. The `moge-2-vits-normal` model predicts, for every pixel, the surface normal, a 3-D
//! point (whose z is the depth, up to an unknown shift) and whether there is a surface at all
//! (not sky). Relight shades the photo with that geometry.
//!
//! The backbone is a DINOv2 ViT-S/14 (`vit`); the neck and the three heads are small
//! convolutional stacks (`stack`). One [`Moge::infer`] over about 1 500 tokens takes a few
//! seconds on a CPU; on macOS the model runs on the GPU through Metal.
//!
//! The weights are not part of LightCraft: they are the `Ruicheng/moge-2-vits-normal` checkpoint
//! (MIT License, 134 MB), downloaded only when the user asks for it ([`fetch`]) and verified
//! before use. Nothing in LightCraft requires the model: without it Relight estimates the relief
//! from the photo's brightness, and photos analyzed with it keep their geometry in their settings,
//! so they render without the model (and on the web).
//!
//! Ported from the MoGe (MIT) and DINOv2 (Apache-2.0) reference implementations; see `NOTICE`.
//! This crate is therefore licensed under the Apache License 2.0 only (not the MIT option of the
//! rest of LightCraft). No UI dependencies (L3).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
#[cfg(not(target_arch = "wasm32"))]
mod model;
#[cfg(not(target_arch = "wasm32"))]
mod stack;
#[cfg(not(target_arch = "wasm32"))]
mod vit;

use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
pub use model::Moge;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("scene geometry model: {0}")]
    Model(String),
    #[error("scene geometry model not found in {0} (expected model.pt)")]
    Missing(PathBuf),
    #[error("the photo is too small or too large to analyze ({0} × {1})")]
    Size(usize, usize),
    #[cfg(not(target_arch = "wasm32"))]
    #[error(transparent)]
    Candle(#[from] candle_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The checkpoint file of a model directory.
pub const WEIGHTS_FILE: &str = "model.pt";

/// Whether `dir` holds the checkpoint.
pub fn is_model_dir(dir: &Path) -> bool {
    dir.join(WEIGHTS_FILE).is_file()
}

/// Fewest and most ViT tokens an analysis may use (the model was trained on 1200–3600).
pub const TOKENS: (usize, usize) = (400, 3600);
/// Tokens of a default analysis: lighting is smooth, so the low end of the trained range is
/// enough, and it keeps the analysis to a few seconds on a CPU.
pub const DEFAULT_TOKENS: usize = 1500;
/// Longest edge of an image handed to [`Moge::infer`] (larger ones gain nothing: the model
/// resamples its input to the token grid).
pub const MAX_INPUT_EDGE: usize = 4096;

/// What the model sees in a photo, on a grid of `width × height` (16 times the token grid, about
/// 760 × 500 for a 3:2 photo at [`DEFAULT_TOKENS`]), row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
    /// Unit surface normals in the camera's space: x to the right, y down, z away from the camera (a
    /// surface facing the camera has z = −1).
    pub normal: Vec<[f32; 3]>,
    /// Depth along the view axis, positive, up to an unknown scale and shift (larger = farther).
    pub depth: Vec<f32>,
    /// 0..1: how sure the model is that there is a surface here (low on sky).
    pub surface: Vec<f32>,
}
