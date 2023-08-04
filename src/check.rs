use crate::{cli::Check, plan};

use std::io::Write;

use anyhow::{Context, Result};
use cargo::core::Workspace;
use termcolor::{ColorChoice, StandardStream};

pub async fn handle_check(check: Check) -> Result<()> {
    let path = check.path.canonicalize()?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    for c in workspace.members() {
        if c.publish().is_some() {
            continue;
        }

        if c.manifest().metadata().description.is_none() {
            writeln!(stdout, "{} has no description", c.name())?;
        }

        if c.manifest().metadata().license.is_none() {
            writeln!(stdout, "{} has no license", c.name())?;
        }
    }

    Ok(())
}
