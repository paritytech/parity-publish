use anyhow::Result;
use cargo::{
    core::{PackageSet, Workspace},
    sources::{source::SourceMap, RegistrySource},
    util::cache_lock::CacheLockMode,
    util_semver::VersionExt,
};
use public_api::{diff::PublicApiDiff, PublicItem};
use std::io::Write;
use std::{collections::HashSet, path::PathBuf};
use termcolor::{Color, WriteColor};
use termcolor::{ColorChoice, ColorSpec, StandardStream};

use crate::{cli::Semver, plan::BumpKind, registry, shared::read_stdin};

struct Change {
    name: String,
    path: PathBuf,
    bump: BumpKind,
    diff: PublicApiDiff,
}

pub fn handle_public_api(mut breaking: Semver) -> Result<()> {
    read_stdin(&mut breaking.crates)?;
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
    let mut changes = Vec::new();
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = breaking.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    let _lock = workspace
        .config()
        .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(&workspace)?;
    let mut upstreams = Vec::new();

    writeln!(stderr, "looking up crates...",)?;
    registry::download_crates(&mut reg, &workspace, false)?;

    writeln!(stderr, "downloading crates...",)?;

    for c in workspace.members() {
        if c.publish().is_some() {
            continue;
        }
        if c.library().is_none() {
            continue;
        }
        if !breaking.crates.is_empty() && !breaking.crates.iter().any(|n| n == c.name().as_str()) {
            continue;
        }

        let upstream = registry::get_crate(&mut reg, c.name())?;
        let upstream = upstream
            .iter()
            .filter(|c| !c.is_yanked())
            .filter(|c| !c.as_summary().version().is_prerelease())
            .max_by_key(|c| c.as_summary().version());

        let Some(upstream) = upstream else {
            continue;
        };

        upstreams.push(upstream.clone());
    }

    let ids = upstreams.iter().map(|c| c.package_id()).collect::<Vec<_>>();
    let mut sources = SourceMap::new();
    for c in &upstreams {
        let c = Box::new(RegistrySource::remote(
            c.as_summary().source_id(),
            &HashSet::new(),
            workspace.config(),
        )?);
        sources.insert(c);
    }
    let download = PackageSet::new(&ids, sources, workspace.config())?;
    let mut downloads = download.enable_download()?;
    let mut upstreams = Vec::new();

    for id in download.package_ids() {
        if let Some(pkg) = downloads.start(id)? {
            upstreams.push(pkg.clone());
        }
    }

    while downloads.remaining() != 0 {
        upstreams.push(downloads.wait()?.clone());
    }

    drop(downloads);
    drop(download);
    drop(_lock);
    writeln!(stderr, "building crates...",)?;

    let mut n = 0;
    let total = workspace
        .members()
        .filter(|c| upstreams.iter().any(|u| c.name() == u.name()))
        .count()
        * 2;

    for c in workspace.members() {
        let Some(upstream) = upstreams.iter().find(|u| c.name() == u.name()) else {
            continue;
        };

        n += 1;
        writeln!(
            stdout,
            "({:3<}/{:3<}) building {}-{}...",
            n,
            total,
            c.name(),
            c.version(),
        )?;

        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .quiet(true)
            .silent(true)
            .manifest_path(c.manifest_path())
            .build()?;

        let new = public_api::Builder::from_rustdoc_json(json_path).build()?;

        n += 1;
        writeln!(
            stdout,
            "({:3<}/{:3<}) building {}-HEAD...",
            n,
            total,
            c.name(),
        )?;

        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .quiet(true)
            .silent(true)
            .manifest_path(upstream.manifest_path())
            .build()?;

        let old = public_api::Builder::from_rustdoc_json(json_path).build()?;
        let path = c.root().strip_prefix(workspace.root()).unwrap();

        let diff = public_api::diff::PublicApiDiff::between(old, new);
        if !diff.changed.is_empty() || !diff.removed.is_empty() {
            changes.push(Change {
                name: c.name().to_string(),
                path: path.to_owned(),
                bump: BumpKind::Major,
                diff,
            })
        } else if !diff.added.is_empty() && !breaking.major {
            changes.push(Change {
                name: c.name().to_string(),
                path: path.to_owned(),
                bump: BumpKind::Minor,
                diff,
            })
        }
    }

    for c in changes {
        if breaking.paths >= 2 {
            writeln!(stdout, "{}", c.path.join("Cargo.toml").display())?;
        } else if breaking.paths == 1 {
            writeln!(stdout, "{}", c.path.display())?;
        } else if breaking.quiet {
            writeln!(stdout, "{}", c.name)?;
        } else {
            stdout.set_color(ColorSpec::new().set_bold(true))?;
            write!(stdout, "{}", c.name)?;
            stdout.set_color(ColorSpec::new().set_bold(false))?;
            writeln!(stdout, " ({}):", c.path.display())?;
            writeln!(stdout, "    {}", c.bump)?;
            if breaking.verbose {
                if c.bump == BumpKind::Major {
                    for change in &c.diff.removed {
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                        writeln!(stdout, "   -{}", split_change(&change))?;
                    }
                    for change in &c.diff.changed {
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                        writeln!(stdout, "   -{}", split_change(&change.old))?;
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
                        writeln!(stdout, "   +{}", split_change(&change.new))?;
                    }
                } else {
                    for change in &c.diff.added {
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
                        writeln!(stdout, "   +{}", split_change(&change))?;
                    }
                }
            }
            writeln!(stdout)?;
            stdout.set_color(&ColorSpec::new())?;
        }
    }

    Ok(())
}

fn split_change(s: &PublicItem) -> String {
    let mut ret = String::new();

    for (n, c) in s.to_string().chars().enumerate() {
        if (n + 1) % 120 == 0 {
            ret.push_str("\n    ");
        }
        ret.push(c);
    }
    ret
}
