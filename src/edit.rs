use std::default;
use std::fs::read_to_string;
use std::path::Path;

use anyhow::{Context, Result};
use cargo::core::{FeatureValue, Workspace};
use cargo::util::toml_mut::dependency::{Dependency, RegistrySource};
use cargo::util::toml_mut::manifest::LocalManifest;
use cargo::{core::dependency::DepKind, util::toml_mut::dependency::PathSource};
use semver::Version;
use toml_edit::{DocumentMut, Formatted};

use crate::plan::{Planner, RemoveCrate, RemoveDep, RemoveFeature, RewriteDep};

pub fn rewrite_workspace_dep(
    _workspace_path: &Path,
    plan: &Planner,
    root_manifest: &mut DocumentMut,
    dep: &RewriteDep,
    cdep: &mut Dependency,
    dev: bool,
) -> Result<()> {
    let wdeps = root_manifest
        .get_mut("workspace")
        .unwrap()
        .get_mut("dependencies")
        .unwrap();

    let wdep = wdeps.get_mut(&dep.name).unwrap();
    let name = if let Some(package) = wdep.get("package") {
        package.as_value().unwrap().as_str().unwrap()
    } else {
        dep.name.as_str()
    };
    let new_ver = if let Some(v) = &dep.version {
        v.to_string()
    } else {
        plan.crates
            .iter()
            .find(|c| c.name == name)
            .context("cant find package ".to_string() + name)?
            .to
            .clone()
    };

    if dev {
        let default_features = wdep.get("default-features").map(|d| d.as_bool().unwrap());
        let path = Path::new(wdep.get("path").unwrap().as_str().unwrap())
            .canonicalize()
            .unwrap();
        let source = PathSource::new(&path);
        *cdep = cdep.clone().set_source(source);
        if default_features == Some(false) && cdep.default_features != Some(true) {
            *cdep = cdep.clone().set_default_features(false);
        }
        if dep.name != name {
            cdep.name = name.to_string();
            *cdep = cdep.clone().set_rename(&dep.name);
        }
    } else {
        let wdep = wdep.as_inline_table_mut().unwrap();
        wdep.insert("version", toml_edit::Value::String(Formatted::new(new_ver)));
        wdep.fmt();
    }
    Ok(())
}

pub fn rewrite_deps(
    workspace_path: &Path,
    plan: &Planner,
    root_manifest: &mut DocumentMut,
    manifest: &mut LocalManifest,
    deps: &[RewriteDep],
) -> Result<()> {
    for dep in deps {
        let exisiting_deps = manifest
            .get_dependency_versions(&dep.name)
            .collect::<Vec<_>>();

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

                let is_workspace = existing_dep
                    .source()
                    .map_or(false, |d| d.as_workspace().is_some());
                if is_workspace {
                    rewrite_workspace_dep(
                        workspace_path,
                        plan,
                        root_manifest,
                        dep,
                        &mut existing_dep,
                        dev,
                    )?;
                    manifest.insert_into_table(&table, &existing_dep)?;
                    continue;
                }

                let mut new_ver = if let Some(v) = &dep.version {
                    v.to_string()
                } else {
                    plan.crates
                        .iter()
                        .find(|c| c.name == existing_dep.name.as_str())
                        .context("cant find package ".to_string() + existing_dep.name.as_str())?
                        .to
                        .clone()
                };
                if !Version::parse(&new_ver).unwrap().pre.is_empty() {
                    new_ver = format!("={}", new_ver);
                }

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
    root_manifest: &mut DocumentMut,
    manifest: &mut LocalManifest,
    dep: &RemoveDep,
) -> Result<()> {
    remove_dep_inner(workspace, root_manifest, manifest, dep)?;
    Ok(())
}

pub fn remove_dep_inner(
    workspace: &Workspace,
    root_manifest: &mut DocumentMut,
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
                let remove = RemoveCrate {
                    name: manifest.package_name()?.to_string(),
                };
                remove_crate_inner(workspace, root_manifest, &remove)?;
            } else {
                manifest.remove_from_table(&table, dep.toml_key())?;
                removed.push(dep.toml_key().to_string());
            }
        }
    }

    manifest.write()?;

    for dep in removed {
        remove_features_of_dep(workspace, root_manifest, manifest, &dep)?;
    }

    Ok(())
}

