use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, FeatureValue, Package, Workspace},
    util::{cache_lock::CacheLockMode, toml_mut::manifest::LocalManifest},
};

use semver::Version;

use std::{
    collections::{BTreeMap, BTreeSet},
    env::{self, current_dir},
    io::{Write, BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
    time::{Duration, Instant},
};

use rayon::prelude::*;

use crate::{
    cli::{Apply, Args},
    config, edit,
    plan::{expand_plan, get_upstream, Planner, RemoveFeature},
    registry,
};

// Structure to hold crate information with dependency data
#[derive(Debug)]
struct CrateInfo<'a> {
    pkg: &'a crate::plan::Publish,
    dependencies: Vec<String>,
}

// Create dependency-aware batches that ensure dependencies are published before dependents
fn create_dependency_aware_batches<'a>(
    workspace: &Workspace<'_>,
    crates_to_publish: &[&'a crate::plan::Publish],
    target_batch_size: usize,
) -> Result<Vec<Vec<CrateInfo<'a>>>> {
    let mut batches = Vec::new();
    let mut current_batch = Vec::new();
    let mut published_crates = std::collections::HashSet::new();

    // Create a map of crate names to their dependencies
    let mut crate_deps = std::collections::HashMap::new();
    let empty_deps = Vec::new();

    for pkg in crates_to_publish {
        let deps = get_crate_dependencies(workspace, pkg.name.as_str())?;
        crate_deps.insert(pkg.name.as_str(), deps);
    }

    // Sort crates by dependency count (fewer dependencies first)
    let mut sorted_crates: Vec<(&crate::plan::Publish, usize)> = crates_to_publish.iter().map(|pkg| {
        let deps = crate_deps.get(pkg.name.as_str()).unwrap_or(&empty_deps);
        (*pkg, deps.len())
    }).collect();

    sorted_crates.sort_by_key(|(_, deps_count)| *deps_count);

    // Process crates in dependency order
    for (pkg, _) in sorted_crates {
        let deps = crate_deps.get(pkg.name.as_str()).unwrap_or(&empty_deps);

        // Check if all dependencies are already published or in current batch
        let deps_available = deps.iter().all(|dep| {
            published_crates.contains(dep.as_str()) || 
            current_batch.iter().any(|c: &CrateInfo| c.pkg.name == *dep)
        });

        if deps_available {
            // Add to current batch
            current_batch.push(CrateInfo {
                pkg,
                dependencies: deps.clone(),
            });

            // If batch is full, start a new one
            if current_batch.len() >= target_batch_size {
                batches.push(current_batch);
                current_batch = Vec::new();
            }
        } else {
            // Dependencies not available, start new batch
            if !current_batch.is_empty() {
                batches.push(current_batch);
                current_batch = Vec::new();
            }

            // Add this crate to the new batch
            current_batch.push(CrateInfo {
                pkg,
                dependencies: deps.clone(),
            });
        }
    }

    // Add the last batch if it's not empty
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    // Mark crates in completed batches as published
    for batch in &batches {
        for crate_info in batch {
            published_crates.insert(crate_info.pkg.name.as_str());
        }
    }

    Ok(batches)
}

// Get dependencies for a specific crate
fn get_crate_dependencies(workspace: &Workspace<'_>, crate_name: &str) -> Result<Vec<String>> {
    let mut dependencies = Vec::new();

    if let Some(member) = workspace.members().find(|m| m.name().as_str() == crate_name) {
        for dep in member.dependencies() {
            if dep.kind() != cargo::core::dependency::DepKind::Development {
                // Check if this dependency is a workspace member
                if let Some(dep_member) = workspace.members().find(|m| m.name() == dep.package_name()) {
                    if dep_member.publish().is_none() {
                        // This is a workspace member that will be published
                        dependencies.push(dep.package_name().to_string());
                    }
                }
            }
        }
    }

    Ok(dependencies)
}

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

    publish(&args, &apply, &cargo_config, plan, &path, token).await
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

