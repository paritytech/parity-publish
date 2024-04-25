use std::{env::current_dir, fs::read_to_string, path::Path};

use anyhow::{Context, Result};
use cargo::{core::Workspace, util::toml_mut::manifest::LocalManifest};

use crate::{
    cli::{self, Args},
    edit,
    plan::{RemoveCrate, RemoveDep, RemoveFeature},
};

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Crate {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub remove_feature: Vec<RemoveFeature>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub remove_dep: Vec<RemoveDep>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Config {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    #[serde(rename = "crate")]
    pub crates: Vec<Crate>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    #[serde(rename = "remove_crate")]
    pub remove_crates: Vec<RemoveCrate>,
}

pub fn handle_config(_args: Args, cli: cli::Config) -> Result<()> {
    let path = current_dir()?;
    let config = read_config(&path)?;

    let cargo_config = cargo::Config::default()?;
    cargo_config
        .shell()
        .set_verbosity(cargo::core::Verbosity::Quiet);

    let workspace = Workspace::new(&path.join("Cargo.toml"), &cargo_config)?;

    if cli.apply {
        apply_config(&workspace, &config)?;
    }

    Ok(())
}

pub fn apply_config(workspace: &Workspace, config: &Config) -> Result<()> {
    for pkg in &config.remove_crates {
        edit::remove_crate(&workspace, pkg)?;
    }

    for pkg in &config.crates {
        let c = workspace
            .members()
            .find(|c| c.name().as_str() == pkg.name)
            .context("can't find crate")?;
        let path = c.root();
        let mut manifest = LocalManifest::try_new(&path.join(path).join("Cargo.toml"))?;

        for remove_feature in &pkg.remove_feature {
            edit::remove_feature(&mut manifest, remove_feature)?;
        }

        for remove_dep in &pkg.remove_dep {
            edit::remove_dep(&workspace, &mut manifest, remove_dep)?;
        }

        manifest.write()?;
    }

    Ok(())
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
