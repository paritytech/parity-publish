use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, resolver::CliFeatures, FeatureValue, Package, Workspace},
    ops::{Packages, PublishOpts},
    sources::IndexSummary,
    util::{cache_lock::CacheLockMode, toml_mut::manifest::LocalManifest},
};

use semver::{Version, VersionReq};

use std::{
    collections::{BTreeMap, BTreeSet},
    env::{self, current_dir},
    io::Write,
    path::Path,
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use crate::{
    cli::{Apply, Args},
    config, edit,
    plan::{expand_plan, get_upstream, Planner, RemoveFeature},
    registry,
};

pub async fn handle_apply(args: Args, apply: Apply) -> Result<()> {
    let path = current_dir()?;
    let mut stdout = args.stdout();
    let mut stderr = args.stderr();

    let cargo_config = cargo::GlobalContext::default()?;
    cargo_config
        .shell()
        .set_verbosity(cargo::core::Verbosity::Quiet);

    let workspace = Workspace::new(&path.join("Cargo.toml"), &cargo_config)?;
    let config = config::read_config(&path)?;

    let workspace_crates = workspace
        .members()
        .map(|m| (m.name().as_str(), m))
        .collect::<BTreeMap<_, _>>();

    let upstream = get_upstream(&workspace, &mut stderr).await?;

    let plan = std::fs::read_to_string(path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have your ran plan first?")?;
    let mut plan: Planner = toml::from_str(&plan)?;
    expand_plan(&workspace, &workspace_crates, &mut plan, &upstream).await?;

    let local_path_overrides = apply
        .registry
        .then(|| compute_must_use_local(&workspace, &plan, &upstream));

    if apply.print {
        list(&path, &cargo_config, &plan)?;
        return Ok(());
    }

    let token = if apply.publish {
        env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
            .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?
    } else {
        String::new()
    };

    writeln!(stdout, "rewriting manifests...")?;

    config::apply_config(&workspace, &config)?;

    let workspace_crates = workspace
        .members()
        .map(|m| (m.name().as_str(), m))
        .collect::<BTreeMap<_, _>>();

    let root_manifest = std::fs::read_to_string(workspace.root_manifest())?;
    let mut root_manifest = toml_edit::DocumentMut::from_str(&root_manifest)?;
    for pkg in &plan.crates {
        let Some(c) = workspace_crates.get(pkg.name.as_str()) else {
            continue;
        };

        let mut manifest = LocalManifest::try_new(c.manifest_path())?;
        edit::set_version(&mut manifest, &pkg.to)?;
        //edit::set_description(&plan, &mut manifest, &pkg.name)?;
        edit::set_readme_desc(&workspace, &plan)?;

        for remove_dep in &pkg.remove_dep {
            edit::remove_dep(&workspace, &mut root_manifest, &mut manifest, remove_dep)?;
        }

        edit::rewrite_deps(
            &workspace,
            &path,
            &plan,
            &mut root_manifest,
            &mut manifest,
            &workspace_crates,
            &upstream,
            &pkg.rewrite_dep,
            apply.registry,
            local_path_overrides.as_ref(),
        )?;

        for remove_feature in &pkg.remove_feature {
            edit::remove_feature(&mut manifest, remove_feature)?;
        }
        for remove_feature in remove_dev_features(c) {
            edit::remove_feature(&mut manifest, &remove_feature)?;
        }

        manifest.write()?;
        std::fs::write(workspace.root_manifest(), &root_manifest.to_string())?;
    }

    if !apply.publish {
        return Ok(());
    }

    publish(&args, &apply, &cargo_config, plan, &path, token)
}

fn list(
    path: &std::path::PathBuf,
    cargo_config: &cargo::GlobalContext,
    plan: &Planner,
) -> Result<(), anyhow::Error> {
    let workspace = Workspace::new(&path.join("Cargo.toml"), cargo_config)?;
    let _lock = cargo_config.acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(&workspace)?;
    registry::download_crates(&mut reg, &workspace, false)?;
    Ok(
        for c in plan
            .crates
            .iter()
            .filter(|c| {
                workspace
                    .members()
                    .find(|m| m.name().as_str() == c.name)
                    .map(|m| m.publish().is_some())
                    .unwrap_or(false)
            })
            .filter(|c| !version_exists(&mut reg, &c.name, &c.to))
        {
            println!("{}@{}", c.name, c.to);
        },
    )
}

fn publish(
    args: &Args,
    apply: &Apply,
    config: &cargo::GlobalContext,
    plan: Planner,
    path: &Path,
    token: String,
) -> Result<()> {
    let mut stdout = args.stdout();
    let mut n = 1;

    let workspace = Workspace::new(&path.join("Cargo.toml"), config)?;

    let _lock = config.acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(&workspace)?;
    registry::download_crates(&mut reg, &workspace, false)?;

    let skipped = plan
        .crates
        .iter()
        .filter(|c| c.publish)
        .filter(|pkg| version_exists(&mut reg, &pkg.name, &pkg.to))
        .count();
    let total = plan.crates.iter().filter(|c| c.publish).count() - skipped;

    writeln!(
        stdout,
        "Publishing {} packages ({} skipped)",
        total, skipped
    )?;

    drop(_lock);

    let mut iter = plan
        .crates
        .iter()
        .filter(|c| c.publish)
        .filter(|c| !version_exists(&mut reg, &c.name, &c.to))
        .peekable();
    while let Some(pkg) = iter.next() {
        write!(
            stdout,
            "({:3<}/{:3<}) publishing {}-{}...",
            n, total, pkg.name, pkg.to
        )?;
        stdout.flush()?;

        n += 1;

        let wait = Duration::from_secs(60);
        let before = Instant::now();

        let opts = PublishOpts {
            gctx: config,
            token: Some(token.clone().into()),
            verify: pkg.verify && !apply.dry_run && !apply.no_verify,
            allow_dirty: apply.allow_dirty,
            jobs: None,
            keep_going: false,
            to_publish: Packages::Packages(vec![pkg.name.clone()]),
            targets: Vec::new(),
            dry_run: apply.dry_run,
            cli_features: CliFeatures::new_all(false),
            reg_or_index: None,
        };
        cargo::ops::publish(&workspace, &opts)?;

        let after = Instant::now();
        writeln!(stdout, " ({}s)", (after - before).as_secs())?;

        if iter.peek().is_some() {
            if let Some(delay) = (before + wait).checked_duration_since(after) {
                thread::sleep(delay);
            }
        }
    }

    Ok(())
}

fn version_exists(reg: &mut cargo::sources::RegistrySource, name: &str, ver: &str) -> bool {
    let c = registry::get_crate(reg, name.to_string().into());
    let ver = Version::parse(ver).unwrap();

    if let Ok(c) = c {
        if c.iter().any(|v| v.as_summary().version() == &ver) {
            return true;
        }
    }

    false
}

fn remove_dev_features(member: &Package) -> Vec<RemoveFeature> {
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
                FeatureValue::DepFeature { dep_name, .. } => dep_name.as_str(),
            };

            if dev.contains(dep_name) {
                remove.push(RemoveFeature {
                    feature: feature.to_string(),
                    value: Some(need.to_string()),
                });
            }
        }
    }

    remove
}

