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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
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

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
pub struct RemoveFeature {
    pub feature: String,
    #[serde(skip_serializing_if = "is_default")]
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Eq, PartialEq, Clone, Debug)]
pub struct RemoveCrate {
    pub name: String,
}

pub async fn handle_plan(args: Args, mut plan: Plan) -> Result<()> {
    read_stdin(&mut plan.crates)?;

    let config = cargo::GlobalContext::default()?;
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

    if plan.print_expanded {
        expand_plan(&workspace, &workspace_crates, &mut planner, &upstream).await?;
        let output = plan_to_str(&workspace, &planner)?;
        writeln!(stdout, "{}", output)?;
        return Ok(());
    }

    if plan.patch {
        patch_bump(&args, &plan, &mut planner, &upstream)?;
        write_plan(&workspace, &planner)?;
        return Ok(());
    }

    write_plan(&workspace, &planner)?;

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
        apply_bump(&args, &plan, &mut planner, &upstream, &changed)?;
        write_plan(&workspace, &planner)?;
        return Ok(());
    }

    if let Some(path) = &plan.prdoc {
        let mut changed = prdoc::get_prdocs(&args, &workspace, path, true, &[])?;

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
        apply_bump(&args, &plan, &mut planner, &upstream, &changed)?;
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
        .gctx()
        .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(workspace)?;
    writeln!(stderr, "looking up crates...",)?;
    registry::download_crates(&mut reg, workspace, true)?;
    for c in workspace.members().filter(|c| c.publish().is_none()) {
        let idx_summaries = registry::get_crate(&mut reg, c.name());
        // New crates (not published yet) should be handled gracefully as
        // a summary can not be fetched for them from the registry.
        if let Ok(summary) = idx_summaries {
            upstream.insert(c.name().to_string(), summary);
        }

        for dep in c.dependencies() {
            if dep.source_id().is_git() || dep.source_id().is_path() {
                // Similarly, the same should happen for new crates that represent dependencies
                // of member crates.
                if let Ok(package) = registry::get_crate(&mut reg, dep.package_name()) {
                    upstream.insert(dep.package_name().to_string(), package);
                }
            }
        }
    }
    Ok(upstream)
}

/// A version number the planner had to step over because it is already taken on
/// crates.io.
struct Taken {
    /// The published version that occupies the slot we wanted.
    version: Version,
    /// Whether that version is yanked. Yanked versions are the surprising case:
    /// they are invisible in normal cargo queries but their number stays
    /// reserved forever.
    yanked: bool,
}

/// Bump `to` according to `bump`, skipping any version number that is already
/// taken on the registry.
///
/// `upstream` includes yanked versions (see [`registry::get_crate`]), so those
/// get skipped too. Publishing over a yanked version is impossible — crates.io
/// answers with `crate version 'x' is already uploaded` — so a plan that lands
/// on one is guaranteed to fail halfway through the release.
///
/// Returns the taken versions that were stepped over, for reporting.
fn bump_version(to: &mut Version, bump: BumpKind, upstream: &[IndexSummary]) -> Vec<Taken> {
    let mut skipped = Vec::new();

    // The lowest published version in a major/minor series, if the series is
    // occupied at all. Used by major bumps, which skip whole series rather than
    // single version numbers.
    let occupied = |taken: &dyn Fn(&Version) -> bool| {
        upstream
            .iter()
            .map(|u| u.as_summary().version())
            .filter(|v| taken(v))
            .min()
            .map(|v| Taken {
                version: v.clone(),
                yanked: upstream
                    .iter()
                    .filter(|u| u.as_summary().version() == v)
                    .any(|u| u.is_yanked()),
            })
    };

    match bump {
        BumpKind::None => (),
        BumpKind::Patch => loop {
            to.patch += 1;
            match occupied(&|v| v == to) {
                Some(taken) => skipped.push(taken),
                None => break,
            }
        },
        BumpKind::Minor => loop {
            if to.major == 0 {
                to.patch += 1;
            } else {
                to.minor += 1;
                to.patch = 0;
            }
            match occupied(&|v| v == to) {
                Some(taken) => skipped.push(taken),
                None => break,
            }
        },
        BumpKind::Major => loop {
            // For 0.x, the minor is the breaking-change component, so a whole
            // 0.minor series has to be free before we can claim it.
            let series: Box<dyn Fn(&Version) -> bool> = if to.major == 0 {
                to.minor += 1;
                to.patch = 0;
                let minor = to.minor;
                Box::new(move |v: &Version| v.major == 0 && v.minor == minor)
            } else {
                to.major += 1;
                to.minor = 0;
                to.patch = 0;
                let major = to.major;
                Box::new(move |v: &Version| v.major == major)
            };
            match occupied(&*series) {
                Some(taken) => skipped.push(taken),
                None => break,
            }
        },
    }

    skipped
}

