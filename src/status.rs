use crate::cli::Status;
use anyhow::{Context, Result};
use cargo::core::Workspace;
use crates_io::Registry;
use std::env;
use std::io::Write;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

const PARITY_CRATE_OWNER_ID: u32 = 150167;
//const PARITY_CORE_DEVS_ID: u32 = 27;

fn color_ok_red(stdout: &mut impl WriteColor, ok: bool, color: Color) -> Result<()> {
    if ok {
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
    } else {
        stdout.set_color(ColorSpec::new().set_fg(Some(color)))?;
    }

    Ok(())
}

pub fn handle_status(status: Status) -> Result<()> {
    let config = cargo::Config::default()?;
    let path = status.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let members = workspace.members();
    let mut curl = curl::easy::Easy::new();
    curl.useragent(&format!(
        "{}/{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    ))?;
    let token = env::var("PARITY_PUBLISH_CRATESIO_TOKEN")
        .context("PARITY_PUBLISH_CRATESIO_TOKEN must be set")?;

    let mut cratesio =
        Registry::new_handle("https://crates.io".to_string(), Some(token), curl, false);

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
        //if member.publish().is_some() {
        //    continue;
        //}

        let upstream = cratesio.search(&member.name(), 1)?.0;
        match upstream.as_slice() {
            [cra] if cra.name == member.name().as_str() => {
                if status.missing {
                    continue;
                }

                let versions_match = member.version().to_string().split('-').next().unwrap()
                    == cra.max_version.split('-').next().unwrap();

                let owners = cratesio.list_owners(&member.name())?;
                let parity_own = owners.iter().any(|user| user.id == PARITY_CRATE_OWNER_ID);

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
            }
            _ => {
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
        }

        stdout.set_color(ColorSpec::new().set_fg(None))?;
        writeln!(stdout)?;
    }

    Ok(())
}
