use std::{fs::read_to_string, path::Path};

use anyhow::{Context, Result};

use crate::plan::{RemoveDep, RemoveFeature};

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Crate {
    pub name: String,
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

pub fn read_config(path: &Path) -> Result<Config> {
    let path = path.join("Plan.config");

    if !path.exists() {
        return Ok(Default::default());
    }

    let config = read_to_string(path).context("failed to read Plan.config")?;
    let config = toml::from_str(&config)?;
    Ok(config)
}
