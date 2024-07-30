use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, resolver::CliFeatures, FeatureValue, Package, Workspace},
    ops::{Packages, PublishOpts},
    util::{cache_lock::CacheLockMode, toml_mut::manifest::LocalManifest},
};

use semver::Version;

use std::{
    collections::{BTreeMap, BTreeSet},
    env::{self, current_dir},
    io::Write,
    ops::Add,
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
        edit::set_description(&plan, &mut manifest, &pkg.name)?;

        for remove_dep in &pkg.remove_dep {
            edit::remove_dep(&workspace, &mut root_manifest, &mut manifest, remove_dep)?;
        }

        edit::rewrite_deps(
            &path,
            &plan,
            &mut root_manifest,
            &mut manifest,
            &pkg.rewrite_dep,
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
        let now = Instant::now();

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

        writeln!(stdout, " ({}s)", (now - Instant::now()).as_secs())?;

        if iter.peek().is_some() {
            if let Some(delay) = now.add(wait).checked_duration_since(Instant::now()) {
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
