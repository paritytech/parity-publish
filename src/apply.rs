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
    process::Stdio,
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use crate::{
    cli::{Apply, Args},
    config, edit,
    plan::{expand_plan, get_upstream, Planner, Publish, RemoveFeature},
    registry,
};

pub async fn handle_apply(args: Args, apply: Apply) -> Result<()> {
    let path = current_dir()?;
    let mut stdout = args.stdout();
    let mut stderr = args.stderr();

    // Cargo's GlobalContext snapshots all env vars at construction time.
    // We must set overrides BEFORE creating GlobalContext.
    //
    // CARGO: Since parity-publish embeds cargo as a library, current_exe()
    // returns the parity-publish binary (not "cargo"), so cargo_exe() falls
    // back to the $CARGO env var from the snapshot. We set it to the real
    // cargo binary so build scripts don't try to run `parity-publish metadata`.
    //
    // RUSTUP_TOOLCHAIN: Without this, rustup's proxy may pick up stale
    // toolchain config or rust-version fields and try to install/use old Rust
    // versions during publish verification. Pinning to the active toolchain
    // ensures all subprocesses use the same Rust version.
    if let Ok(output) = std::process::Command::new("rustup").args(["which", "cargo"]).output() {
        if output.status.success() {
            let cargo_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            env::set_var("CARGO", &cargo_path);
        }
    }
    if let Ok(output) = std::process::Command::new("rustup")
        .args(["show", "active-toolchain"])
        .output()
    {
        if output.status.success() {
            let toolchain = String::from_utf8_lossy(&output.stdout);
            // Output format: "stable-aarch64-apple-darwin (default)" — take first word
            if let Some(name) = toolchain.split_whitespace().next() {
                env::set_var("RUSTUP_TOOLCHAIN", name);
            }
        }
    }

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

    // Compute publish levels before rewriting manifests (workspace still has path deps)
    let levels = if apply.jobs > 1 {
        let publishable: BTreeSet<String> = plan
            .crates
            .iter()
            .filter(|c| c.publish)
            .map(|c| c.name.clone())
            .collect();
        compute_publish_levels(&workspace, &publishable)
    } else {
        Vec::new()
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
        edit::remove_rust_version(&mut manifest)?;
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

    if apply.jobs > 1 {
        publish_parallel(&args, &apply, &cargo_config, plan, &path, token, levels).await
    } else {
        publish(&args, &apply, &cargo_config, plan, &path, token)
    }
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

        let wait = Duration::from_secs(15);
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

/// Compute dependency levels for parallel publishing.
/// Crates within the same level have no interdependencies and can be published simultaneously.
fn compute_publish_levels(workspace: &Workspace, publishable: &BTreeSet<String>) -> Vec<Vec<String>> {
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for member in workspace.members() {
        let name = member.name().to_string();
        if !publishable.contains(&name) {
            continue;
        }

        let member_deps: BTreeSet<String> = member
            .dependencies()
            .iter()
            .filter(|d| d.kind() != DepKind::Development)
            .map(|d| d.package_name().to_string())
            .filter(|d| publishable.contains(d))
            .collect();

        deps.insert(name, member_deps);
    }

    let mut levels = Vec::new();

    while !deps.is_empty() {
        let level: Vec<String> = deps
            .iter()
            .filter(|(_, d)| d.is_empty())
            .map(|(name, _)| name.clone())
            .collect();

        if level.is_empty() {
            // Remaining crates have circular dependencies; add as final level
            levels.push(deps.keys().cloned().collect());
            break;
        }

        let level_set: BTreeSet<&str> = level.iter().map(|s| s.as_str()).collect();

        for d in deps.values_mut() {
            d.retain(|dep| !level_set.contains(dep.as_str()));
        }

        deps.retain(|name, _| !level_set.contains(name.as_str()));

        levels.push(level);
    }

    levels
}

async fn publish_parallel(
    args: &Args,
    apply: &Apply,
    config: &cargo::GlobalContext,
    plan: Planner,
    path: &Path,
    token: String,
    levels: Vec<Vec<String>>,
) -> Result<()> {
    let mut stdout = args.stdout();
    let jobs = apply.jobs.max(1);

    // Check which crates are already published
    let workspace = Workspace::new(&path.join("Cargo.toml"), config)?;
    let _lock = config.acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(&workspace)?;
    registry::download_crates(&mut reg, &workspace, false)?;

    let already_published: BTreeSet<String> = plan
        .crates
        .iter()
        .filter(|c| c.publish)
        .filter(|c| version_exists(&mut reg, &c.name, &c.to))
        .map(|c| c.name.clone())
        .collect();

    drop(_lock);

    // Filter levels to only include crates that need publishing
    let levels: Vec<Vec<String>> = levels
        .into_iter()
        .map(|level| {
            level
                .into_iter()
                .filter(|name| !already_published.contains(name))
                .collect()
        })
        .filter(|level: &Vec<String>| !level.is_empty())
        .collect();

    let total: usize = levels.iter().map(|l| l.len()).sum();
    let skipped = already_published.len();

    writeln!(
        stdout,
        "Publishing {} crates in {} levels ({} skipped, max {} parallel)",
        total,
        levels.len(),
        skipped,
        jobs,
    )?;

    let plan_map: BTreeMap<&str, &Publish> = plan
        .crates
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    let mut n = 0usize;

    for (level_idx, level) in levels.iter().enumerate() {
        writeln!(
            stdout,
            "\n--- Level {}/{} ({} crates) ---",
            level_idx + 1,
            levels.len(),
            level.len(),
        )?;
        stdout.flush()?;

        let level_start = Instant::now();

        for chunk in level.chunks(jobs) {
            // Spawn all processes in the chunk simultaneously
            let mut children: Vec<(String, String, tokio::process::Child)> = Vec::new();

            for crate_name in chunk {
                let pkg = plan_map.get(crate_name.as_str());

                let mut cmd = tokio::process::Command::new("cargo");
                cmd.arg("publish")
                    .arg("-p")
                    .arg(crate_name)
                    .arg("--token")
                    .arg(&token);

                if apply.dry_run {
                    cmd.arg("--dry-run");
                }

                let no_verify =
                    apply.no_verify || apply.dry_run || pkg.map_or(false, |p| !p.verify);
                if no_verify {
                    cmd.arg("--no-verify");
                }

                if apply.allow_dirty {
                    cmd.arg("--allow-dirty");
                }

                cmd.current_dir(path);
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());

                let child = cmd.spawn().with_context(|| {
                    format!("failed to spawn cargo publish for {}", crate_name)
                })?;

                let version = pkg.map(|p| p.to.clone()).unwrap_or_default();
                children.push((crate_name.clone(), version, child));
            }

            // Wait for all children in this chunk (they're already running in parallel)
            for (name, version, child) in children {
                let output = child
                    .wait_with_output()
                    .await
                    .with_context(|| format!("cargo publish for {} failed", name))?;

                n += 1;

                if output.status.success() {
                    writeln!(
                        stdout,
                        "({:3}/{:3}) published {}-{}",
                        n, total, name, version,
                    )?;
                } else {
                    let stderr_str = String::from_utf8_lossy(&output.stderr);
                    if stderr_str.contains("already uploaded")
                        || stderr_str.contains("already exists")
                    {
                        writeln!(
                            stdout,
                            "({:3}/{:3}) skipped {}-{} (already published)",
                            n, total, name, version,
                        )?;
                    } else {
                        writeln!(
                            stdout,
                            "({:3}/{:3}) FAILED {}-{}",
                            n, total, name, version,
                        )?;
                        anyhow::bail!(
                            "failed to publish {}-{}:\n{}",
                            name,
                            version,
                            stderr_str.trim(),
                        );
                    }
                }
            }
        }

        let level_elapsed = level_start.elapsed();
        writeln!(stdout, "    level completed in {}s", level_elapsed.as_secs())?;

        // Wait between levels for crates.io index to update
        if level_idx + 1 < levels.len() && !apply.dry_run {
            let wait = Duration::from_secs(30);
            write!(stdout, "Waiting {}s for index update...", wait.as_secs())?;
            stdout.flush()?;
            tokio::time::sleep(wait).await;
            writeln!(stdout, " done")?;
        }
    }

    writeln!(stdout, "\nDone! Published {} crates.", n)?;
    Ok(())
}
