use std::env::{current_dir, temp_dir};
use std::fs::{create_dir, remove_dir_all};
use std::io::Write;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs, thread};

use crate::cli::{Args, Claim};
use crate::shared::{self, get_owners, Owner};

use anyhow::{Context, Result};
use cargo::core::resolver::CliFeatures;
use cargo::core::Workspace;
use cargo::ops::{Packages, PublishOpts};
use termcolor::{Color, ColorSpec, WriteColor};

pub async fn handle_claim(args: Args, claim: Claim) -> Result<()> {
    let mut ret = 0;
    let config = cargo::GlobalContext::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = current_dir()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let token = if claim.dry_run {
        String::new()
    } else {
        env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
            .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?
    };

    let cratesio = Arc::new(shared::cratesio()?);

    let mut stdout = args.stdout();
    let mut stderr = args.stderr();
    let mut throttle = false;

    writeln!(stderr, "looking up crate data, this may take a while....")?;

    let owners = get_owners(&workspace, &cratesio).await;

    for (member, owner) in workspace.members().zip(owners) {
        if member.publish().is_some() {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
            writeln!(stdout, "{} is set to not publish", member.name())?;
            stdout.set_color(ColorSpec::new().set_fg(None))?;
            continue;
        }

        match owner {
            Owner::Us => (),
            Owner::Other => {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                writeln!(
                    stdout,
                    "{} exists and is owned by someone else",
                    member.name()
                )?;
                stdout.set_color(ColorSpec::new().set_fg(None))?;
                ret = 1;
            }
            Owner::None => {
                if member.publish().is_some() {
                    stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
                    writeln!(stdout, "{} is set to not publish", member.name())?;
                    stdout.set_color(ColorSpec::new().set_fg(None))?;
                    continue;
                }

                let manifest = write_manifest(&member.name())?;
                let opts = PublishOpts {
                    gctx: workspace.gctx(),
                    token: Some(token.clone().into()),
                    verify: false,
                    allow_dirty: true,
                    jobs: None,
                    keep_going: false,
                    to_publish: Packages::Default,
                    targets: Vec::new(),
                    dry_run: claim.dry_run,
                    cli_features: CliFeatures {
                        features: Default::default(),
                        all_features: false,
                        uses_default_features: true,
                    },
                    reg_or_index: None,
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
    }

    exit(ret);
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
description = "Reserved by Midnight while we work on an official release"
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