pub fn remove_features_of_dep(
    workspace: &Workspace,
    root_manifest: &mut DocumentMut,
    manifest: &mut LocalManifest,
    toml_key: &str,
) -> Result<()> {
    let mut remove = Vec::new();
    let package_name = manifest.package_name()?.to_string();
    let features = manifest.manifest.get_table_mut(&["features".to_string()]);
    if let Ok(features) = features {
        let features = features.as_table_mut().context("not a table")?;

        for (key, value) in features.iter_mut() {
            let value = value.as_array_mut().context("not an array")?;

            // We don't really know if we should remove the whole feature line or just the
            // part of the feature that references the dep we deleted.
            //
            // If the feature enables code that references the dep then not removing the whole
            // feature would mean if that dep is enabled the code would not compile.
            //
            // So only remove the whole feature if the feature unconditionally enabled the dep
            // otherwise just remove the one part.
            let dep_value = value.iter().any(|v| {
                let v = v.as_str().unwrap();
                let feature = FeatureValue::new(v.into());
                matches!(feature, FeatureValue::Dep { dep_name } if dep_name.as_str() == toml_key)
            });
            if dep_value {
                remove.push(key.get().to_string());
            } else {
                value.retain(|v| {
                    let v = v.as_str().unwrap();
                    let feature = FeatureValue::new(v.into());
                    match feature {
                        FeatureValue::Feature(dep_name) => {
                            if dep_name.as_str() == toml_key {
                                remove.push(key.get().to_string());
                            }
                            true
                        }
                        FeatureValue::Dep { dep_name } => {
                            if dep_name.as_str() == toml_key {
                                remove.push(key.get().to_string());
                            }
                            true
                        }
                        FeatureValue::DepFeature { dep_name, weak, .. } => {
                            if dep_name.as_str() == toml_key {
                                if !weak {
                                    remove.push(key.get().to_string());
                                }
                                false
                            } else {
                                true
                            }
                        }
                    }
                });
            }
        }
    }

    remove.dedup();

    let features = manifest.manifest.get_table_mut(&["features".to_string()]);
    if let Ok(features) = features {
        let features = features.as_table_mut().context("not a table")?;
        for key in remove {
            remove_dep_feature_all(workspace, root_manifest, &package_name, &key)?;
            features.remove(&key);
        }
    }

    manifest.write()?;

    Ok(())
}

pub fn remove_dep_feature_all(
    workspace: &Workspace,
    root_manifest: &mut DocumentMut,
    name: &str,
    value: &str,
) -> Result<()> {
    for c in workspace.members() {
        let mut remove = Vec::new();
        let mut manifest = LocalManifest::try_new(c.manifest_path())?;

        for (table, dep) in manifest.get_dependency_versions(name) {
            if table.kind() == DepKind::Development {
                continue;
            }

            let dep = dep?;
            if let Some(features) = &dep.features {
                if features.contains(value) {
                    remove_crate_inner(
                        workspace,
                        root_manifest,
                        &RemoveCrate {
                            name: c.name().to_string(),
                        },
                    )?;
                }
            }
        }

        let features = manifest.manifest.get_table_mut(&["features".to_string()])?;
        let features = features.as_table_mut().context("not a table")?;

        for (key, feature) in features.iter() {
            let feature = feature.as_array().context("not an array")?;
            for feature in feature {
                let feature = feature.as_str().context("not a string")?;
                let feature = FeatureValue::new(feature.into());
                if matches!(feature, FeatureValue::DepFeature { dep_name, dep_feature, .. } if dep_name.as_str() == name && dep_feature.as_str() == value)
                {
                    remove.push(key.to_string());
                }
            }
        }

        for key in &remove {
            features.remove(key);
        }

        manifest.write()?;

        for key in remove {
            remove_dep_feature_all(workspace, root_manifest, c.name().as_str(), &key)?;
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
pub fn set_description(plan: &Planner, manifest: &mut LocalManifest, name: &str) -> Result<()> {
    let package = manifest.manifest.get_table_mut(&["package".to_string()])?;

    let mut desc = if let Some(desc) = package.get("description") {
        desc.as_str().unwrap().to_string()
    } else {
        name.to_string()
    };

    if let Some(suffix) = &plan.options.description {
        let suffix = format!(" ({})", suffix);
        if !desc.ends_with(&suffix) {
            desc.push_str(&suffix);
        }
    }

    package
        .as_table_mut()
        .unwrap()
        .insert("description", toml_edit::value(desc));

    Ok(())
}

pub fn set_version(manifest: &mut LocalManifest, new_ver: &str) -> Result<()> {
    let package = manifest.manifest.get_table_mut(&["package".to_string()])?;
    let ver = package.get_mut("version").unwrap();
    *ver = toml_edit::value(new_ver);
    Ok(())
}

pub fn remove_crate(workspace: &Workspace, remove_c: &RemoveCrate) -> Result<()> {
    let root_manifest = read_to_string(workspace.root_manifest())?;
    let mut root_manifest: DocumentMut = root_manifest.parse()?;
    remove_crate_inner(workspace, &mut root_manifest, remove_c)?;
    let root_manifest = root_manifest.to_string();
    std::fs::write(workspace.root_manifest(), &root_manifest)?;
    Ok(())
}

pub fn remove_crate_inner(
    workspace: &Workspace,
    manifest: &mut DocumentMut,
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

    remove_dep_all(&workspace, manifest, &remove_c.name)?;
    Ok(())
}

pub fn remove_dep_all(
    workspace: &Workspace,
    root_manifest: &mut DocumentMut,
    remove_c: &str,
) -> Result<()> {
    for c in workspace.members() {
        if c.dependencies()
            .iter()
            .any(|d| d.package_name() == remove_c)
        {
            let mut manifest = LocalManifest::try_new(c.manifest_path())?;
            remove_dep_inner(
                workspace,
                root_manifest,
                &mut manifest,
                &RemoveDep {
                    name: remove_c.to_string(),
                    package: None,
                },
            )?;
        }
    }
    Ok(())
}
