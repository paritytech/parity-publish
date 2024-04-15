use std::{
    collections::{BTreeMap, BTreeSet},
    env::args,
    fmt::Display,
    io::Write,
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use cargo::{
    core::{dependency::DepKind, Package, Workspace},
    sources::IndexSummary,
    util::cache_lock::CacheLockMode,
};
use semver::{BuildMetadata, Prerelease, Version};
use termcolor::{ColorChoice, StandardStream};
use toml_edit::DocumentMut;

use crate::{
    changed, check,
    cli::{Check, Plan},
    prdoc, registry,
    shared::*,
};

#[derive(
    serde::Serialize, serde::Deserialize, Default, PartialEq, Eq, PartialOrd, Ord, Copy, Clone,
)]
pub enum BumpKind {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "patch")]
    Patch,
    #[serde(rename = "minor")]
    Minor,
    #[serde(rename = "major")]
    Major,
}

impl Display for BumpKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BumpKind::None => f.write_str("None"),
            BumpKind::Major => f.write_str("Major"),
            BumpKind::Minor => f.write_str("Minor"),
            BumpKind::Patch => f.write_str("Patch"),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum PublishReason {
    #[serde(rename = "bumped by --patch")]
    Bumped,
    #[serde(rename = "manually specified")]
    Specified,
    #[serde(rename = "changed")]
    Changed,
    #[serde(rename = "--all was specified")]
    All,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Options {
    pub description: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Planner {
    #[serde(default)]
    pub options: Options,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    #[serde(rename = "crate")]
    pub crates: Vec<Publish>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    #[serde(rename = "remove_crate")]
    pub remove_crates: Vec<RemoveCrate>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Publish {
    pub name: String,
    pub from: String,
    pub to: String,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub bump: BumpKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub reason: Option<PublishReason>,
    #[serde(default = "bool_true")]
    #[serde(skip_serializing_if = "is_not_default")]
    pub publish: bool,
    #[serde(skip_serializing_if = "is_not_default")]
    #[serde(default = "bool_true")]
    pub verify: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub rewrite_dep: Vec<RewriteDep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub remove_dep: Vec<RemoveDep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub remove_feature: Vec<RemoveFeature>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct RewriteDep {
    pub name: String,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default, PartialOrd, Ord, PartialEq, Eq)]
pub struct RemoveDep {
    pub name: String,
    pub package: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct RemoveFeature {
    pub feature: String,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Eq, PartialEq)]
pub struct RemoveCrate {
    pub name: String,
}

pub async fn handle_plan(mut plan: Plan) -> Result<()> {
    read_stdin(&mut plan.crates)?;
    if plan.patch {
        patch_bump(&plan)
    } else {
        generate_plan(&plan).await
    }
}

pub fn patch_bump(plan: &Plan) -> Result<()> {
    let mut planner = read_plan(plan)
        .context("Can't read Plan.toml")?
        .context("Plan.toml does not exist")?;

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let manifest_path = plan.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&manifest_path, &config)?;

    for package in &plan.crates {
        let c = planner.crates.iter_mut().find(|c| c.name == *package);

        let Some(c) = c else {
            continue;
        };

        //.with_context(|| format!("could not find crate '{}' in Plan.toml", package))?;

        if !c.publish {
            bail!("crate '{}' is set not to publish", package);
        }

        c.from = c.to.clone();
        let mut to = Version::parse(&c.from)?;
        to.patch += 1;
        c.to = to.to_string();
        c.bump = BumpKind::Minor;
        c.reason = Some(PublishReason::Bumped);
    }

    write_plan(plan, &workspace, &planner)?;

    Ok(())
}

pub async fn generate_plan(plan: &Plan) -> Result<()> {
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
            quiet: false,
            paths: 0,
        })
        .await?;
    }

    let changed = if let Some(from) = &plan.since {
        let changed = changed::get_changed_crates(&workspace, true, from, "HEAD")?;
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
    } else if let Some(path) = &plan.prdoc {
        let changed = prdoc::get_prdocs(&workspace, path, true, &[])?;
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

    let _lock = workspace
        .config()
        .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(&workspace)?;

    writeln!(stdout, "looking up crates...",)?;
    registry::download_crates(&mut reg, &workspace, true)?;

    for c in workspace.members().filter(|c| c.publish().is_none()) {
        upstream.insert(
            c.name().to_string(),
            registry::get_crate(&mut reg, c.name()).unwrap(),
        );
        for dep in c.dependencies() {
            if dep.source_id().is_git() || dep.source_id().is_path() {
                if let Ok(package) = registry::get_crate(&mut reg, dep.package_name()) {
                    upstream.insert(dep.package_name().to_string(), package);
                }
            }
        }
    }

    let workspace_crates = workspace
        .members()
        .map(|m| (m.name().as_str(), m))
        .collect::<BTreeMap<_, _>>();

    writeln!(stdout, "calculating plan...")?;

    let planner = calculate_plan(&plan, order, &upstream, workspace_crates, &changed).await?;

    write_plan(plan, &workspace, &planner)?;
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
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
    workspace_crates: BTreeMap<&str, &Package>,
    changed: &BTreeSet<String>,
) -> Result<Planner> {
    let old_plan = read_plan(plan)?.unwrap_or_default();
    let mut planner = Planner::default();
    let mut new_versions = BTreeMap::new();

    planner.options = old_plan.options;

    if plan.description.is_some() {
        planner.options.description = plan.description.clone();
    }

    for c in order {
        let upstreamc = upstream.get(c);
        let old_crate = old_plan.crates.iter().find(|old| old.name == c);
        let c = *workspace_crates.get(c).unwrap();

        let mut publish_reason = is_publish(plan, c, changed)?;

        let (from, to) = get_versions(plan, upstreamc, c, publish_reason.is_some(), old_crate)?;

        // if the version is already taken assume it's from a previous pre release and use this
        // version instead of making a new release
        if let Some(upstreamc) = upstreamc {
            if upstreamc.iter().any(|u| u.as_summary().version() == &to) && !to.pre.is_empty() {
                publish_reason = None;
            }
        }

        new_versions.insert(c.name().to_string(), to.to_string());

        let mut rewrite_deps = old_crate.map(|c| c.rewrite_dep.clone()).unwrap_or_default();
        for dep in rewrite_git_deps(c, &workspace_crates, upstream).await? {
            if !rewrite_deps.iter().any(|d| d.name == dep.name) {
                rewrite_deps.push(dep);
            }
        }

        let remove_deps =
            remove_git_deps(c, &workspace_crates, upstream, &mut planner.remove_crates);

        planner.crates.push(Publish {
            publish: publish_reason.is_some(),
            name: c.name().to_string(),
            from: from.to_string(),
            to: to.to_string(),
            bump: BumpKind::Major,
            reason: publish_reason,
            rewrite_dep: rewrite_deps,
            remove_feature: old_crate
                .map(|c| c.remove_feature.clone())
                .unwrap_or_default(),
            remove_dep: remove_deps,
            verify: !plan.no_verify,
        });
    }
    Ok(planner)
}