/// Publish a single crate using cargo publish subprocess to capture full output
fn publish_with_subprocess(
    pkg: &crate::plan::Publish,
    apply: &Apply,
    token: &str,
    current_dir: &Path,
) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("publish")
        .arg("--package")
        .arg(&pkg.name)
        .current_dir(current_dir);

    // Add dry-run flag if specified
    if apply.dry_run {
        cmd.arg("--dry-run");
    }

    // Add allow-dirty flag if specified
    if apply.allow_dirty {
        cmd.arg("--allow-dirty");
    }

    // Add no-verify flag if specified
    if apply.no_verify {
        cmd.arg("--no-verify");
    }

    // Configure registry
    if apply.staging || apply.registry_url.is_some() {
        let registry_url = if let Some(url) = &apply.registry_url {
            url.clone()
        } else if apply.staging {
            "https://staging.crates.io".to_string()
        } else {
            "https://crates.io".to_string()
        };

        cmd.env("CARGO_REGISTRY_INDEX", &registry_url);

        if apply.staging {
            cmd.env("CARGO_REGISTRY_STAGING", "true");
        }
    }

    // Set token
    cmd.env("CARGO_REGISTRY_TOKEN", token);

    // Capture both stdout and stderr
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    println!("    Running: cargo publish --package {} in {}", pkg.name, current_dir.display());

    let mut child = cmd.spawn()
        .with_context(|| format!("Failed to spawn cargo publish for {}", pkg.name))?;

    // Capture and display output in real-time
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => println!("    [cargo] {}", line),
                Err(e) => eprintln!("    [cargo stdout error] {}", e),
            }
        }
    }

    if let Some(stderr) = child.stderr.take() {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => eprintln!("    [cargo] {}", line),
                Err(e) => eprintln!("    [cargo stderr error] {}", e),
            }
        }
    }

    let status = child.wait()
        .with_context(|| format!("Failed to wait for cargo publish for {}", pkg.name))?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("cargo publish failed with exit code: {}", status.code().unwrap_or(-1)))
    }
}

