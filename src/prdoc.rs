use std::collections::HashMap;
use std::env::current_dir;
use std::fs::{read_dir, read_to_string};
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use cargo::core::Workspace;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::changed::{find_indirect_changes, get_changed_crates, Change, ChangeKind};
use crate::cli::{Prdoc, Semver};
use crate::plan::BumpKind;
use crate::public_api::{self, fmt_change};
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
            "none" => BumpKind::None,
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

    if prdoc.validate {
        return validate(&prdoc, &workspace);
    }

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

fn validate(prdoc: &Prdoc, w: &Workspace) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let Some(from) = &prdoc.since else {
        bail!("--since must be specified for --validate");
    };

    writeln!(stdout, "PR Doc validation is best effort")?;
    writeln!(
        stdout,
        "We can only detect the minimum guaranteed semver change"
    )?;
    writeln!(
        stdout,
        "It's possible to not detect changes or for changes to be greater than detected"
    )?;
    writeln!(stdout, "Always reason about semver changes yourself")?;
    writeln!(stdout)?;

    writeln!(stdout, "validating prdocs...")?;
    let prdocs = get_prdocs(w, &prdoc.prdoc_path, false, &prdoc.crates)?;

    writeln!(stdout, "checking file changes...")?;
    let changes = get_changed_crates(w, false, from, "HEAD")?;

    writeln!(stdout, "checking semver changes...")?;
    let mut crates = prdocs
        .iter()
        .map(|p| p.name.clone())
        .chain(changes.iter().map(|c| c.name.clone()))
        .collect::<Vec<_>>();
    crates.sort();
    crates.dedup();
    let breaking = Semver {
        paths: 0,
        quiet: true,
        major: false,
        verbose: false,
        since: Some(from.clone()),
        crates,
    };
    let (_tmp, upstreams) = public_api::get_from_commit(&w, &breaking, from)?;

    let breaking = public_api::get_changes(w, upstreams, &breaking, !prdoc.verbose)?;
    let mut ok = true;

    writeln!(stdout)?;

    for prdoc in &prdocs {
        let changed = changes.iter().any(|c| c.name == prdoc.name);
        let api_change = breaking.iter().find(|c| c.name == prdoc.name);

        let predicted = api_change.map(|b| b.bump).unwrap_or_else(|| {
            if changed {
                BumpKind::Patch
            } else {
                BumpKind::None
            }
        });

        if prdoc.bump == predicted || (prdoc.bump == BumpKind::None && predicted == BumpKind::Patch)
        {
            continue;
        }

        stdout.set_color(ColorSpec::new().set_bold(true))?;
        write!(stdout, "{}", prdoc.name)?;
        stdout.set_color(ColorSpec::new().set_bold(false))?;
        writeln!(stdout, " ({}):", prdoc.path.display())?;
        writeln!(stdout, "    PR Doc says change is {}", prdoc.bump)?;
        writeln!(stdout, "    Predicted semver change: {}", predicted)?;
        writeln!(stdout, "    Found file change: {}", yesno(changed))?;

        if let Some(api_change) = api_change {
            if api_change.bump == BumpKind::Major && prdoc.bump != BumpKind::Major {
                writeln!(
                    stdout,
                    "    Major API change found but prdoc specified {}",
                    prdoc.bump
                )?;
                ok = false;

                for change in &api_change.diff.removed {
                    stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                    writeln!(stdout, "   -{}", fmt_change(&change))?;
                }
                for change in &api_change.diff.changed {
                    stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                    writeln!(stdout, "   -{}", fmt_change(&change.old))?;
                    stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
                    writeln!(stdout, "   +{}", fmt_change(&change.new))?;
                }
            }
            if api_change.bump == BumpKind::Minor && prdoc.bump == BumpKind::Patch {
                // just warn don't return 1 for this
                writeln!(
                    stdout,
                    "    Minor API change found but prdoc specified {}",
                    prdoc.bump
                )?;

                for change in &api_change.diff.added {
                    stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                    writeln!(stdout, "   +{}", fmt_change(&change))?;
                }
            }
        }

        writeln!(stdout)?;
    }

    for change in &changes {
        if prdocs.iter().any(|p| p.name == change.name) {
            continue;
        }

        stdout.set_color(ColorSpec::new().set_bold(true))?;
        write!(stdout, "{}", change.name)?;
        stdout.set_color(ColorSpec::new().set_bold(false))?;
        writeln!(stdout, " ({}):", change.path.display())?;
        match change.kind {
            ChangeKind::Files => {
                writeln!(stdout, "    Files changed but crate not listed in PR Doc")?
            }
            ChangeKind::Manifest => writeln!(
                stdout,
                "    Cargo.toml changed but crate not listed in PR Doc"
            )?,
            _ => (),
        }
        writeln!(
            stdout,
            "    Predicted Semver change: {}",
            breaking
                .iter()
                .find(|b| b.name == change.name)
                .map(|b| b.bump)
                .unwrap_or(BumpKind::Patch)
        )?;
        ok = false;

        writeln!(stdout)?;
    }

    if !ok {
        std::process::exit(1);
    }

    Ok(())
}

fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}
