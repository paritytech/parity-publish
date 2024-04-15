use anyhow::Result;
use cargo::{
    core::{PackageSet, Workspace},
    sources::{source::SourceMap, RegistrySource},
    util::cache_lock::CacheLockMode,
    util_semver::VersionExt,
};
use std::collections::HashSet;
use std::io::Write;
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Semver, registry};

pub fn handle_public_api(breaking: Semver) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = StandardStream::stderr(ColorChoice::Auto);
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

    for c in workspace.members() {
        let Some(upstream) = upstreams.iter().find(|u| c.name() == u.name()) else {
            continue;
        };

        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .quiet(true)
            .silent(true)
            .manifest_path(c.manifest_path())
            .build()?;

        let new = public_api::Builder::from_rustdoc_json(json_path).build()?;

        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .quiet(true)
            .silent(true)
            .manifest_path(upstream.manifest_path())
            .build()?;

        let old = public_api::Builder::from_rustdoc_json(json_path).build()?;

        let diff = public_api::diff::PublicApiDiff::between(old, new);
        if !diff.changed.is_empty() || !diff.removed.is_empty() {
            println!("{} {} major change", c.name(), upstream.version());
        } else if !diff.added.is_empty() {
            println!("{} {} minor change", c.name(), upstream.version());
        } else {
            println!("{} {} no change", c.name(), upstream.version());
        }
    }
    Ok(())
}
