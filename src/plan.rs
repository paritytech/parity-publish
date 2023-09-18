use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::PathBuf,
};

use anyhow::{ensure, Context, Result};
use cargo::{
    core::{dependency::DepKind, FeatureValue, Package, Workspace},
    Config,
};
use crates_io_api::{AsyncClient, FullCrate};
use semver::{BuildMetadata, Prerelease, Version};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{
    changed::{diff_crate, download_crates},
    cli::Plan,
    shared::{self, parity_crate_owner_id},
};

fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

fn is_not_default<T: Default + PartialEq>(t: &T) -> bool {
    *t != Default::default()
}

fn bool_true() -> bool {
    true
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Planner {
    #[serde(rename = "crate")]
    pub crates: Vec<Publish>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Publish {
    #[serde(default = "bool_true")]
    #[serde(skip_serializing_if = "is_not_default")]
    pub publish: bool,
    pub name: String,
    pub path: PathBuf,
    pub from: String,
    pub to: String,
    pub bump: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub rewrite_dep: Vec<RewriteDep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub remove_feature: Vec<RemoveFeature>,
    #[serde(skip_serializing_if = "is_not_default")]
    #[serde(default = "bool_true")]
    pub verify: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct RewriteDep {
    pub name: String,
    pub package: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub exact: bool,
    pub path: Option<PathBuf>,
    //pub dev: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct RemoveFeature {
    pub feature: String,
    pub value: String,
}

pub async fn handle_plan(plan: Plan) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let manifest_path = plan.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&manifest_path, &config)?;
    let mut upstream = BTreeMap::new();

    let cratesio = shared::cratesio()?;
    let workspace_crates = workspace
        .members()
        .map(|m| (m.name().as_str(), m))
        .collect::<BTreeMap<_, _>>();

    let mut deps = BTreeMap::new();
    let mut order = Vec::new();
    let mut own_all = true;

    writeln!(stdout, "looking up crates...",)?;

    let cache_path = plan.path.join("Plan.cache");
    if plan.cache && cache_path.exists() && !plan.refresh {
        upstream = toml::from_str(&fs::read_to_string(&cache_path)?)?;
    }

    for package in workspace_crates.values() {
        let c = if let Some(c) = upstream.remove(package.name().as_str()) {
            c
        } else if let Ok(c) = cratesio.full_crate(package.name().as_str(), true).await {
            c
        } else {
            continue;
        };

        let parity_own = c
            .owners
            .iter()
            .any(|user| user.id == parity_crate_owner_id())
            || package.publish().is_some();
        if !parity_own {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
            writeln!(
                stdout,
                "{} exists and is owned by someone else",
                package.name()
            )?;
            stdout.set_color(ColorSpec::new().set_fg(None))?;
        }
        upstream.insert(package.name().to_string(), c);
        own_all &= parity_own;
    }

    if plan.cache {
        fs::write(&cache_path, toml::to_string_pretty(&upstream)?)?;
    }
    //new_versions.insert(c.name().to_string(), to.to_string());

    ensure!(own_all, "we do not own all crates in the workspace");

    download_crates(&workspace, &upstream.values().cloned().collect::<Vec<_>>()).await?;

    writeln!(stdout, "calculating order...")?;

    // map name to deps
    for member in workspace.members() {
        let deps_list = member
            .dependencies()
            .iter()
            .filter(|d| d.kind() != DepKind::Development)
            .collect::<Vec<_>>();
        deps.insert(member.name().as_str(), deps_list);
    }

    let mut names = workspace
        .members()
        .map(|c| c.name())
        .collect::<BTreeSet<_>>();

    while !deps.is_empty() {
        // strip out deps that are not in the workspace
        for deps in deps.values_mut() {
            deps.retain(|dep| names.contains(dep.package_name().as_str()))
        }

        deps.retain(|name, deps| {
            if deps.is_empty() {
                order.push(*name);
                names.remove(*name);
                false
            } else {
                true
            }
        });
    }

    writeln!(stdout, "calculating plan...")?;

    let mut planner = Planner::default();
    let mut new_versions = BTreeMap::new();
    let mut breaking: BTreeSet<String> = BTreeSet::new();

    for c in order {
        let upstreamc = upstream.get(c);
        let c = *workspace_crates.get(c).unwrap();
        let mut rewrite = Vec::new();

        let mut publish = is_publish(&config, &plan, upstreamc, c, &breaking)?;

        let (from, to) = get_versions(&config, &plan, upstreamc, c, publish, &mut breaking)?;

        // if the version is already taken assume it's from a previous pre release and use this
        // version instead of making a new release
        if let Some(upstreamc) = upstreamc {
            if upstreamc.versions.iter().any(|v| v.num == to.to_string()) && !to.pre.is_empty() {
                publish = false;
            }
        }

        new_versions.insert(c.name().to_string(), to.to_string());

        rewrite_deps(
            &plan,
            &cratesio,
            c,
            &workspace_crates,
            &mut upstream,
            &mut rewrite,
        )
        .await?;

        let remove = remove_features(c);

        planner.crates.push(Publish {
            publish,
            name: c.name().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            bump: "unknown".to_string(),
            reason: "changed".to_string(),
            rewrite_dep: rewrite,
            path: c
                .manifest_path()
                .parent()
                .unwrap()
                .strip_prefix(manifest_path.parent().unwrap())
                .unwrap()
                .to_path_buf(),
            remove_feature: remove,
            verify: !plan.no_verify,
        });
    }

    if plan.cache {
        fs::write(&cache_path, toml::to_string_pretty(&upstream)?)?;
    }

    let output = toml::to_string_pretty(&planner)?;
    std::fs::write(plan.path.join("Plan.toml"), output)?;
    writeln!(
        stdout,
        "plan generated {} packages {} to publish",
        planner.crates.len(),
        planner.crates.iter().filter(|c| c.publish).count()
    )?;

    Ok(())
}

fn get_versions(
    _config: &Config,
    plan: &Plan,
    upstreamc: Option<&FullCrate>,
    c: &Package,
    publish: bool,
    breaking: &mut BTreeSet<String>,
) -> Result<(Version, Version)> {
    let from = upstreamc
        .and_then(|u| u.max_stable_version.as_deref())
        .unwrap_or("0.0.0");

    let from = Version::parse(from).unwrap();
    let mut to = from.clone();

    if !publish {
        return Ok((from, to));
    }

    // support also setting a version via Cargo.toml
    //
    // if the version in Cargo.toml is > than the last crates.io release we should use it.
    // if the version in Cargo.timl is > but still compatible we should major bump it to be
    // safe

    if c.version() > &from {
        let mut v = c.version().clone();
        v.pre = Prerelease::EMPTY;
        to = v;
    }

    let compatible = if to.major != 0 {
        to.major == from.major
    } else {
        to.minor == from.minor
    };

    if compatible {
        to.pre = Prerelease::EMPTY;
        to.build = BuildMetadata::EMPTY;

        if let Some(ref pre) = plan.pre {
            to.pre = Prerelease::new(pre).unwrap();
        }

        if to.major == 0 {
            to.minor += 1;
            to.patch = 0;
        } else {
            to.major += 1;
            to.minor = 0;
            to.patch = 0;
        }

        breaking.insert(c.name().to_string());
    }

    Ok((from, to))
}

fn is_publish(
    config: &Config,
    plan: &Plan,
    upstreamc: Option<&crates_io_api::FullCrate>,
    c: &Package,
    breaking: &BTreeSet<String>,
) -> Result<bool> {
    if c.publish().is_some() {
        return Ok(false);
    }

    if plan.all {
        return Ok(true);
    }

    if plan.crates.iter().any(|p| p == c.name().as_str()) {
        return Ok(true);
    }

    if c.dependencies()
        .iter()
        .filter(|d| d.kind() != DepKind::Development)
        .any(|d| breaking.contains(d.package_name().as_str()))
    {
        return Ok(true);
    }

    if plan.changed {
        if let Some(ver) = upstreamc.and_then(|u| u.max_stable_version.as_ref()) {
            return diff_crate(false, config, c, ver);
        } else {
            return Ok(true);
        }
    }

    Ok(false)
}

async fn rewrite_deps(
    plan: &Plan,
    cratesio: &AsyncClient,
    cra: &Package,
    workspace_crates: &BTreeMap<&str, &Package>,
    upstream: &mut BTreeMap<String, crates_io_api::FullCrate>,
    rewrite: &mut Vec<RewriteDep>,
) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    for dep in cra.dependencies() {
        if dep.source_id().is_git() || dep.source_id().is_path() {
            if let Some(dep_crate) = workspace_crates.get(dep.package_name().as_str()) {
                let path = plan.path.canonicalize()?;
                let package_name = if dep.name_in_toml() == dep.package_name() {
                    None
                } else {
                    Some(dep.package_name().to_string())
                };

                rewrite.push(RewriteDep {
                    name: dep.name_in_toml().to_string(),
                    package: package_name,
                    version: None,
                    exact: plan.exact,
                    path: Some(
                        dep_crate
                            .manifest_path()
                            .parent()
                            .unwrap()
                            .strip_prefix(path)
                            .unwrap()
                            .to_path_buf(),
                    ),
                })
            } else {
                let u = if let Some(u) = upstream.get(&dep.package_name().to_string()) {
                    Some(u)
                } else {
                    writeln!(stdout, "looking up {}...", dep.package_name())?;
                    let u = cratesio
                        .full_crate(dep.package_name().as_str(), true)
                        .await
                        .ok();
                    if let Some(u) = u {
                        upstream.insert(dep.package_name().to_string(), u);
                        upstream.get(dep.package_name().as_str())
                    } else {
                        None
                    }
                };

                if let Some(u) = u {
                    if u.versions.iter().all(|v| v.yanked) {
                        continue;
                    }

                    let new_ver = if plan.pre.is_some() {
                        format!("{}", u.max_version)
                    } else {
                        u.max_stable_version.clone().with_context(|| {
                            format!("crate {} does not have a release", dep.package_name())
                        })?
                    };

                    let package_name = if dep.name_in_toml() == dep.package_name() {
                        None
                    } else {
                        Some(dep.package_name().to_string())
                    };

                    rewrite.push(RewriteDep {
                        name: dep.name_in_toml().to_string(),
                        package: package_name,
                        version: Some(new_ver),
                        exact: plan.exact,
                        path: None,
                    });
                }
            }
        }
    }

    Ok(())
}

fn remove_features(member: &Package) -> Vec<RemoveFeature> {
    let mut remove = Vec::new();
    let mut dev = BTreeSet::new();
    let mut non_dev = BTreeSet::new();

    for dep in member.dependencies() {
        if dep.kind() == DepKind::Development {
            dev.insert(dep.name_in_toml());
        } else {
            non_dev.insert(dep.name_in_toml());
        }
    }

    for feature in non_dev {
        dev.remove(&feature);
    }

    for (feature, needs) in member.summary().features() {
        for need in needs {
            let dep_name = match need {
                FeatureValue::Feature(_) => continue,
                FeatureValue::Dep { dep_name } => dep_name.as_str(),
                FeatureValue::DepFeature {
                    dep_name,
                    dep_feature: _,
                    weak: _,
                } => dep_name.as_str(),
            };

            if dev.contains(dep_name) {
                remove.push(RemoveFeature {
                    feature: feature.to_string(),
                    value: need.to_string(),
                });
            }
        }
    }

    remove
}
