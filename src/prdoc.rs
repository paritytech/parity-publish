use std::collections::HashMap;
use std::env::current_dir;
use std::fs::{read_dir, read_to_string};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use cargo::core::Workspace;
use termcolor::{ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::changed::{find_indirect_changes, Change, ChangeKind};
use crate::cli::Prdoc;
use crate::plan::BumpKind;
use crate::shared::read_stdin;

#[derive(serde::Deserialize)]
struct Document {
    crates: Vec<Crates>,
}

#[derive(serde::Deserialize)]
struct Crates {
    name: String,
    #[serde(default)]
    bump: String,
}

pub fn get_prdocs(
    workspace: &Workspace,
    path: &Path,
    deps: bool,
    filter: &[String],
) -> Result<Vec<Change>> {
    let mut entries = HashMap::new();

    if path.is_file() {
        read_prdoc(path, workspace, &mut entries)?;
    } else {
        let dirs = read_dir(path).context("failed to read prdoc dir")?;

        for dir in dirs {
            let dir = dir.context("failed to read prdoc dir")?;

            if dir.path().extension().unwrap_or_default() != "prdoc" {
                continue;
            }

            read_prdoc(&dir.path(), workspace, &mut entries)?;
        }
    }

    let mut entries = entries.into_values().into_iter().collect::<Vec<_>>();

    if !filter.is_empty() {
        entries.retain(|e| filter.contains(&e.name));
    }

    if deps {
        find_indirect_changes(workspace, &mut entries);
    }
    Ok(entries)
}

fn read_prdoc(
    path: &Path,
    workspace: &Workspace<'_>,
    entries: &mut HashMap<String, Change>,
) -> Result<(), anyhow::Error> {
    let prdoc = read_to_string(path).context("failed to read prdo")?;
    let prdoc: Document = serde_yaml::from_str(&prdoc)?;
    Ok(for c in prdoc.crates {
        let Some(package) = workspace.members().find(|m| m.name().as_str() == c.name) else {
            continue;
        };
        if package.publish().is_some() {
            continue;
        }
        let path = package.root().strip_prefix(workspace.root()).unwrap();
        let kind = ChangeKind::Files;
        let bump = match c.bump.as_str() {
            "patch" => BumpKind::Patch,
            "minor" => BumpKind::Minor,
            _ => BumpKind::Major,
        };
        let entry = entries.entry(c.name.to_string()).or_insert(Change {
            name: c.name.into(),
            path: path.into(),
            kind,
            bump,
        });
        entry.bump = entry.bump.max(bump);
    })
}

pub fn handle_prdoc(mut prdoc: Prdoc) -> Result<()> {
    read_stdin(&mut prdoc.crates)?;
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = current_dir()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let deps = !prdoc.no_deps;

    let prdocs = get_prdocs(&workspace, &prdoc.prdoc_path, deps, &prdoc.crates)?;

    for c in prdocs {
        if prdoc.major && c.bump != BumpKind::Major {
            continue;
        }

        if prdoc.paths >= 2 {
            writeln!(stdout, "{}", c.path.join("Cargo.toml").display())?;
        } else if prdoc.paths == 1 {
            writeln!(stdout, "{}", c.path.display())?;
        } else if prdoc.quiet {
            writeln!(stdout, "{}", c.name)?;
        } else {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{}", c.name)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({}):", c.path.display())?;
            writeln!(stdout, "    {}", c.kind)?;
            writeln!(stdout, "    {}", c.bump)?;
            writeln!(stdout)?;
        }
    }

    Ok(())
}
