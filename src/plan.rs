use std::{
    collections::{BTreeMap, BTreeSet},
    env::{args, current_dir},
    fmt::Display,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, Package, Workspace},
    sources::IndexSummary,
    util::cache_lock::CacheLockMode,
};
use semver::{Prerelease, Version};
use toml_edit::DocumentMut;

use crate::{
    changed::{self, Change},
    check,
    cli::{Args, Check, Plan},
    prdoc, registry,
    shared::*,
};

#[derive(
    serde::Serialize,
    serde::Deserialize,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Copy,
    Clone,
    Debug,
    clap::ValueEnum,
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

#[derive(serde::Serialize, serde::Deserialize, Clone)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct Options {
    pub description: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
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

#[derive(
    Debug, serde::Serialize, serde::Deserialize, Default, PartialOrd, Ord, PartialEq, Eq, Clone,
)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Eq, PartialEq, Clone)]
pub struct RemoveCrate {
    pub name: String,
}

pub async fn handle_plan(args: Args, mut plan: Plan) -> Result<()> {
    read_stdin(&mut plan.crates)?;

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = current_dir()?;
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;
    let mut stdout = args.stdout();
    let mut stderr = args.stderr();

    let upstream = get_upstream(&workspace, &mut stderr).await?;

    let workspace_crates = workspace
        .members()
        .map(|m| (m.name().as_str(), m))
        .collect::<BTreeMap<_, _>>();

    let mut planner = generate_plan(&args, &plan, &workspace, &workspace_crates, &upstream).await?;
    write_plan(&workspace, &planner)?;

    if plan.print_expanded {
        expand_plan(&workspace_crates, &mut planner, &upstream).await?;
        let output = plan_to_str(&workspace, &planner)?;
        writeln!(stdout, "{}", output)?;
        return Ok(());
    }

    if plan.patch {
        patch_bump(&args, &plan, &mut planner)?;
        write_plan(&workspace, &planner)?;
        return Ok(());
    }

    if let Some(from) = &plan.since {
        let changed = changed::get_changed_crates(&workspace, true, from, "HEAD")?;
        let indirect = changed
            .iter()
            .filter(|c| matches!(c.kind, changed::ChangeKind::Dependency))
            .count();
        writeln!(
            stderr,
            "{} packages changed {} indirect",
            changed.len(),
            indirect
        )?;
        apply_bump(&plan, &mut planner, &upstream, &changed)?;
        write_plan(&workspace, &planner)?;
        return Ok(());
    }

    if let Some(path) = &plan.prdoc {
        let mut changed = prdoc::get_prdocs(&workspace, path, true, &[])?;

        changed.retain(|c| {
            workspace_crates
                .get(c.name.as_str())
                .map(|c| c.publish().is_none())
                .unwrap_or(true)
        });

        changed.retain(|c| c.bump != BumpKind::None);

        let indirect = changed
            .iter()
            .filter(|c| matches!(c.kind, changed::ChangeKind::Dependency))
            .filter(|c| c.bump != BumpKind::None)
            .count();
        writeln!(
            stderr,
            "{} packages changed {} indirect",
            changed.len(),
            indirect
        )?;
        apply_bump(&plan, &mut planner, &upstream, &changed)?;
        write_plan(&workspace, &planner)?;
        return Ok(());
    }

    Ok(())
}

pub async fn get_upstream(
    workspace: &Workspace<'_>,
    stderr: &mut termcolor::StandardStream,
) -> Result<BTreeMap<String, Vec<IndexSummary>>> {
    let mut upstream = BTreeMap::new();
    let _lock = workspace
        .config()
        .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(workspace)?;
    writeln!(stderr, "looking up crates...",)?;
    registry::download_crates(&mut reg, workspace, true)?;
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
    Ok(upstream)
}

pub fn apply_bump(
    plan: &Plan,
    planner: &mut Planner,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
    changes: &[Change],
) -> Result<()> {
    for change in changes {
        let Some(c) = planner.crates.iter_mut().find(|c| c.name == change.name) else {
            continue;
        };

        if !c.publish {
            continue;
        }

        let empty = Vec::new();
        c.from = c.to.clone();
        let mut to = Version::parse(&c.from)?;
        c.to = to.to_string();
        c.bump = change.bump;
        c.reason = Some(PublishReason::Changed);
        let u = upstream.get(c.name.as_str()).unwrap_or(&empty);

        match change.bump {
            BumpKind::None => (),
            BumpKind::Patch => loop {
                to.patch += 1;
                if !u.iter().any(|u| u.as_summary().version() == &to) {
                    break;
                }
            },
            BumpKind::Minor => loop {
                if to.major == 0 {
                    to.patch += 1;
                } else {
                    to.minor += 1;
                    to.patch = 0;
                }
                if !u.iter().any(|u| u.as_summary().version() == &to) {
                    break;
                }
            },
            BumpKind::Major => loop {
                if to.major == 0 {
                    to.minor += 1;
                    to.patch = 0;
                    if !u.iter().any(|u| {
                        u.as_summary().version().major == 0
                            && u.as_summary().version().minor == to.minor
                    }) {
                        break;
                    }
                } else {
                    to.major += 1;
                    to.minor = 0;
                    to.patch = 0;
                    if !u.iter().any(|u| u.as_summary().version().major == to.major) {
                        break;
                    }
                }
            },
        }

        if let Some(ref pre) = plan.pre {
            to.pre = Prerelease::new(pre)?;
        } else {
            to.pre = Prerelease::EMPTY;
        }
        to.build = Default::default();

        c.to = to.to_string();
    }

    Ok(())
}

