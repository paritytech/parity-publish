use crate::{cli::Check, plan};

use std::io::Write;

use anyhow::{Context, Result};
use cargo::core::Workspace;
use termcolor::{ColorChoice, StandardStream};

pub async fn handle_check(check: Check) -> Result<()> {
    let path = check.path.canonicalize()?;
    let plan = std::fs::read_to_string(check.path.join("Plan.toml"))
        .context("Can't find Plan.toml. Have your ran plan first?")?;
    let plan: plan::Planner = toml::from_str(&plan)?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let workspace = Workspace::new(&path.join("Cargo.toml"), &config)?;

    for c in workspace.members() {
        if c.publish().is_none() && c.manifest().metadata().description.is_none() {
            writeln!(stdout, "{} has no description", c.name())?;
        }
    }

    Ok(())
}
