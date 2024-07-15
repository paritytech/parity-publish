use crate::{
    cli::{self, Args},
    shared::read_stdin,
};
use anyhow::Result;
use cargo::core::Workspace;
use std::{collections::HashSet, env::current_dir, io::Write, path::Path};

pub fn handle_workspace(args: Args, mut cli: cli::Workspace) -> Result<()> {
    read_stdin(&mut cli.targets)?;
    let config = cargo::GlobalContext::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = current_dir()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    if cli.owns {
        owns(&args, cli, &workspace)?;
    } else {
        members(&args, cli, &workspace)?;
    }

    Ok(())
}

fn owns(args: &Args, cli: cli::Workspace, w: &Workspace) -> Result<()> {
    let mut stdout = args.stdout();
    let mut stderr = args.stderr();
    let mut seen = HashSet::new();

    'outer: for targ in &cli.targets {
        for c in w.members() {
            if seen.contains(&c.name()) {
                continue;
            }

            let contains = if Path::new(targ) == c.root().strip_prefix(w.root()).unwrap()
                || Path::new(targ) == c.manifest_path().strip_prefix(w.root()).unwrap()
            {
                true
            } else {
                let mut src =
                    cargo::sources::PathSource::new(c.root(), c.package_id().source_id(), w.gctx());
                src.update().unwrap();
                let src_files = src.list_files(c)?;
                src_files
                    .into_iter()
                    .map(|f| f.strip_prefix(w.root()).unwrap().display().to_string())
                    .any(|f| &f == targ)
            };

            if contains {
                seen.insert(c.name());

                if cli.quiet {
                    writeln!(stdout, "{}", c.name(),)?;
                } else {
                    writeln!(
                        stdout,
                        "{} {}",
                        c.name(),
                        c.root().strip_prefix(w.root()).unwrap().display()
                    )?;
                }
                continue 'outer;
            }
        }

        writeln!(stderr, "error: can't find owner for '{}'", targ)?;
    }

    Ok(())
}

fn members(args: &Args, cli: cli::Workspace, w: &Workspace) -> Result<()> {
    let mut stdout = args.stdout();
    let mut stderr = args.stderr();

    for targ in &cli.targets {
        let Some(c) = w.members().find(|c| targ == c.name().as_str()) else {
            writeln!(stderr, "error: can't find package '{}'", targ)?;
            continue;
        };

        if cli.paths > 1 {
            writeln!(
                stdout,
                "{}",
                c.root()
                    .strip_prefix(w.root())
                    .unwrap()
                    .join("Cargo.toml")
                    .display()
            )?;
        } else if cli.quiet || cli.paths == 1 {
            writeln!(
                stdout,
                "{}",
                c.root().strip_prefix(w.root()).unwrap().display()
            )?;
        } else {
            writeln!(
                stdout,
                "{} {}",
                c.name(),
                c.root().strip_prefix(w.root()).unwrap().display()
            )?;
        }
    }

    Ok(())
}