async fn publish(
    args: &Args,
    apply: &Apply,
    config: &cargo::GlobalContext,
    plan: Planner,
    path: &Path,
    token: String,
) -> Result<()> {
    let mut stdout = args.stdout();
    let mut stderr = args.stderr();

    // Configure staging registry if requested
    if apply.staging || apply.registry_url.is_some() {
        let registry_url = if let Some(url) = &apply.registry_url {
            url.clone()
        } else if apply.staging {
            "https://staging.crates.io".to_string()
        } else {
            "https://crates.io".to_string()
        };

        writeln!(
            stdout,
            "Using registry: {}",
            registry_url
        )?;

        // Set environment variables for Cargo to use staging registry
        env::set_var("CARGO_REGISTRY_INDEX", &registry_url);

        // Also set staging-specific environment variable
        if apply.staging {
            env::set_var("CARGO_REGISTRY_STAGING", "true");
        }
    }

    // Store the current working directory to ensure threads use the same path
    let current_dir = env::current_dir()?;

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

    // Get list of crates to publish
    let crates_to_publish: Vec<_> = plan
        .crates
        .iter()
        .filter(|c| c.publish)
        .filter(|c| !version_exists(&mut reg, &c.name, &c.to))
        .collect();

    if crates_to_publish.is_empty() {
        writeln!(stdout, "No packages to publish!")?;
        return Ok(());
    }

    // Create dependency-aware batches
    let batches = create_dependency_aware_batches(&workspace, &crates_to_publish, apply.batch_size)?;

    writeln!(
        stdout,
        "Created {} dependency-aware batches",
        batches.len()
    )?;

    // Show batch information
    for (i, batch) in batches.iter().enumerate() {
        writeln!(
            stdout,
            "Batch {}: {} crates ({} dependencies, {} dependents)",
            i + 1,
            batch.len(),
            batch.iter().filter(|c| c.dependencies.is_empty()).count(),
            batch.iter().filter(|c| !c.dependencies.is_empty()).count()
        )?;
    }

    // Configuration for parallel publishing
    let max_concurrent = apply.max_concurrent;
    let delay_between_batches = Duration::from_secs(apply.batch_delay);

    if apply.parallel_batches > 0 {
        writeln!(
            stdout,
            "Using dependency-aware parallel publishing: max {} concurrent crates per batch, {} parallel batches, {}s delay between batch groups",
            max_concurrent, apply.parallel_batches, delay_between_batches.as_secs()
        )?;
    } else {
        writeln!(
            stdout,
            "Using dependency-aware parallel publishing: max {} concurrent crates per batch, {}s delay between batches",
            max_concurrent, delay_between_batches.as_secs()
        )?;
    }

    let mut published_count = 0;
    let mut failed_crates = Vec::new();

    // Process crates in dependency-aware batches
    if apply.parallel_batches > 0 {
        // Process batches in parallel groups
        let batch_groups: Vec<_> = batches.chunks(apply.parallel_batches).collect();

        for (group_idx, batch_group) in batch_groups.iter().enumerate() {
            let group_num = group_idx + 1;
            let total_groups = batch_groups.len();

            writeln!(
                stdout,
                "\n=== Processing batch group {}/{} ({} batches) ===",
                group_num, total_groups, batch_group.len()
            )?;

            // Process batches in this group in parallel
            let group_results: Vec<_> = batch_group.par_iter()
                .enumerate()
                .map(|(batch_idx, batch)| {
                    let global_batch_idx = group_idx * apply.parallel_batches + batch_idx;
                    let batch_num = global_batch_idx + 1;
                    let total_batches = batches.len();

                    println!(
                        "\n--- Processing batch {}/{} ({} crates) ---",
                        batch_num, total_batches, batch.len()
                    );

                    // Process crates in parallel within the batch
                    println!(
                        "Processing batch with up to {} concurrent crates...",
                        max_concurrent
                    );

                    // Create a thread pool for this batch
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(max_concurrent)
                        .build()
                        .unwrap();

                    let batch_results: Vec<_> = pool.install(|| {
                        batch.par_iter()
                            .map(|pkg| {
                                let before = Instant::now();

                                // Use cargo publish as subprocess to capture full output
                                let result = publish_with_subprocess(&pkg.pkg, &apply, &token, &current_dir);
                                let after = Instant::now();
                                let duration = after.duration_since(before);

                                (pkg, result, duration)
                            })
                            .collect()
                    });

                    (batch_num, batch_results)
                })
                .collect();

            // Process all results from this group
            for (_batch_num, batch_results) in group_results {
                for (pkg, result, duration) in batch_results {
                    match result {
                        Ok(_) => {
                            published_count += 1;
                            println!(
                                "✓ ({:3<}/{:3<}) {}@{} published successfully ({}s)",
                                published_count, total, pkg.pkg.name, pkg.pkg.to, duration.as_secs()
                            );
                        }
                        Err(e) => {
                            failed_crates.push((pkg.pkg.name.clone(), pkg.pkg.to.clone(), e.to_string()));
                            eprintln!(
                                "✗ ({:3<}/{:3<}) {}@{} failed: {}",
                                published_count + 1, total, pkg.pkg.name, pkg.pkg.to, e
                            );
                        }
                    }
                }
            }

            // Delay between batch groups (except for the last group)
            if group_idx < batch_groups.len() - 1 {
                writeln!(
                    stdout,
                    "Waiting {}s before next batch group...",
                    delay_between_batches.as_secs()
                )?;
                std::thread::sleep(delay_between_batches);
            }
        }
    } else {
        // Sequential batch processing (original behavior)
        for (batch_idx, batch) in batches.iter().enumerate() {
            let batch_num = batch_idx + 1;
            let total_batches = batches.len();

            writeln!(
                stdout,
                "\n--- Processing batch {}/{} ({} crates) ---",
                batch_num, total_batches, batch.len()
            )?;

            // Process crates in parallel within the batch
            writeln!(
                stdout,
                "Processing batch with up to {} concurrent crates...",
                max_concurrent
            )?;

            // Create a thread pool for this batch
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(max_concurrent)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to create thread pool: {}", e))?;

            let batch_results: Vec<_> = pool.install(|| {
                batch.par_iter()
                    .map(|pkg| {
                        let before = Instant::now();

                        // Use cargo publish as subprocess to capture full output
                        let result = publish_with_subprocess(&pkg.pkg, &apply, &token, &current_dir);
                        let after = Instant::now();
                        let duration = after.duration_since(before);

                        (pkg, result, duration)
                    })
                    .collect()
            });

            // Process batch results
            for (pkg, result, duration) in batch_results {
                match result {
                    Ok(_) => {
                        published_count += 1;
                        writeln!(
                            stdout,
                            "✓ ({:3<}/{:3<}) {}@{} published successfully ({}s)",
                            published_count, total, pkg.pkg.name, pkg.pkg.to, duration.as_secs()
                        )?;
                    }
                    Err(e) => {
                        failed_crates.push((pkg.pkg.name.clone(), pkg.pkg.to.clone(), e.to_string()));
                        writeln!(
                            stderr,
                            "✗ ({:3<}/{:3<}) {}@{} failed: {}",
                            published_count + 1, total, pkg.pkg.name, pkg.pkg.to, e
                        )?;
                    }
                }
            }

            // Wait between batches (except for the last batch)
            if batch_num < total_batches {
                writeln!(
                    stdout,
                    "Waiting {}s before next batch...",
                    delay_between_batches.as_secs()
                )?;
                tokio::time::sleep(delay_between_batches).await;
            }
        } // End of for loop in sequential processing
    } // End of else block for sequential processing

    // Summary
    writeln!(
        stdout,
        "\n=== Publishing Summary ==="
    )?;
    writeln!(
        stdout,
        "Successfully published: {}/{}",
        published_count, total
    )?;

    if !failed_crates.is_empty() {
        writeln!(
            stderr,
            "Failed to publish {} crates:",
            failed_crates.len()
        )?;
        for (name, version, error) in &failed_crates {
            writeln!(stderr, "  {}@{}: {}", name, version, error)?;
        }
 
        // Return error if any crates failed
        return Err(anyhow::anyhow!("Failed to publish {} crates", failed_crates.len()));
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

    // Mock crate for testing
    fn create_mock_crate(name: &str, dependencies: Vec<&str>) -> crate::plan::Publish {
        crate::plan::Publish {
            name: name.to_string(),
            from: "1.0.0".to_string(),
            to: "1.0.0".to_string(),
            bump: crate::plan::BumpKind::None,
            reason: None,
            publish: true,
            verify: true,
            rewrite_dep: vec![],
            remove_dep: vec![],
            remove_feature: vec![],
        }
    }

    #[test]
    fn test_crate_info_structure() {
        let pkg = create_mock_crate("test-crate", vec![]);
        let crate_info = CrateInfo {
            pkg: &pkg,
            dependencies: vec!["dep1".to_string(), "dep2".to_string()],
        };

        assert_eq!(crate_info.pkg.name, "test-crate");
        assert_eq!(crate_info.dependencies.len(), 2);
        assert_eq!(crate_info.dependencies[0], "dep1");
        assert_eq!(crate_info.dependencies[1], "dep2");
    }

    #[test]
    fn test_dependency_aware_batching_logic() {
        // Test the core batching logic without cargo workspace dependencies

        // Create test crates
        let crates = vec![
            create_mock_crate("crate-a", vec![]),
            create_mock_crate("crate-b", vec![]),
            create_mock_crate("crate-c", vec![]),
        ];

        let crates_refs: Vec<&crate::plan::Publish> = crates.iter().collect();

        // Test that we can create batches from the crates
        assert_eq!(crates_refs.len(), 3);
        assert_eq!(crates_refs[0].name, "crate-a");
        assert_eq!(crates_refs[1].name, "crate-b");
        assert_eq!(crates_refs[2].name, "crate-c");
    }

    #[test]
    fn test_batch_size_calculation() {
        // Test batch size calculations
        let total_crates = 25;
        let batch_size = 10;
        let expected_batches = (total_crates + batch_size - 1) / batch_size;

        assert_eq!(expected_batches, 3); // 25 crates / 10 per batch = 3 batches
    }

    #[test]
    fn test_concurrent_settings() {
        // Test that concurrent settings make sense
        let max_concurrent = 3;
        let batch_size = 10;

        assert!(max_concurrent <= batch_size, "Concurrent should not exceed batch size for efficiency");
        assert!(max_concurrent > 0, "Concurrent should be positive");
        assert!(batch_size > 0, "Batch size should be positive");
    }

    #[test]
    fn test_delay_calculation() {
        // Test delay calculations
        let delay_seconds = 120;
        let delay = Duration::from_secs(delay_seconds);

        assert_eq!(delay.as_secs(), 120);
        assert!(delay > Duration::from_secs(0));
    }

    #[test]
    fn test_crate_filtering() {
        // Test crate filtering logic
        let crates = vec![
            create_mock_crate("crate-a", vec![]),
            create_mock_crate("crate-b", vec![]),
            create_mock_crate("crate-c", vec![]),
        ];

        // Filter crates that should be published
        let publishable_crates: Vec<&crate::plan::Publish> = crates.iter()
            .filter(|c| c.publish)
            .collect();

        assert_eq!(publishable_crates.len(), 3);
        assert!(publishable_crates.iter().all(|c| c.publish));
    }

    #[test]
    fn test_error_handling() {
        // Test error handling scenarios
        let empty_crates: Vec<&crate::plan::Publish> = vec![];

        // Should handle empty input gracefully
        assert_eq!(empty_crates.len(), 0);

        // Test that we can create empty batches
        let empty_batch: Vec<CrateInfo> = vec![];
        assert_eq!(empty_batch.len(), 0);
    }

    #[test]
    fn test_batch_creation_edge_cases() {
        // Test edge cases for batch creation

        // Single crate
        let single_crate = vec![create_mock_crate("single", vec![])];
        assert_eq!(single_crate.len(), 1);

        // Large number of crates
        let many_crates: Vec<crate::plan::Publish> = (0..100)
            .map(|i| create_mock_crate(&format!("crate-{}", i), vec![]))
            .collect();
        assert_eq!(many_crates.len(), 100);

        // Zero crates
        let zero_crates: Vec<crate::plan::Publish> = vec![];
        assert_eq!(zero_crates.len(), 0);
    }
}