pub fn patch_bump(args: &Args, plan: &Plan, planner: &mut Planner) -> Result<()> {
    let mut stderr = args.stderr();

    for package in &plan.crates {
        let c = planner.crates.iter_mut().find(|c| c.name == *package);

        let Some(c) = c else {
            continue;
        };

        //.with_context(|| format!("could not find crate '{}' in Plan.toml", package))?;

        if !c.publish {
            writeln!(stderr, "crate '{}' is no publish -- ignoring", package)?;
            continue;
        }

        c.from = c.to.clone();
        let mut to = Version::parse(&c.from)?;
        to.patch += 1;
        c.to = to.to_string();
        c.bump = BumpKind::Patch;
        c.reason = Some(PublishReason::Bumped);
    }

    Ok(())
}

pub async fn generate_plan(
    args: &Args,
    plan: &Plan,
    workspace: &Workspace<'_>,
    workspace_crates: &BTreeMap<&str, &Package>,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
) -> Result<Planner> {
    let mut stderr = args.stderr();

    let mut planner = Planner::default();
    let old_plan = read_plan(plan)?.unwrap_or_default();

    planner.options = old_plan.options;

    if plan.description.is_some() {
        planner.options.description = plan.description.clone();
    }

    if !plan.skip_check {
        check::check(
            args,
            Check {
                allow_nonfatal: true,
                allow_unpublished: false,
                no_check_owner: false,
                recursive: false,
                quiet: false,
                paths: 0,
            },
        )
        .await?;
    }

    let order = order(args, &workspace)?;

    for c in order {
        let old_crate = old_plan.crates.iter().find(|old| old.name == c);
        let c = *workspace_crates.get(c).unwrap();

        if let Some(old_crate) = old_crate {
            planner.crates.push(old_crate.clone());
            continue;
        }

        let from = get_version(plan, upstream, c)?;

        planner.crates.push(Publish {
            publish: !c.publish().is_some(),
            name: c.name().to_string(),
            from: from.to_string(),
            to: from.to_string(),
            bump: BumpKind::None,
            reason: None,
            rewrite_dep: vec![],
            remove_feature: vec![],
            remove_dep: vec![],
            verify: true,
        });
    }

    if old_plan.crates.is_empty() {
        writeln!(
            stderr,
            "plan generated {} packages -- {} to publish",
            planner.crates.len(),
            planner.crates.iter().filter(|c| c.publish).count()
        )?;
    } else {
        let added = planner
            .crates
            .iter()
            .filter(|c| !old_plan.crates.iter().any(|o| o.name == c.name))
            .count();
        let removed = old_plan
            .crates
            .iter()
            .filter(|c| !planner.crates.iter().any(|o| o.name == c.name))
            .count();

        writeln!(
            stderr,
            "plan refreshed {} packages (+{} -{}) -- {} to publish",
            planner.crates.len(),
            added,
            removed,
            planner.crates.iter().filter(|c| c.publish).count()
        )?;
    }

    Ok(planner)
}

pub async fn expand_plan(
    workspace_crates: &BTreeMap<&str, &Package>,
    planner: &mut Planner,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
) -> Result<()> {
    for pkg in &mut planner.crates {
        let Some(c) = workspace_crates.get(pkg.name.as_str()) else {
            continue;
        };

        for dep in rewrite_git_deps(c, &workspace_crates, upstream).await? {
            if !pkg.rewrite_dep.iter().any(|d| d.name == dep.name) {
                pkg.rewrite_dep.push(dep);
            }
        }

        for dep in remove_git_deps(c, &workspace_crates, upstream, &mut planner.remove_crates) {
            if !pkg.remove_dep.iter().any(|d| d.name == dep.name) {
                pkg.remove_dep.push(dep);
            }
        }
    }
    Ok(())
}

fn get_version(
    plan: &Plan,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
    c: &Package,
) -> Result<Version> {
    let upstreamc = upstream.get(c.name().as_str());
    let mut from = upstreamc
        .and_then(|u| max_ver(u, plan.pre.is_some()))
        .map(|u| u.as_summary().version().clone())
        .unwrap_or_else(|| {
            let mut v = c.version().clone();
            v.pre = Default::default();
            v.build = Default::default();
            v
        });

    if from.major == 0 && from.minor == 0 {
        from = Version::parse("0.1.0").unwrap();
    }

    Ok(from)
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

fn order<'a>(args: &Args, workspace: &'a Workspace) -> Result<Vec<&'a str>> {
    let mut stderr = args.stderr();
    writeln!(stderr, "calculating order...")?;

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
    let path = Path::new("Plan.toml");

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

fn plan_to_str(workspace: &Workspace, planner: &Planner) -> Result<String> {
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

    Ok(output)
}

fn write_plan(workspace: &Workspace, planner: &Planner) -> Result<()> {
    let output = plan_to_str(workspace, planner)?;
    std::fs::write(Path::new("Plan.toml"), output)?;
    Ok(())
}

fn max_ver(crates: &[IndexSummary], pre: bool) -> Option<&IndexSummary> {
    crates
        .iter()
        .filter(|c| pre || c.as_summary().version().pre.is_empty())
        .max_by_key(|c| c.as_summary().version())
}