fn get_versions(
    plan: &Plan,
    upstreamc: Option<&Vec<IndexSummary>>,
    c: &Package,
    publish: bool,
    old_crate: Option<&Publish>,
) -> Result<(Version, Version)> {
    let from = upstreamc
        .and_then(|u| max_ver(u, plan.pre.is_some()))
        .map(|u| u.as_summary().version().clone())
        .unwrap_or(Version::parse("0.1.0").unwrap());

    if let Some(oldc) = old_crate {
        return Ok((Version::parse(&oldc.from)?, Version::parse(&oldc.to)?));
    }

    let mut to = from.clone();

    if !publish || plan.hold_version {
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
    }

    Ok((from, to))
}

fn is_publish(
    plan: &Plan,
    c: &Package,
    changed: &BTreeSet<String>,
) -> Result<Option<PublishReason>> {
    if c.publish().is_some() {
        return Ok(None);
    }

    if plan.all {
        return Ok(Some(PublishReason::All));
    }

    if plan.crates.iter().any(|p| p == c.name().as_str()) {
        return Ok(Some(PublishReason::Specified));
    }

    if changed.contains(c.name().as_str()) {
        return Ok(Some(PublishReason::Changed));
    }

    Ok(None)
}

fn remove_git_deps(
    cra: &Package,
    workspace_crates: &BTreeMap<&str, &Package>,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
    remove_crate: &mut Vec<RemoveCrate>,
) -> Vec<RemoveDep> {
    let mut remove_deps = Vec::new();

    if cra.publish().is_some() {
        return Vec::new();
    }

    for dep in cra
        .dependencies()
        .iter()
        .filter(|d| d.kind() != DepKind::Development)
    {
        if dep.source_id().is_git() {
            if !workspace_crates.contains_key(dep.package_name().as_str()) {
                if !upstream.contains_key(dep.package_name().as_str()) {
                    if dep.is_optional() {
                        let remove = RemoveDep {
                            name: dep.package_name().to_string(),
                            package: None,
                        };
                        remove_deps.push(remove);
                    } else {
                        let remove = RemoveCrate {
                            name: dep.package_name().to_string(),
                        };
                        if !remove_crate.contains(&remove) {
                            remove_crate.push(remove);
                        }
                    }
                }
            }
        }
    }

    remove_deps.sort();
    remove_deps.dedup();
    remove_deps
}

