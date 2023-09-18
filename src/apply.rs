use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, resolver::CliFeatures, Workspace},
    ops::{Packages, PublishOpts},
    util::{
        auth::Secret,
        toml_mut::{
            dependency::{PathSource, RegistrySource},
            manifest::LocalManifest,
        },
    },
};
use crates_io_api::AsyncClient;
use std::{env, io::Write, thread, time::Duration};
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Apply, plan, shared};

pub async fn handle_apply(apply: Apply) -> Result<()> {
    let path = apply.path.canonicalize()?;
    env::set_current_dir(&path)?;

    let plan = std::fs::read_to_string(apply.path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have your ran plan first?")?;
    let plan: plan::Planner = toml::from_str(&plan)?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    let token = if !apply.local {
        env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
            .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?
    } else {
        String::new()
    };

    let cratesio = shared::cratesio()?;

    writeln!(stdout, "rewriting deps...")?;

    for pkg in &plan.crates {
        let mut manifest = LocalManifest::try_new(&path.join(&pkg.path).join("Cargo.toml"))?;
        let package = manifest.manifest.get_table_mut(&["package".to_string()])?;
        let ver = package.get_mut("version").unwrap();
        *ver = toml_edit::value(&pkg.to);

        // hack because come crates don't have a desc
        if package.get("description").is_none() {
            package
                .as_table_mut()
                .unwrap()
                .insert("description", toml_edit::value(&pkg.name));
        }

        for dep in &pkg.rewrite_dep {
            let dep_name = dep.package.as_ref().unwrap_or(&dep.name);

            let exisiting_deps = manifest
                .get_dependency_versions(dep_name)
                .collect::<Vec<_>>();

            let mut new_ver = if let Some(v) = &dep.version {
                v.to_string()
            } else {
                plan.crates
                    .iter()
                    .inspect(|c| println!("{} {}", &c.name, dep_name))
                    .find(|c| &c.name == dep_name)
                    .unwrap()
                    .to
                    .clone()
            };

            if dep.exact {
                new_ver = format!("={}", new_ver);
            }

            for exisiting_dep in exisiting_deps {
                let (table, exisiting_dep) = exisiting_dep;
                let mut existing_dep = exisiting_dep?;
                let dev = table.kind() == DepKind::Development;

                if existing_dep.toml_key() == dep.name {
                    let table = table
                        .to_table()
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();

                    if let Some(path) = &dep.path {
                        let path = apply.path.canonicalize()?.join(path);
                        let mut source = PathSource::new(&path);

                        if dev {
                            existing_dep = existing_dep.clear_version();
                        } else {
                            source = source.set_version(&new_ver);
                        }
                        let existing_dep = existing_dep.set_source(source);
                        manifest.insert_into_table(&table, &existing_dep)?;
                    } else {
                        let source = RegistrySource::new(&new_ver);
                        let existing_dep = existing_dep.set_source(source);
                        manifest.insert_into_table(&table, &existing_dep)?;
                    }
                }
            }
        }

        for remove_feature in &pkg.remove_feature {
            let features = manifest.manifest.get_table_mut(&["features".to_string()])?;
            for feature in features.as_table_mut().unwrap().iter_mut() {
                if feature.0 == remove_feature.feature {
                    let needs = feature.1.as_array_mut().unwrap();
                    needs.retain(|need| need.as_str().unwrap() != remove_feature.value);
                }
            }
        }

        manifest.write()?;
    }

    if apply.local {
        return Ok(());
    }

    drop(workspace);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    let total = plan.crates.iter().filter(|c| c.publish).count();

    for (n, pkg) in plan.crates.iter().filter(|c| c.publish).enumerate() {
        if version_exists(&cratesio, &pkg.name, &pkg.to).await {
            writeln!(
                stdout,
                "({:3<}/{:3<}) {}-{} already published",
                n, total, pkg.name, pkg.to
            )?;
            continue;
        }

        writeln!(
            stdout,
            "({:3<}/{:3<}) publishing {}-{}...",
            n, total, pkg.name, pkg.to
        )?;

        let opts = PublishOpts {
            config: &config,
            token: Some(Secret::from(token.clone())),
            index: None,
            verify: pkg.verify && !apply.dry_run,
            allow_dirty: true,
            jobs: None,
            keep_going: false,
            to_publish: Packages::Packages(vec![pkg.name.clone()]),
            targets: Vec::new(),
            dry_run: apply.dry_run,
            registry: None,
            cli_features: CliFeatures::new_all(false),
        };
        cargo::ops::publish(&workspace, &opts)?;
        thread::sleep(Duration::from_secs(60));
    }

    Ok(())
}

async fn version_exists(cratesio: &AsyncClient, name: &str, ver: &str) -> bool {
    let c = cratesio.get_crate(name).await;
    if let Ok(c) = c {
        if c.versions.iter().any(|v| v.num == ver) {
            return true;
        }
    }

    false
}
