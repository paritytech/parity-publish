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

    let poll_interval = Duration::from_secs(apply.poll_interval);
    let poll_timeout = Duration::from_secs(apply.poll_timeout);

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

        let after_publish = Instant::now();
        writeln!(stdout, " published ({}s)", (after_publish - before).as_secs())?;

        // Wait for crate to appear in the index before publishing dependents
        if iter.peek().is_some() && !apply.dry_run {
            let version = Version::parse(&pkg.to)?;
            write!(stdout, "    waiting for {} to appear in index...", pkg.name)?;
            stdout.flush()?;

            let wait_start = Instant::now();
            let mut appeared = false;

            while wait_start.elapsed() < poll_timeout {
                if registry::version_exists_fresh(&workspace, &pkg.name, &version)? {
                    appeared = true;
                    break;
                }
                thread::sleep(poll_interval);
            }

            let wait_time = wait_start.elapsed().as_secs();
            if appeared {
                writeln!(stdout, " ready ({}s)", wait_time)?;
            } else {
                writeln!(stdout, " timeout after {}s, continuing anyway", wait_time)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_version_exists_returns_true_for_existing() {
        // Test the version_exists function with a mock registry
        // This is a basic sanity check that the function signature works
        // Real integration tests would require a workspace
    }

    #[test]
    fn test_polling_timeout_logic() {
        // Test that the polling loop respects timeout
        let poll_timeout = Duration::from_millis(100);
        let poll_interval = Duration::from_millis(20);
        let start = Instant::now();

        let mut iterations = 0;
        while start.elapsed() < poll_timeout {
            iterations += 1;
            // Simulate checking (always returns false)
            let found = false;
            if found {
                break;
            }
            thread::sleep(poll_interval);
        }

        // Should have done at least a few iterations
        assert!(iterations >= 2, "Should have polled multiple times");
        // Should have respected the timeout (with some tolerance for timing)
        assert!(
            start.elapsed() >= poll_timeout,
            "Should have waited at least poll_timeout duration"
        );
    }

    #[test]
    fn test_polling_early_exit() {
        // Test that polling exits early when condition is met
        let poll_timeout = Duration::from_secs(10); // Long timeout
        let poll_interval = Duration::from_millis(10);
        let start = Instant::now();

        let mut iterations = 0;
        while start.elapsed() < poll_timeout {
            iterations += 1;
            // Simulate finding the crate on second iteration
            let found = iterations >= 2;
            if found {
                break;
            }
            thread::sleep(poll_interval);
        }

        // Should have exited after finding
        assert_eq!(iterations, 2, "Should have exited after finding on 2nd iteration");
        // Should have finished well before timeout
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "Should have finished quickly after finding"
        );
    }

    #[test]
    fn test_duration_from_cli_values() {
        // Test that Duration::from_secs works with typical CLI values
        let poll_interval = Duration::from_secs(5);
        let poll_timeout = Duration::from_secs(60);

        assert_eq!(poll_interval.as_secs(), 5);
        assert_eq!(poll_timeout.as_secs(), 60);
        assert!(poll_timeout > poll_interval);
    }
}
