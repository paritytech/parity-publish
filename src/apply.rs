use anyhow::{Context, Result};
use cargo::{
    core::{dependency::DepKind, resolver::CliFeatures, Workspace},
    ops::{Packages, PublishOpts},
    util::{
        auth::Secret,
        toml_mut::{
            dependency::{PathSource, RegistrySource, Source},
            manifest::LocalManifest,
        },
    },
};
use std::{env, io::Write, path::PathBuf};
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Apply, plan};

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

            if pkg.name == "sc-client-api" {
                println!("{}, {:#?}", dep.name, exisiting_deps);
            }
            for exisiting_dep in exisiting_deps {
                let (table, exisiting_dep) = exisiting_dep;
                let existing_dep = exisiting_dep?;

                if existing_dep.name == dep.name {
                    let table = table
                        .to_table()
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                    let path = apply.path.canonicalize()?.join(&dep.path);
                    let mut source = PathSource::new(&path);

                    if !apply.local && !dep.dev {
                        source = source.set_version(&dep.version);
                    }
                    let source = Source::Path(source);
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
    }

    Ok(())
}
