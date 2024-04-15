use anyhow::Result;
use cargo::{
    core::Workspace,
    sources::{source::Source, RegistrySource},
    util::cache_lock::CacheLockMode,
    util_semver::VersionExt,
};
use std::{collections::HashSet, io::Write};
use termcolor::{ColorChoice, StandardStream};

use crate::{cli::Semver, registry};

pub fn handle_public_api(breaking: Semver) -> Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    let config = cargo::Config::default()?;
    config.shell().set_verbosity(cargo::core::Verbosity::Quiet);
    let path = breaking.path.canonicalize()?.join("Cargo.toml");
    let workspace = Workspace::new(&path, &config)?;

    let _lock = workspace
        .config()
        .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
    let mut reg = registry::get_registry(&workspace)?;

    writeln!(stdout, "looking up crates...",)?;
    registry::download_crates(&mut reg, &workspace, false)?;
    drop(_lock);

    writeln!(stdout, "compiling crates...",)?;

    for c in workspace.members() {
        if c.publish().is_some() {
            continue;
        }

        let _lock = workspace
            .config()
            .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;
        let upstream = registry::get_crate(&mut reg, c.name())?;
        drop(_lock);
        let upstream = upstream
            .iter()
            .filter(|c| !c.is_yanked())
            .filter(|c| !c.as_summary().version().is_prerelease())
            .max_by_key(|c| c.as_summary().version());

        let Some(upstream) = upstream else {
            continue;
        };

        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .quiet(true)
            .silent(true)
            .manifest_path(c.manifest_path())
            .build()?;

        let new = public_api::Builder::from_rustdoc_json(json_path).build()?;

        let upstream = Box::new(RegistrySource::remote(
            upstream.as_summary().source_id(),
            &HashSet::new(),
            workspace.config(),
        )?)
        .download_now(upstream.as_summary().package_id(), workspace.config())?;

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