/// Compute the transitive closure of crates that must use local paths.
///
/// A crate must use a local path if:
/// 1. Its new version doesn't exist on crates.io, OR
/// 2. It depends (transitively) on a crate that must use a local path
fn compute_must_use_local(
    workspace: &Workspace,
    plan: &Planner,
    upstream: &BTreeMap<String, Vec<IndexSummary>>,
) -> BTreeSet<String> {
    // Step 1: Find crates whose new version doesn't exist on crates.io
    let mut must_use_local = BTreeSet::new();
    for pkg in &plan.crates {
        let ver = VersionReq::parse(&pkg.to).ok();
        let has_upstream = ver.as_ref().and_then(|v| {
            upstream.get(&pkg.name).and_then(|versions| {
                versions
                    .iter()
                    .find(|u| v.matches(u.as_summary().version()))
            })
        });

        if has_upstream.is_none() {
            must_use_local.insert(pkg.name.clone());
        }
    }

    // Step 2: Build reverse dependency graph (crate → dependents)
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for member in workspace.members() {
        let name = member.name().to_string();
        for dep in member
            .dependencies()
            .iter()
            .filter(|d| d.kind() != DepKind::Development)
        {
            dependents
                .entry(dep.package_name().to_string())
                .or_default()
                .push(name.clone());
        }
    }

    // Step 3: BFS to propagate to all transitive dependents
    let mut queue: Vec<String> = must_use_local.iter().cloned().collect();
    while let Some(crate_name) = queue.pop() {
        if let Some(deps) = dependents.get(&crate_name) {
            for dep in deps {
                if must_use_local.insert(dep.clone()) {
                    queue.push(dep.clone());
                }
            }
        }
    }

    must_use_local
}
