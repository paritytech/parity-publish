use std::{
    env::current_dir,
    io::Write,
    process::Command,
};

use anyhow::{Context, Result};
use cargo::core::Workspace;
use cargo::util::toml_mut::manifest::LocalManifest;

use crate::{
    cli::{Args, Restore},
    edit,
    plan::{BumpKind, Planner},
};

pub fn handle_restore(args: Args, restore: Restore) -> Result<()> {
    let path = current_dir()?;
    let mut stdout = args.stdout();

    let plan_str = std::fs::read_to_string(path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have you run plan first?")?;
    let plan: Planner = toml::from_str(&plan_str)?;

    // Collect crates that were bumped
    let bumped: Vec<_> = plan
        .crates
        .iter()
        .filter(|c| c.bump != BumpKind::None)
        .collect();

    if bumped.is_empty() {
        writeln!(stdout, "No bumped crates found in Plan.toml")?;
        return Ok(());
    }

    writeln!(
        stdout,
        "Restoring clean manifests ({} crates bumped)",
        bumped.len()
    )?;

    if restore.dry_run {
        writeln!(stdout, "\nDry run — would restore from {}", restore.from)?;
    }

    // Step 1: Restore all Cargo.toml and Cargo.lock from the given git ref
    if !restore.dry_run {
        let status = Command::new("git")
            .args([
                "checkout",
                &restore.from,
                "--",
                "**/Cargo.toml",
                "Cargo.toml",
                "Cargo.lock",
            ])
            .current_dir(&path)
            .status()
            .context("failed to run git checkout")?;

        if !status.success() {
            anyhow::bail!(
                "git checkout {} failed (exit {}). Is the ref valid?",
                restore.from,
                status.code().unwrap_or(-1)
            );
        }

        writeln!(stdout, "Restored Cargo.toml files from {}", restore.from)?;
    }

    // Step 2: Re-open the workspace from the restored manifests and bump only versions
    if !restore.dry_run {
        let cargo_config = cargo::GlobalContext::default()?;
        cargo_config
            .shell()
            .set_verbosity(cargo::core::Verbosity::Quiet);
        let workspace = Workspace::new(&path.join("Cargo.toml"), &cargo_config)?;

        let workspace_crates: std::collections::BTreeMap<&str, &cargo::core::Package> = workspace
            .members()
            .map(|m| (m.name().as_str(), m))
            .collect();

        for pkg in &bumped {
            let Some(member) = workspace_crates.get(pkg.name.as_str()) else {
                writeln!(stdout, "  warning: {} not found in workspace, skipping", pkg.name)?;
                continue;
            };

            let mut manifest = LocalManifest::try_new(member.manifest_path())?;
            edit::set_version(&mut manifest, &pkg.to)?;
            manifest.write()?;
        }

        writeln!(stdout, "Applied {} version bumps", bumped.len())?;
    } else {
        for pkg in &bumped {
            writeln!(stdout, "  {} -> {}", pkg.name, pkg.to)?;
        }
    }

    // Step 3: Update lockfile
    if !restore.dry_run {
        writeln!(stdout, "Updating Cargo.lock...")?;
        let status = Command::new("cargo")
            .args(["update", "--workspace", "--offline"])
            .current_dir(&path)
            .status();

        let success = status.as_ref().map(|s| s.success()).unwrap_or(false);
        if !success {
            // Fallback to online update
            Command::new("cargo")
                .args(["update", "--workspace"])
                .current_dir(&path)
                .status()
                .context("failed to run cargo update")?;
        }
    }

    writeln!(stdout, "Done")?;
    Ok(())
}
