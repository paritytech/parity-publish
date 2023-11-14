use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::PathBuf,
};

use anyhow::{Context, Result};
use cargo::core::{dependency::DepKind, FeatureValue, Package, Summary, Workspace};
use semver::{BuildMetadata, Prerelease, Version};
use termcolor::{ColorChoice, StandardStream};

use crate::{
    changed, check,
    cli::{Check, Plan},
    registry,
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
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub exact: bool,
    pub path: Option<PathBuf>,
    //pub dev: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct RemoveDep {
    pub name: String,
    pub package: Option<String>,
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

    if !plan.skip_check {
        check::check(Check {
            path: plan.path.clone(),
            allow_nonfatal: true,
            allow_unpublished: false,
            no_check_owner: false,
            recursive: false,
        })
        .await?;
    }

    let changed = if let Some(from) = &plan.changed {
        let changed = changed::get_changed_crates(&workspace, from, "HEAD")?;
        let indirect = changed
            .iter()
            .filter(|c| matches!(c.kind, changed::ChangeKind::Dependency))
            .count();
        writeln!(
            stdout,
            "{} packages changed {} indirect",
            changed.len(),
            indirect
        )?;
        changed.into_iter().map(|c| c.name).collect()
    } else {
        BTreeSet::new()
    };

    let order = order(&mut stdout, &workspace)?;

    let _lock = config.acquire_package_cache_lock()?;
    let mut reg = registry::get_registry(&workspace)?;

    writeln!(stdout, "looking up crates...",)?;
    registry::download_crates(&mut reg, &workspace, true)?;

    for c in workspace.members().filter(|c| c.publish().is_none()) {
        upstream.insert(
            c.name().to_string(),
            registry::get_crate(&mut reg, c.name()).unwrap(),
        );
    }

    let workspace_crates = workspace
        .members()
        .map(|m| (m.name().as_str(), m))
        .collect::<BTreeMap<_, _>>();

    writeln!(stdout, "calculating plan...")?;

    let planner = calculate_plan(&plan, order, &mut upstream, workspace_crates, &changed).await?;

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

async fn calculate_plan(
    plan: &Plan,
    order: Vec<&str>,
    upstream: &BTreeMap<String, Vec<Summary>>,
    workspace_crates: BTreeMap<&str, &Package>,
    changed: &BTreeSet<String>,
) -> Result<Planner> {
    let manifest_path = plan.path.canonicalize()?.join("Cargo.toml");
    let old_plan = old_plan(plan);
    let mut planner = Planner::default();
    let mut new_versions = BTreeMap::new();
    let mut breaking: BTreeSet<String> = BTreeSet::new();

    for c in order {
        let upstreamc = upstream.get(c);
        let c = *workspace_crates.get(c).unwrap();
        let mut rewrite = Vec::new();

        let mut publish = is_publish(plan, c, &breaking, changed)?;

        let (from, to) = get_versions(plan, upstreamc, c, publish, &mut breaking, &old_plan)?;

        // if the version is already taken assume it's from a previous pre release and use this
        // version instead of making a new release
        if let Some(upstreamc) = upstreamc {
            if upstreamc.iter().any(|u| u.version() == &to) && !to.pre.is_empty() {
                publish = false;
            }
        }

        new_versions.insert(c.name().to_string(), to.to_string());

        rewrite_deps(plan, c, &workspace_crates, upstream, &mut rewrite).await?;

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
    Ok(planner)
}

fn get_versions(
    plan: &Plan,
    upstreamc: Option<&Vec<Summary>>,
    c: &Package,
    publish: bool,
    breaking: &mut BTreeSet<String>,
    old_plan: &Planner,
) -> Result<(Version, Version)> {
    let from = upstreamc
        .and_then(|u| max_ver(u, plan.pre.is_some()))
        .map(|u| u.version().clone())
        .unwrap_or(Version::parse("0.1.0").unwrap());

    if let Some(oldc) = old_plan
        .crates
        .iter()
        .find(|cr| cr.name == c.name().as_str())
    {
        return Ok((Version::parse(&oldc.from)?, Version::parse(&oldc.to)?));
    }

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
    plan: &Plan,
    c: &Package,
    breaking: &BTreeSet<String>,
    changed: &BTreeSet<String>,
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

    if changed.contains(c.name().as_str()) {
        return Ok(true);
    }

    Ok(false)
}

async fn rewrite_deps(
    plan: &Plan,
    cra: &Package,
    workspace_crates: &BTreeMap<&str, &Package>,
    upstream: &BTreeMap<String, Vec<Summary>>,
    rewrite: &mut Vec<RewriteDep>,
) -> Result<()> {
    for dep in cra.dependencies() {
        if dep.source_id().is_git() || dep.source_id().is_path() {
            if let Some(dep_crate) = workspace_crates.get(dep.package_name().as_str()) {
                let path = plan.path.canonicalize()?;

                //let package_name = (dep.name_in_toml() != dep.package_name())
                //    .then(|| dep.package_name().to_string());

                let version = if dep.source_id().is_git() {
                    let version = upstream
                        .get(dep.package_name().as_str())
                        .and_then(|c| max_ver(c, false))
                        .with_context(|| {
                            format!("crate {} has no crates.io release", dep.package_name())
                        })?
                        .version();
                    Some(version.to_string())
                } else {
                    None
                };

                rewrite.push(RewriteDep {
                    name: dep.name_in_toml().to_string(),
                    version,
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

fn order<'a>(stdout: &mut StandardStream, workspace: &'a Workspace) -> Result<Vec<&'a str>> {
    writeln!(stdout, "calculating order...")?;

    let mut deps = BTreeMap::new();
    let mut order = Vec::new();

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

    Ok(order)
}

fn old_plan(plan: &Plan) -> Planner {
    if plan.new {
        Default::default()
    } else {
        std::fs::read_to_string(plan.path.join("Plan.toml"))
            .ok()
            .and_then(|plan| toml::from_str(&plan).ok())
            .unwrap_or_default()
    }
}

fn max_ver(crates: &[Summary], pre: bool) -> Option<&Summary> {
    crates
        .iter()
        .filter(|c| pre || c.version().pre.is_empty())
        .max_by_key(|c| c.version())
}