fn report_taken(
    stderr: &mut termcolor::StandardStream,
    name: &str,
    skipped: &[Taken],
    to: &Version,
) -> Result<()> {
    for taken in skipped {
        writeln!(
            stderr,
            "{}: {} is already published on crates.io{} -- bumping past it to {}",
            name,
            taken.version,
            if taken.yanked { " (yanked)" } else { "" },
            to,
        )?;
    }
    Ok(())
}

pub fn apply_bump(
    args: &Args,
    plan: &Plan,
    planner: &mut Planner,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
    changes: &[Change],
) -> Result<()> {
    let mut stderr = args.stderr();

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

        let skipped = bump_version(&mut to, change.bump, u);

        if let Some(ref pre) = plan.pre {
            to.pre = Prerelease::new(pre)?;
        } else {
            to.pre = Prerelease::EMPTY;
        }
        to.build = Default::default();

        report_taken(&mut stderr, &c.name, &skipped, &to)?;

        c.to = to.to_string();
    }

    Ok(())
}

pub fn patch_bump(
    args: &Args,
    plan: &Plan,
    planner: &mut Planner,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
) -> Result<()> {
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

        let empty = Vec::new();
        let u = upstream.get(c.name.as_str()).unwrap_or(&empty);

        c.from = c.to.clone();
        let mut to = Version::parse(&c.from)?;
        let skipped = bump_version(&mut to, BumpKind::Patch, u);
        report_taken(&mut stderr, &c.name, &skipped, &to)?;
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
            publish: true,
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

    let mut expanded = planner.clone();
    expand_plan(&workspace, workspace_crates, &mut expanded, upstream).await?;

    if old_plan.crates.is_empty() {
        writeln!(
            stderr,
            "plan generated {} packages -- {} to publish",
            expanded.crates.len(),
            expanded.crates.iter().filter(|c| c.publish).count()
        )?;
    } else {
        let added = expanded
            .crates
            .iter()
            .filter(|c| !old_plan.crates.iter().any(|o| o.name == c.name))
            .count();
        let removed = old_plan
            .crates
            .iter()
            .filter(|c| !expanded.crates.iter().any(|o| o.name == c.name))
            .count();

        writeln!(
            stderr,
            "plan refreshed {} packages (+{} -{}) -- {} to publish",
            expanded.crates.len(),
            added,
            removed,
            expanded.crates.iter().filter(|c| c.publish).count()
        )?;
    }

    Ok(planner)
}

pub async fn expand_plan(
    w: &Workspace<'_>,
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

        for dep in rewrite_deps(w, c, workspace_crates)? {
            if !pkg.rewrite_dep.iter().any(|d| d.name == dep.name) {
                pkg.rewrite_dep.push(dep);
            }
        }

        for dep in remove_git_deps(c, &workspace_crates, upstream, &mut planner.remove_crates) {
            if !pkg.remove_dep.iter().any(|d| d.name == dep.name) {
                pkg.remove_dep.push(dep);
            }
        }

        if let Some(c) = workspace_crates.get(pkg.name.as_str()) {
            pkg.publish = c.publish().is_none();
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
        // Fall back to the highest yanked version if every release was yanked,
        // so that the bump starts from what crates.io actually has rather than
        // from a stale local version.
        .and_then(|u| max_ver(u, plan.pre.is_some()).or_else(|| max_ver_any(u, plan.pre.is_some())))
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
                        k.leaf_decor_mut()
                            .set_prefix(format!("# {}\n", c.display()))
                    });
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

/// The highest release that can actually be depended on.
///
/// Yanked versions are excluded — [`registry::get_crate`] returns them so that
/// version numbers are never reused, but they are not something to point a
/// dependency at or to treat as the current release.
fn max_ver(crates: &[IndexSummary], pre: bool) -> Option<&IndexSummary> {
    crates
        .iter()
        .filter(|c| registry::is_usable(c))
        .filter(|c| pre || c.as_summary().version().pre.is_empty())
        .max_by_key(|c| c.as_summary().version())
}

/// The highest version number ever published, yanked or not.
fn max_ver_any(crates: &[IndexSummary], pre: bool) -> Option<&IndexSummary> {
    crates
        .iter()
        .filter(|c| pre || c.as_summary().version().pre.is_empty())
        .max_by_key(|c| c.as_summary().version())
}

