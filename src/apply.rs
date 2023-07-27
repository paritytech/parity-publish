use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, resolver::CliFeatures, Workspace},
    ops::{CompileOptions, Packages, PublishOpts},
    util::{
        auth::Secret,
        command_prelude::CompileMode,
        toml_mut::{
            dependency::{PathSource, RegistrySource, Source},
            manifest::LocalManifest,
        },
    },
};
use crates_io_api::AsyncClient;
use std::{env, io::Write, path::PathBuf, thread, time::Duration};
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Apply, plan, shared};

pub async fn handle_apply(apply: Apply) -> Result<()> {
    let path = apply.path.canonicalize()?;
    let plan = std::fs::read_to_string(apply.path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have your ran plan first?")?;
    let plan: plan::Planner = toml::from_str(&plan)?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    let token = env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
        .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?;

    let cratesio = shared::cratesio()?;

    writeln!(stdout, "rewriting deps...")?;

    for pkg in &plan.crates {
        //let cra =  workspace.members_mut().find(|m| m.name() == pkg.name).unwrap();
        //cra.manifest_mut().summary_mut().map_dependencies(f)

        let mut manifest = LocalManifest::try_new(&path.join(&pkg.path).join("Cargo.toml"))?;
        let package = manifest.manifest.get_table_mut(&["package".to_string()])?;
        let ver = package.get_mut("version").unwrap();
        *ver = toml_edit::value(&pkg.to);

        for dep in &pkg.rewrite_dep {
            let exisiting_deps = manifest
                .get_dependency_versions(&dep.name)
                .collect::<Vec<_>>();

            for exisiting_dep in exisiting_deps {
                let (table, exisiting_dep) = exisiting_dep;
                let mut existing_dep = exisiting_dep?;

                if existing_dep.toml_key() == dep.name {
                    let table = table
                        .to_table()
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                    let path = apply.path.canonicalize()?.join(&dep.path);
                    let mut source = PathSource::new(&path);

                    if !dep.dev {
                        source = source.set_version(&dep.version);
                    } else {
                        existing_dep = existing_dep.clear_version();
                    }
                    let existing_dep = existing_dep.set_source(source);
                    manifest.insert_into_table(&table, &existing_dep)?;
                }
            }
        }

        manifest.write()?;
    }

    if apply.local {
        return Ok(());
    }

    for pkg in &plan.crates {
        if !pkg.publish {
            continue;
        }

        if version_exists(&cratesio, &pkg.name, &pkg.to).await {
            writeln!(stdout, "{}-{} already published", pkg.name, pkg.to)?;
            continue;
        }

        writeln!(stdout, "publishing {}-{}...", pkg.name, pkg.to)?;

        let opts = PublishOpts {
            config: &config,
            token: Some(Secret::from(token.clone())),
            index: None,
            verify: true,
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

        if !apply.dry_run {
            for _ in 0..100 {
                writeln!(
                    stdout,
                    "waiting for {}-{} to become avaliable...",
                    pkg.name, pkg.to
                )?;
                thread::sleep(Duration::from_secs(10));
                if version_exists(&cratesio, &pkg.name, &pkg.to).await {
                    break;
                }
            }
        }
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