async fn rewrite_git_deps(
    cra: &Package,
    workspace_crates: &BTreeMap<&str, &Package>,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
) -> Result<Vec<RewriteDep>> {
    let mut rewrite = Vec::new();

    if cra.publish().is_some() {
        return Ok(rewrite);
    }

    for dep in cra.dependencies() {
        if dep.source_id().is_git() && !dep.is_optional() {
            if !workspace_crates.contains_key(dep.package_name().as_str()) {
                let version = upstream
                    .get(dep.package_name().as_str())
                    .and_then(|c| max_ver(c, false))
                    .with_context(|| {
                        format!("crate {} has no crates.io release", dep.package_name())
                    })?
                    .as_summary()
                    .version();

                rewrite.push(RewriteDep {
                    name: dep.name_in_toml().to_string(),
                    version: Some(version.to_string()),
                    path: None,
                })
            }
        }
    }

    Ok(rewrite)
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

fn read_plan(plan: &Plan) -> Result<Option<Planner>> {
    let path = plan.path.join("Plan.toml");

    if plan.new {
        return Ok(None);
    }

    if path.exists() {
        let plan = std::fs::read_to_string(&path)?;
        let plan = toml::from_str(&plan)?;
        Ok(Some(plan))
    } else {
        Ok(None)
    }
}

fn write_plan(plan: &Plan, workspace: &Workspace, planner: &Planner) -> Result<()> {
    let mut planner: DocumentMut = toml_edit::ser::to_string_pretty(planner)?.parse()?;

    planner
        .as_table_mut()
        .get_mut("crate")
        .and_then(|c| c.as_array_of_tables_mut())
        .into_iter()
        .flat_map(|c| c.iter_mut())
        .for_each(|c| {
            c.get_key_value_mut("name").map(|(mut k, v)| {
                workspace
                    .members()
                    .find(|name| Some(name.name().as_str()) == v.as_str())
                    .and_then(|c| c.root().strip_prefix(workspace.root()).ok())
                    .map(|c| {
                        k.dotted_decor_mut()
                            .set_prefix(format!("# {}\n", c.display()))
                    })
            });
        });

    let command = args().skip(1).collect::<Vec<_>>().join(" ");

    let output = format!(
        "# generated by {} v{}\n# command: {} {}\n\n{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_NAME"),
        command,
        planner.to_string(),
    );

    std::fs::write(plan.path.join("Plan.toml"), output)?;
    Ok(())
}

fn max_ver(crates: &[IndexSummary], pre: bool) -> Option<&IndexSummary> {
    crates
        .iter()
        .filter(|c| pre || c.as_summary().version().pre.is_empty())
        .max_by_key(|c| c.as_summary().version())
}
