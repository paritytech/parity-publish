use std::path::Path;

use anyhow::{bail, Context, Result};
use cargo::core::{FeatureValue, Workspace};
use cargo::util::toml_mut::dependency::RegistrySource;
use cargo::util::toml_mut::manifest::LocalManifest;
use cargo::{core::dependency::DepKind, util::toml_mut::dependency::PathSource};
use toml_edit::Document;

use crate::plan::{Planner, RemoveDep, RemoveFeature, RewriteDep};

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct RemoveCrate {
    pub name: String,
}

pub fn rewrite_deps(
    workspace_path: &Path,
    plan: &Planner,
    manifest: &mut LocalManifest,
    deps: &[RewriteDep],
) -> Result<()> {
    for dep in deps {
        let exisiting_deps = manifest
            .get_dependency_versions(&dep.name)
            .collect::<Vec<_>>();

        let toml_name = exisiting_deps
            .iter()
            .find_map(|d| d.1.as_ref().ok())
            .context("coultnt find dep")?;
        let toml_name = toml_name.name.as_str();

        let mut new_ver = if let Some(v) = &dep.version {
            v.to_string()
        } else {
            plan.crates
                .iter()
                .find(|c| c.name == toml_name)
                .context("cant find package")?
                .to
                .clone()
        };

        if dep.exact {
            new_ver = format!("={}", new_ver);
        }

        for exisiting_dep in exisiting_deps {
            let (table, exisiting_dep) = exisiting_dep;
            let mut existing_dep = exisiting_dep?;
            let dev = table.kind() == DepKind::Development;

            if existing_dep.toml_key() == dep.name {
                let table = table
                    .to_table()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();

                if let Some(path) = &dep.path {
                    let path = workspace_path.canonicalize()?.join(path);
                    let mut source = PathSource::new(&path);

                    if dev {
                        existing_dep = existing_dep.clear_version();
                    } else {
                        source = source.set_version(&new_ver);
                    }
                    let existing_dep = existing_dep.set_source(source);
                    manifest.insert_into_table(&table, &existing_dep)?;
                } else {
                    let source = RegistrySource::new(&new_ver);
                    let existing_dep = existing_dep.set_source(source);
                    manifest.insert_into_table(&table, &existing_dep)?;
                }
            }
        }
    }

    Ok(())
}

pub fn remove_dep(
    workspace: &Workspace,
    manifest: &mut LocalManifest,
    dep: &RemoveDep,
) -> Result<()> {
    let mut removed = Vec::new();

    let exiting_deps = manifest
        .get_dependency_versions(&dep.name)
        .collect::<Vec<_>>();
    for (table, dep) in exiting_deps {
        let table = table
            .to_table()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if let Ok(dep) = dep {
            if !dep.optional.unwrap_or(false) {
                bail!(
                    "can't remove dependency {} from {} as it is not optional ({})",
                    dep.name,
                    manifest.package_name().unwrap_or("unknown"),
                    manifest.path.strip_prefix(workspace.root())?.display()
                );
            }
            manifest.remove_from_table(&table, dep.toml_key())?;
            removed.push(dep.toml_key().to_string());
        }
    }

    for dep in removed {
        remove_features_of_dep(manifest, &dep)?;
    }

    Ok(())
}

pub fn remove_features_of_dep(manifest: &mut LocalManifest, toml_key: &str) -> Result<()> {
    let features = manifest.manifest.get_table_mut(&["features".to_string()]);
    if let Ok(features) = features {
        let features = features.as_table_mut().context("not a table")?;

        for (_, value) in features.iter_mut() {
            let value = value.as_array_mut().context("not an array")?;
            value.retain(|v| {
                let v = v.as_str().unwrap();
                let feature = FeatureValue::new(v.into());
                match feature {
                    FeatureValue::Feature(_) => true,
                    FeatureValue::Dep { dep_name } => dep_name.as_str() != toml_key,
                    FeatureValue::DepFeature { dep_name, .. } => dep_name.as_str() != toml_key,
                }
            });
        }
    }

    Ok(())
}

pub fn remove_feature(manifest: &mut LocalManifest, remove_feature: &RemoveFeature) -> Result<()> {
    let features = manifest.manifest.get_table_mut(&["features".to_string()])?;
    let features = features.as_table_mut().context("not a table")?;

    if let Some(value) = &remove_feature.value {
        for feature in features.iter_mut() {
            if feature.0 == remove_feature.feature {
                let needs = feature.1.as_array_mut().unwrap();
                needs.retain(|need| need.as_str().unwrap() != value);
            }
        }
    } else {
        features.remove(&remove_feature.feature);
    }

    Ok(())
}

// hack because come crates don't have a desc
pub fn fix_description(manifest: &mut LocalManifest, name: &str) -> Result<()> {
    let package = manifest.manifest.get_table_mut(&["package".to_string()])?;

    if package.get("description").is_none() {
        package
            .as_table_mut()
            .unwrap()
            .insert("description", toml_edit_cargo::value(name));
    }

    Ok(())
}

pub fn set_version(manifest: &mut LocalManifest, new_ver: &str) -> Result<()> {
    let package = manifest.manifest.get_table_mut(&["package".to_string()])?;
    let ver = package.get_mut("version").unwrap();
    *ver = toml_edit_cargo::value(new_ver);
    Ok(())
}

pub fn remove_crate(
    workspace: &Workspace,
    manifest: &mut Document,
    remove_c: &RemoveCrate,
) -> Result<()> {
    let path = workspace
        .members()
        .find(|c| c.name().as_str() == remove_c.name)
        .map(|c| c.root());

    if let Some(path) = path {
        let path = path.strip_prefix(workspace.root())?;
        if let Some(workspace) = manifest.get_mut("workspace") {
            let workspace = workspace.as_table_mut().context("not a table")?;
            if let Some(members) = workspace.get_mut("members") {
                let members = members.as_array_mut().context("not an array")?;
                members.retain(|m| Path::new(m.as_str().unwrap()) != path)
            }
        }
    }
    Ok(())
}
