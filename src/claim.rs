use std::env::temp_dir;
use std::fs::{create_dir, remove_dir_all};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use std::{env, fs, thread};

use crate::cli::Claim;
use crate::shared::{self, parity_crate_owner_id};

use anyhow::{Context, Result};
use cargo::core::resolver::CliFeatures;
use cargo::core::Workspace;
use cargo::ops::{Packages, PublishOpts};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

pub async fn handle_claim(claim: Claim) -> Result<()> {
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = claim.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let members = workspace.members();
    let token = if claim.dry_run {
        String::new()
    } else {
        env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
        .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?
    };

    let cratesio = shared::cratesio()?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
    let mut throttle = false;

    writeln!(
        stderr,
        "looking for crates to publish, this may take a while...."
    )?;

    for member in members {
        if let Ok(cra) = cratesio.full_crate(&member.name(), false).await {
            let owners = cra.owners;
            let parity_own = owners.iter().any(|user| user.id == parity_crate_owner_id());
            if !parity_own {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                writeln!(
                    stdout,
                    "{} exists and is owned by someone else",
                    member.name()
                )?;
                stdout.set_color(ColorSpec::new().set_fg(None))?;
            }
            if member.publish().is_some() {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
                writeln!(stdout, "{} is set to not publish", member.name())?;
                stdout.set_color(ColorSpec::new().set_fg(None))?;
                continue;
            }
        } else {
            if member.publish().is_some() {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
                writeln!(stdout, "{} is set to not publish", member.name())?;
                stdout.set_color(ColorSpec::new().set_fg(None))?;
                continue;
            }

            let manifest = write_manifest(&member.name())?;
            let opts = PublishOpts {
                config: &config,
                token: Some(token.clone().into()),
                index: None,
                verify: false,
                allow_dirty: true,
                jobs: None,
                keep_going: false,
                to_publish: Packages::Default,
                targets: Vec::new(),
                dry_run: claim.dry_run,
                registry: None,
                cli_features: CliFeatures {
                    features: Default::default(),
                    all_features: false,
                    uses_default_features: true,
                },
            };
            let workspace = Workspace::new(&manifest, &config)?;

            if !throttle && cargo::ops::publish(&workspace, &opts).is_err() {
                throttle = true;
            }

            if throttle {
                // crates.io rate limit
                thread::sleep(Duration::from_secs(60 * 10 + 5));
                cargo::ops::publish(&workspace, &opts)?;
            }

            remove_dir_all(manifest.parent().unwrap())?;
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Blue)))?;
            if claim.dry_run {
                writeln!(stdout, "published {} (dryrun)", member.name())?;
            } else {
                writeln!(stdout, "published {}", member.name())?;
            }
            stdout.set_color(ColorSpec::new().set_fg(None))?;
        }
    }

    Ok(())
}

fn write_manifest(name: &str) -> Result<PathBuf> {
    let dir = temp_dir().join("parity-publish");
    let manifest = dir.join("Cargo.toml");
    let _ = remove_dir_all(&dir);
    create_dir(&dir)?;

    fs::write(dir.join("lib.rs"), "")?;
    fs::write(dir.join("LICENSE"), "")?;

    fs::write(
        &manifest,
        format!(
            r#"

[package]
name = "{}"
description = "Reserved by Parity while we work on an official release"
version = "0.0.0"
license-file = "LICENSE"
include = ["LICENSE", "/lib.rs"]

[lib]
path = "lib.rs"
"#,
            name
        ),
    )?;

    Ok(manifest)
}
