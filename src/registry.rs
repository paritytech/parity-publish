use std::collections::HashSet;
use std::task::Poll;

use anyhow::{anyhow, Result};
use cargo::sources::source::{QueryKind, Source};
use cargo::sources::IndexSummary;
use cargo::{
    core::{Dependency, SourceId, Workspace},
    sources::RegistrySource,
    util::{cache_lock::CacheLockMode, interning::InternedString},
};

pub fn get_registry<'a>(workspace: &Workspace<'a>) -> Result<RegistrySource<'a>> {
    let whitelist = workspace.members().map(|c| c.package_id()).collect();
    let config = workspace.gctx();

    let mut reg = RegistrySource::remote(SourceId::crates_io(config)?, &whitelist, config)?;
    reg.invalidate_cache();

    Ok(reg)
}

pub fn get_crate(reg: &mut RegistrySource, name: InternedString) -> Result<Vec<IndexSummary>> {
    match reg.query_vec(
        &Dependency::parse(name, None, reg.source_id())?,
        QueryKind::Alternatives,
    )? {
        Poll::Ready(c) if c.is_empty() => Err(anyhow!("not found")),
        Poll::Ready(c) => Ok(c),
        Poll::Pending => Err(anyhow!("pending")),
    }
}

pub fn download_crates(reg: &mut RegistrySource, workspace: &Workspace, deps: bool) -> Result<()> {
    let mut seen = HashSet::new();

    for c in workspace.members().filter(|c| c.publish().is_none()) {
        let _ = get_crate(reg, c.name());
        seen.insert(c.name());
    }

    if deps {
        for cra in workspace.members() {
            for dep in cra.dependencies() {
                if dep.source_id().is_git() || dep.source_id().is_path() {
                    if !seen.contains(dep.package_name().as_str()) {
                        let _ = get_crate(reg, dep.package_name());
                    }
                }
            }
        }
    }

    reg.block_until_ready()?;
    Ok(())
}

/// Check if a specific version of a crate exists in the registry.
/// This creates a fresh registry source to ensure we get the latest index state.
pub fn version_exists_fresh(
    workspace: &Workspace,
    name: &str,
    version: &semver::Version,
) -> Result<bool> {
    let whitelist = workspace.members().map(|c| c.package_id()).collect();
    let config = workspace.gctx();
    let _lock = config.acquire_package_cache_lock(CacheLockMode::DownloadExclusive)?;

    let mut reg = RegistrySource::remote(SourceId::crates_io(config)?, &whitelist, config)?;
    reg.invalidate_cache();

    let crates = get_crate(&mut reg, name.into());
    reg.block_until_ready()?;

    match crates {
        Ok(c) => Ok(c.iter().any(|v| v.as_summary().version() == version)),
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    #[test]
    fn test_version_parsing() {
        // Test that version parsing works as expected
        let version = Version::parse("1.2.3").unwrap();
        assert_eq!(version.major, 1);
        assert_eq!(version.minor, 2);
        assert_eq!(version.patch, 3);
    }

    #[test]
    fn test_version_comparison() {
        let v1 = Version::parse("1.0.0").unwrap();
        let v2 = Version::parse("1.0.1").unwrap();
        let v3 = Version::parse("1.0.0").unwrap();

        assert!(v2 > v1);
        assert_eq!(v1, v3);
    }

    #[test]
    fn test_version_with_prerelease() {
        let stable = Version::parse("1.0.0").unwrap();
        let pre = Version::parse("1.0.0-alpha.1").unwrap();

        // Pre-release versions are less than stable
        assert!(pre < stable);
    }
}
