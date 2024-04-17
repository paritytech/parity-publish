use crate::cli::Status;
use crate::shared::{self, parity_crate_owner_id};

use anyhow::Result;
use cargo::core::Workspace;
use std::env::current_dir;
use std::io::Write;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

fn color_ok_red(stdout: &mut impl WriteColor, ok: bool, color: Color) -> Result<()> {
    if ok {
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
    } else {
        stdout.set_color(ColorSpec::new().set_fg(Some(color)))?;
    }

    Ok(())
}

pub async fn handle_status(status: Status) -> Result<()> {
    let config = cargo::Config::default()?;
    let path = current_dir()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let members = workspace.members();

    let cratesio = shared::cratesio()?;

    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

    if !status.quiet {
        stderr.set_color(ColorSpec::new().set_bold(true))?;
        writeln!(
            stderr,
            "{:<50}{:<16}{:<16}{:<0}",
            "Crate", "Local Ver", "crates.io Ver", "Owner"
        )?;
        stderr.set_color(ColorSpec::new().set_bold(false))?;
    }

    for member in members {
        // crates may have no publish set because the current workflow doesn't involve publishing
        // to crates.io
        // so keep this disabled for now just to be safe.
        if member.publish().is_some() {
            continue;
        }

        if let Ok(cra) = cratesio.full_crate(&member.name(), false).await {
            if status.missing {
                continue;
            }

            let versions_match = member.version().to_string().split('-').next().unwrap()
                == cra.max_version.split('-').next().unwrap();

            let owners = cra.owners;
            let parity_own = owners.iter().any(|user| user.id == parity_crate_owner_id());

            if status.external && parity_own {
                continue;
            }
            if status.version && versions_match {
                continue;
            }

            if !parity_own {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
            } else if !versions_match {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
            } else {
                stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
            }
            if status.quiet {
                write!(stdout, "{}", member.name())?;
            } else {
                write!(stdout, "{:<50}", member.name())?;
            }

            if status.quiet {
                continue;
            }

            color_ok_red(&mut stdout, versions_match, Color::Yellow)?;
            write!(stdout, "{:<16}{:<16}", member.version(), cra.max_version)?;

            color_ok_red(&mut stdout, parity_own, Color::Red)?;
            if parity_own {
                write!(stdout, "Parity")?;
            } else {
                write!(stdout, "External")?;
            }
        } else {
            color_ok_red(&mut stdout, false, Color::Red)?;
            if status.quiet {
                write!(stdout, "{}", member.name())?;
            } else {
                write!(
                    stdout,
                    "{:<50}{:<16}{:<16}{:<0}",
                    member.name(),
                    member.version(),
                    "Missing",
                    "No One"
                )?;
            }
        }

        stdout.set_color(ColorSpec::new().set_fg(None))?;
        writeln!(stdout)?;
    }

    Ok(())
}
