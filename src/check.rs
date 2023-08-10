use crate::cli::Check;

use std::{io::Write, process::exit};

use anyhow::Result;
use cargo::core::Workspace;
use termcolor::{ColorChoice, StandardStream};

pub async fn handle_check(check: Check) -> Result<()> {
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

        if c.manifest().metadata().description.is_none() {
            writeln!(stdout, "{} has no description", c.name())?;
            ret = 1
        }

        if c.manifest().metadata().license.is_none() {
            writeln!(stdout, "{} has no license", c.name())?;
            ret = 1;
        }
    }

    exit(ret)
}
