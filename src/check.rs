use crate::cli::Check;

use std::{io::Write, process::exit};

use anyhow::{Context, Result};
use cargo::core::Workspace;
use termcolor::{ColorChoice, ColorSpec, StandardStream, WriteColor};

pub async fn handle_check(chk: Check) -> Result<()> {
    check(chk).await
}

pub async fn check(check: Check) -> Result<()> {
    let path = check.path.canonicalize()?;
    let mut ret = 0;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    for c in workspace.members() {
        if c.publish().is_some() {
            continue;
        }

        let path = c
            .manifest_path()
            .strip_prefix(workspace.root_manifest().parent().context("no parent")?)?;

        if c.manifest().metadata().description.is_none() {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            writeln!(stdout, "{} has no description", c.name())?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, "        {}", path.display())?;

            if !check.allow_nonfatal {
                ret = 1
            }
        }

        if c.manifest().metadata().license.is_none()
            && c.manifest().metadata().license_file.is_none()
        {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            writeln!(stdout, "{} has no license:", path.display())?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, "        {}", path.display())?;
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
                writeln!(
                    stdout,
                    "{} specifies readme but the file does not exist:",
                    c.name()
                )?;
                stdout.set_color(ColorSpec::new().set_bold(false))?;
                writeln!(stdout, "        {}", path.display())?;
            }
        }
    }

    exit(ret)
}
