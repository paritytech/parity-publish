use crate::{
    cli::Check,
    shared::{cratesio, get_owners, Owner},
};

use std::{collections::BTreeSet, io::Write, process::exit, sync::Arc};

use anyhow::{Context, Result};
use cargo::core::{dependency::DepKind, Workspace};
use termcolor::{ColorChoice, ColorSpec, StandardStream, WriteColor};

pub async fn handle_check(chk: Check) -> Result<()> {
    exit(check(chk).await?)
}

pub async fn check(check: Check) -> Result<i32> {
    let path = check.path.canonicalize()?;
    let mut ret = 0;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    writeln!(stderr, "looking up crate data, this may take a while....")?;

    let owners = get_owners(&workspace, &Arc::new(cratesio()?)).await;

    for (c, owner) in workspace.members().zip(owners) {
        if c.publish().is_some() {
            continue;
        }

        match owner {
            Owner::Us => (),
            Owner::None => {
                stdout.set_color(ColorSpec::new().set_bold(true))?;
                write!(stdout, "{} is not claimed on crates.io", c.name())?;
                stdout.set_color(ColorSpec::new().set_bold(false))?;
                writeln!(stdout, " ({})", path.display())?;
                ret = 1;
            }
            Owner::Other => {
                stdout.set_color(ColorSpec::new().set_bold(true))?;
                write!(
                    stdout,
                    "{} is owned by some one else on crates.io",
                    c.name()
                )?;
                stdout.set_color(ColorSpec::new().set_bold(false))?;
                writeln!(stdout, " ({})", path.display())?;
                ret = 1;
            }
        }

        let path = c
            .manifest_path()
            .strip_prefix(workspace.root_manifest().parent().context("no parent")?)?;

        if !check.allow_nonfatal {
            if c.manifest().metadata().description.is_none() {
                stdout.set_color(ColorSpec::new().set_bold(true))?;
                write!(stdout, "{} has no description", c.name())?;
                stdout.set_color(ColorSpec::new().set_bold(false))?;
                writeln!(stdout, " ({})", path.display())?;

                ret = 1
            }
        }

        if c.manifest().metadata().license.is_none()
            && c.manifest().metadata().license_file.is_none()
        {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{} has no license:", c.name())?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({})", path.display())?;
            ret = 1;
        }

        if let Some(readme) = &c.manifest().metadata().readme {
            if !c
                .manifest_path()
                .parent()
                .context("no parent")?
                .join(readme)
                .exists()
            {
                stdout.set_color(ColorSpec::new().set_bold(true))?;
                write!(
                    stdout,
                    "{} specifies readme but the file does not exist:",
                    c.name()
                )?;
                stdout.set_color(ColorSpec::new().set_bold(false))?;
                writeln!(stdout, " ({})", path.display())?;
            }
        }
    }

    let mut new_publish = BTreeSet::new();
    let mut should_publish = workspace
        .members()
        .filter(|c| c.publish().is_none())
        .flat_map(|c| c.dependencies())
        .filter(|d| d.kind() != DepKind::Development)
        .map(|d| d.package_name().as_str())
        .collect::<BTreeSet<_>>();

    loop {
        new_publish = workspace
            .members()
            .filter(|c| new_publish.contains(c.name().as_str()))
            .flat_map(|c| c.dependencies())
            .filter(|d| d.kind() != DepKind::Development)
            .map(|d| d.package_name().as_str())
            .collect();

        if new_publish.is_empty() {
            break;
        }

        should_publish.extend(new_publish);
        new_publish = BTreeSet::new();
    }

    for c in workspace.members() {
        if should_publish.contains(c.name().as_str()) && c.publish().is_some() {
            let path = c
                .manifest_path()
                .strip_prefix(workspace.root_manifest().parent().context("no parent")?)?;

            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{} is no publish but a needed dependency", c.name())?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({})", path.display())?;
        }
    }

    return Ok(ret);
}
