use anyhow::{Result, Context};
use cargo::{core::{Workspace, dependency::DepKind, resolver::CliFeatures}, util::{toml_mut::{manifest::LocalManifest, dependency::{Source, RegistrySource}}, auth::Secret}, ops::{PublishOpts, Packages}};
use termcolor::{StandardStream, ColorChoice};
use std::{io::Write, env};

use crate::{cli::Apply, plan};

pub async fn handle_apply(apply: Apply) -> Result<()> {
    let path = apply.path.canonicalize()?;
    let plan = std::fs::read_to_string(apply.path.join("Plan.toml")).context("Can't find Plan.toml. Have your ran plan first?")?;
    let plan: plan::Planner = toml::from_str(&plan)?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

  let token =
        env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
            .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?;
    
    writeln!(stdout, "rewriting deps...")?;

    for pkg in &plan.publish {
       //let cra =  workspace.members_mut().find(|m| m.name() == pkg.name).unwrap();
       //cra.manifest_mut().summary_mut().map_dependencies(f)
       
        let mut manifest = LocalManifest::try_new(&path.join(&pkg.path).join("Cargo.toml"))?;
        let package = manifest.manifest.get_table_mut(&["package".to_string()])?;
        let ver = package.get_mut("version").unwrap();
        *ver = toml_edit::value(&pkg.to);

        for dep in &pkg.rewrite_dep {
            let exisiting_deps = manifest.get_dependency_versions(&dep.name).collect::<Vec<_>>();
            for exisiting_dep in exisiting_deps {
                let (table, exisiting_dep) = exisiting_dep;
                let existing_dep = exisiting_dep?;

                if existing_dep.name == dep.name {
                    let table = table.to_table().iter().map(|s| s.to_string()).collect::<Vec<_>>();
                    let source = Source::Registry(RegistrySource::new(&dep.version));
                    let existing_dep = existing_dep.set_source(source);
                    manifest.insert_into_table(&table, &existing_dep)?;
                }
            }

        }


        manifest.write()?;
    }


    for pkg in &plan.publish {
        writeln!(stdout, "publishing {}-{}...", pkg.name, pkg.name)?;

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
            dry_run: false,
            registry: None,
            cli_features: CliFeatures::new_all(false),
        };
        cargo::ops::publish(&workspace, &opts)?;
    }

    Ok(())
}
