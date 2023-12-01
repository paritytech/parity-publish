use std::{fs::read_to_string, path::Path};

use anyhow::{Context, Result};

use crate::{plan::RemoveFeature, shared::*};

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Crate {
    pub remove_feature: Vec<RemoveFeature>,
    pub remove_dep: Vec<RemoveDep>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Config {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    #[serde(rename = "crate")]
    pub crates: Vec<Crate>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct RemoveDep {
    pub name: String,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub value: Option<String>,
}

pub fn read_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Default::default());
    }

    let config = read_to_string(path).context("failed to read Plan.config")?;
    let config = toml::from_str(&config)?;
    Ok(config)
}
