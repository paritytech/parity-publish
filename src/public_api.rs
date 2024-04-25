use anyhow::{ensure, Result};
use cargo::{
    core::{Package, PackageSet, Workspace},
    sources::{source::SourceMap, RegistrySource},
    util::cache_lock::CacheLockMode,
    util_semver::VersionExt,
};
use cargo_semver_checks::ReleaseType;
use public_api::{diff::PublicApiDiff, PublicItem};
use std::{collections::HashSet, env::current_dir, path::PathBuf};
use std::{io::Write, process::Command};
use tempfile::TempDir;
use termcolor::{Color, WriteColor};
use termcolor::{ColorChoice, ColorSpec, StandardStream};

use crate::{cli::Semver, plan::BumpKind, registry, shared::read_stdin};

pub struct Change {
    pub name: String,
    pub path: PathBuf,
    pub bump: BumpKind,
    pub diff: PublicApiDiff,
}

pub fn handle_public_api(mut breaking: Semver) -> Result<()> {
    read_stdin(&mut breaking.crates)?;
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = current_dir()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;
    let _tmp;

    let upstreams = if let Some(commit) = &breaking.since {
        let (tmp, upstream) = get_from_commit(&workspace, &breaking, commit)?;
        _tmp = tmp;
        upstream
    } else {
        get_from_last_release(&workspace, &breaking)?
    };
    writeln!(stderr, "building crates...",)?;

    let changes = get_changes(&workspace, upstreams, &breaking, true)?;

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
                        writeln!(stdout, "   -{}", fmt_change(&change))?;
                    }
                    for change in &c.diff.changed {
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Red)))?;
                        writeln!(stdout, "   -{}", fmt_change(&change.old))?;
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
                        writeln!(stdout, "   +{}", fmt_change(&change.new))?;
                    }
                } else {
                    for change in &c.diff.added {
                        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Green)))?;
                        writeln!(stdout, "   +{}", fmt_change(&change))?;
                    }
                }
            }
            writeln!(stdout)?;
            stdout.set_color(&ColorSpec::new())?;
        }
    }

    Ok(())
}

pub fn get_from_commit(
    workspace: &Workspace,
    breaking: &Semver,
    commit: &str,
) -> Result<(TempDir, Vec<Package>)> {
    let dir = workspace.root().parent().unwrap();
    let dir = tempfile::TempDir::with_prefix_in("parity_publish-", dir)?;

    let status = Command::new("git")
        .arg("clone")
        .arg("-q")
        .arg("-n")
        .arg(workspace.root())
        .arg(dir.path())
        .status()?;
    ensure!(status.success(), "git exited non 0");

    let status = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .arg("checkout")
        .arg("-q")
        .arg(commit)
        .status()?;
    ensure!(status.success(), "git exited non 0");

    let mut upstream = Vec::new();
    let uworkspace = Workspace::new(&dir.path().join("Cargo.toml"), workspace.config())?;

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

        if let Some(u) = uworkspace.members().find(|u| c.name() == u.name()) {
            upstream.push(u.to_owned());
        }
    }

    Ok((dir, upstream))
}

fn get_from_last_release(workspace: &Workspace<'_>, breaking: &Semver) -> Result<Vec<Package>> {
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);

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
    Ok(upstreams)
}

pub fn get_changes(
    workspace: &Workspace<'_>,
    upstreams: Vec<cargo::core::Package>,
    breaking: &Semver,
    silent: bool,
) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

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
            "({:3<}/{:3<}) building {}-HEAD...",
            n,
            total,
            c.name(),
        )?;

        let json_path = rustdoc_json::Builder::default()
            .toolchain(public_api::MINIMUM_NIGHTLY_RUST_VERSION)
            .quiet(true)
            .silent(silent)
            .manifest_path(c.manifest_path())
            .build()?;

        let new = cargo_semver_checks::Rustdoc::from_path(&json_path);
        let new_diff = public_api::Builder::from_rustdoc_json(&json_path).build()?;
        let mut new = cargo_semver_checks::Check::new(new);

        n += 1;
        writeln!(
            stdout,
            "({:3<}/{:3<}) building {}-{}...",
            n,
            total,
            c.name(),
            upstream.version(),
        )?;

        let json_path = rustdoc_json::Builder::default()
            .toolchain(public_api::MINIMUM_NIGHTLY_RUST_VERSION)
            .quiet(true)
            .silent(silent)
            .manifest_path(upstream.manifest_path())
            .build()?;

        let path = c.root().strip_prefix(workspace.root()).unwrap();
        let old = cargo_semver_checks::Rustdoc::from_path(&json_path);
        let old_diff = public_api::Builder::from_rustdoc_json(&json_path).build()?;
        let report = new.with_baseline(old).check_release()?;

        let report = report.crate_reports().first_key_value().unwrap().1;
        let diff = public_api::diff::PublicApiDiff::between(old_diff, new_diff);

        if let Some(bump) = report.required_bump() {
            let bump = match bump {
                ReleaseType::Major => BumpKind::Major,
                ReleaseType::Minor => BumpKind::Minor,
                ReleaseType::Patch => BumpKind::Patch,
                _ => BumpKind::Major,
            };

            if !breaking.major || bump == BumpKind::Major {
                changes.push(Change {
                    name: c.name().to_string(),
                    path: path.to_owned(),
                    bump,
                    diff,
                });
            }
        }
    }

    Ok(changes)
}

pub fn fmt_change(s: &PublicItem) -> String {
    let mut ret = String::new();

    let s = s.to_string();
    let s = s
        .split(' ')
        .map(|s| s.rsplit("::").next().unwrap())
        .collect::<Vec<_>>()
        .join(" ");

    s.split(' ').fold(0, |mut n, word| {
        n += word.len() + 1;

        if n >= 140 {
            ret.push_str("\n    ");
            n = 0;
        }
        ret.push(' ');
        ret.push_str(word);
        n
    });
    ret.trim().to_string()
}
