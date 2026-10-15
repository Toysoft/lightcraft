//! The crop tool's aspect-ratio list (issue #1854): the built-in ratios, which of them the user
//! hid, and the ratios the user saved under a name, kept with the library preferences.

use serde::{Deserialize, Serialize};

use super::CommandSpec;

/// A ratio the user saved under a name (`[width, height]`, any positive numbers).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomAspect {
    pub name: String,
    pub aspect: [f64; 2],
}

/// The user's part of the aspect menu: saved ratios in menu order, hidden built-in keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropAspects {
    pub custom: Vec<CustomAspect>,
    pub hidden: Vec<String>,
}

pub fn specs() -> Vec<CommandSpec> {
    Vec::new()
}
