use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, resolver::CliFeatures, FeatureValue, Package, Workspace},
    ops::{Packages, PublishOpts},
    util::toml_mut::manifest::LocalManifest,
};

use semver::Version;

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    io::Write,
    ops::Add,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use termcolor::{ColorChoice, StandardStream};

use crate::{
    cli::Apply,
    config, edit,
    plan::{Planner, RemoveFeature, RewriteDep},
    registry,
};

pub async fn handle_apply(apply: Apply) -> Result<()> {
    let path = apply.path.canonicalize()?;
    env::set_current_dir(&path)?;

    let plan = std::fs::read_to_string(apply.path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have your ran plan first?")?;
    let plan: Planner = toml::from_str(&plan)?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let cargo_config = cargo::Config::default()?;
    cargo_config
        .shell()
        .set_verbosity(cargo::core::Verbosity::Quiet);

    let workspace = Workspace::new(&path.join("Cargo.toml"), &cargo_config)?;

    let config = config::read_config(&path)?;

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

    for pkg in &plan.crates {
        let c = *workspace_crates.get(pkg.name.as_str()).unwrap();
        let mut manifest = LocalManifest::try_new(&path.join(&pkg.path).join("Cargo.toml"))?;
        edit::set_version(&mut manifest, &pkg.to)?;
        edit::fix_description(&mut manifest, &pkg.name)?;

        for remove_dep in &pkg.remove_dep {
            edit::remove_dep(&workspace, &mut manifest, remove_dep)?;
        }

        let rewite_deps = rewrite_deps(&apply, c, &workspace_crates)?;
        edit::rewrite_deps(&path, &plan, &mut manifest, &pkg.rewrite_dep)?;
        edit::rewrite_deps(&path, &plan, &mut manifest, &rewite_deps)?;

        for remove_feature in &pkg.remove_feature {
            edit::remove_feature(&mut manifest, remove_feature)?;
        }
        for remove_feature in remove_dev_features(c) {
            edit::remove_feature(&mut manifest, &remove_feature)?;
        }

        manifest.write()?;
    }

    if !apply.publish {
        return Ok(());
    }

    publish(&apply, &cargo_config, plan, &path, token)
}

fn publish(
    apply: &Apply,
    config: &cargo::Config,
    plan: Planner,
    path: &Path,
    token: String,
) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut n = 1;

    let workspace = Workspace::new(&path.join("Cargo.toml"), config)?;

    let _lock = config.acquire_package_cache_lock()?;
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

    for pkg in plan.crates.iter().filter(|c| c.publish) {
        if version_exists(&mut reg, &pkg.name, &pkg.to) {
            continue;
        }

        writeln!(
            stdout,
            "({:3<}/{:3<}) publishing {}-{}...",
            n, total, pkg.name, pkg.to
        )?;

        n += 1;

        let wait = Duration::from_secs(60);
        let now = Instant::now();

        let opts = PublishOpts {
            config,
            token: Some(token.clone().into()),
            index: None,
            verify: pkg.verify && !apply.dry_run && !apply.no_verify,
            allow_dirty: apply.allow_dirty,
            jobs: None,
            keep_going: false,
            to_publish: Packages::Packages(vec![pkg.name.clone()]),
            targets: Vec::new(),
            dry_run: apply.dry_run,
            registry: None,
            cli_features: CliFeatures::new_all(false),
        };
        cargo::ops::publish(&workspace, &opts)?;

        if let Some(delay) = now.add(wait).checked_duration_since(Instant::now()) {
            thread::sleep(delay);
        }
    }

    Ok(())
}

fn version_exists(reg: &mut cargo::sources::RegistrySource, name: &str, ver: &str) -> bool {
    let c = registry::get_crate(reg, name.to_string().into());
    let ver = Version::parse(ver).unwrap();

    if let Ok(c) = c {
        if c.iter().any(|v| v.version() == &ver) {
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

fn rewrite_deps(
    apply: &Apply,
    cra: &Package,
    workspace_crates: &BTreeMap<&str, &Package>,
) -> Result<Vec<RewriteDep>> {
    let mut rewrite = Vec::new();

    for dep in cra.dependencies() {
        if dep.source_id().is_path() {
            let dep_crate = workspace_crates
                .get(dep.package_name().as_str())
                .with_context(|| {
                    format!(
                        "dependency '{}' in crate '{}' is not part of the workspace",
                        dep.package_name(),
                        cra.name(),
                    )
                })?;
            let path = apply.path.canonicalize()?;

            rewrite.push(RewriteDep {
                name: dep.name_in_toml().to_string(),
                version: None,
                path: Some(dep_crate.root().strip_prefix(path).unwrap().to_path_buf()),
            })
        }
    }

    Ok(rewrite)
}