fn rewrite_deps(
    w: &Workspace,
    cra: &Package,
    workspace_crates: &BTreeMap<&str, &Package>,
) -> Result<Vec<RewriteDep>> {
    let mut rewrite = Vec::new();

    for dep in cra.dependencies() {
        if let Some(dep_crate) = workspace_crates.get(dep.package_name().as_str()) {
            rewrite.push(RewriteDep {
                name: dep.name_in_toml().to_string(),
                version: None,
                path: Some(
                    dep_crate
                        .root()
                        .strip_prefix(w.root())
                        .unwrap()
                        .to_path_buf(),
                ),
            })
        }
    }

    Ok(rewrite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo::core::{PackageId, SourceId, Summary};
    use std::collections::BTreeMap;

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    /// Build index summaries from `(version, yanked)` pairs, as the registry
    /// would return them for one crate.
    fn summaries(versions: &[(&str, bool)]) -> Vec<IndexSummary> {
        let source = SourceId::from_url(CRATES_IO).unwrap();
        let features = BTreeMap::new();

        versions
            .iter()
            .map(|(version, yanked)| {
                let id = PackageId::new(
                    "test-crate".into(),
                    Version::parse(version).unwrap(),
                    source,
                );
                let summary =
                    Summary::new(id, Vec::new(), &features, None::<String>, None).unwrap();

                if *yanked {
                    IndexSummary::Yanked(summary)
                } else {
                    IndexSummary::Candidate(summary)
                }
            })
            .collect()
    }

    fn bump(from: &str, kind: BumpKind, upstream: &[(&str, bool)]) -> String {
        let mut to = Version::parse(from).unwrap();
        bump_version(&mut to, kind, &summaries(upstream));
        to.to_string()
    }

    /// The case from the bug report: cumulus-pov-validator was released as
    /// 0.4.0 and then yanked, so the next major release has to be 0.5.0.
    #[test]
    fn major_bump_skips_yanked_version() {
        let upstream = [("0.3.0", false), ("0.4.0", true), ("0.3.1", false)];
        assert_eq!(bump("0.3.1", BumpKind::Major, &upstream), "0.5.0");
    }

    #[test]
    fn major_bump_uses_free_version() {
        let upstream = [("0.3.0", false), ("0.3.1", false)];
        assert_eq!(bump("0.3.1", BumpKind::Major, &upstream), "0.4.0");
    }

    #[test]
    fn major_bump_skips_yanked_series() {
        // A 0.x major bump claims a whole 0.minor series, so a yanked 0.4.3
        // rules out 0.4.0 as well.
        let upstream = [("0.3.1", false), ("0.4.3", true)];
        assert_eq!(bump("0.3.1", BumpKind::Major, &upstream), "0.5.0");
    }

    #[test]
    fn major_bump_skips_consecutive_yanked_versions() {
        let upstream = [("0.3.1", false), ("0.4.0", true), ("0.5.0", true)];
        assert_eq!(bump("0.3.1", BumpKind::Major, &upstream), "0.6.0");
    }

    #[test]
    fn major_bump_skips_yanked_major() {
        let upstream = [("1.2.3", false), ("2.0.0", true)];
        assert_eq!(bump("1.2.3", BumpKind::Major, &upstream), "3.0.0");
    }

    #[test]
    fn minor_bump_skips_yanked_version() {
        let upstream = [("1.2.3", false), ("1.3.0", true)];
        assert_eq!(bump("1.2.3", BumpKind::Minor, &upstream), "1.4.0");
    }

    #[test]
    fn patch_bump_skips_yanked_version() {
        let upstream = [("0.3.0", false), ("0.3.1", true)];
        assert_eq!(bump("0.3.0", BumpKind::Patch, &upstream), "0.3.2");
    }

    #[test]
    fn no_bump_leaves_version_alone() {
        let upstream = [("0.3.0", false), ("0.4.0", true)];
        assert_eq!(bump("0.3.0", BumpKind::None, &upstream), "0.3.0");
    }

    #[test]
    fn reports_the_yanked_version_it_skipped() {
        let upstream = summaries(&[("0.3.1", false), ("0.4.0", true)]);
        let mut to = Version::parse("0.3.1").unwrap();
        let skipped = bump_version(&mut to, BumpKind::Major, &upstream);

        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].version, Version::parse("0.4.0").unwrap());
        assert!(skipped[0].yanked);
    }

    #[test]
    fn max_ver_ignores_yanked_releases() {
        let upstream = summaries(&[("0.3.1", false), ("0.4.0", true)]);

        let usable = max_ver(&upstream, false).unwrap().as_summary().version();
        assert_eq!(usable, &Version::parse("0.3.1").unwrap());

        let any = max_ver_any(&upstream, false)
            .unwrap()
            .as_summary()
            .version();
        assert_eq!(any, &Version::parse("0.4.0").unwrap());
    }

    #[test]
    fn yanked_versions_are_taken() {
        let upstream = summaries(&[("0.3.1", false), ("0.4.0", true)]);
        assert!(registry::version_taken(
            &upstream,
            &Version::parse("0.4.0").unwrap()
        ));
        assert!(!registry::version_taken(
            &upstream,
            &Version::parse("0.5.0").unwrap()
        ));
    }
}
