//! Downloading the MoGe-2 model, only when the user asked for it. The downloader itself (mirrors,
//! resuming, checks, the pure-Rust HTTPS client) is the `lightcraft-fetch` crate; this module
//! says what the model needs and where it may come from.
//!
//! The weights are never part of LightCraft: they are Microsoft's `moge-2-vits-normal`
//! checkpoint, published under the MIT License.

pub use lightcraft_fetch::*;

use std::path::Path;

/// Size of the official `model.pt` (134 MB).
pub const WEIGHTS_SIZE: u64 = 140_550_416;

/// The checkpoint [`crate::Moge::load`] needs, pinned to the official file (Hugging Face LFS SHA-256).
pub const FILES: &[FileSpec] = &[FileSpec {
    name: crate::WEIGHTS_FILE,
    size: Some(WEIGHTS_SIZE),
    sha256: Some("79a16621928c2bf0ed04659218c55c01075e950507f40bb3332fb4c873d3e1dc"),
    max: WEIGHTS_SIZE,
}];

/// Where the model is downloaded from, in order: each is a base URL, and file `f` is at
/// `<base>/<f>`. The model's own repository is public (not gated), and the file is verified
/// against [`FILES`] wherever it comes from.
pub const DEFAULT_MIRRORS: &[&str] = &["https://huggingface.co/Ruicheng/moge-2-vits-normal/resolve/main"];

/// Environment variable with extra mirrors (base URLs separated by commas, spaces or newlines),
/// tried before the defaults.
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_RELIEF_MIRRORS";

/// The mirrors to try, in order: the environment variable's, then the mirrors file's (one base
/// URL per line, `#` comments), then [`DEFAULT_MIRRORS`].
pub fn mirrors(env: Option<&str>, file: Option<&Path>) -> Vec<String> {
    lightcraft_fetch::mirrors(env, file, DEFAULT_MIRRORS)
}
