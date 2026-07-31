use std::collections::HashSet;
use std::task::Poll;

use anyhow::{anyhow, Result};
use cargo::sources::source::{QueryKind, Source};
use cargo::sources::IndexSummary;
use cargo::{
    core::{Dependency, SourceId, Workspace},
    sources::RegistrySource,
    util::interning::InternedString,
};
use semver::Version;

pub fn get_registry<'a>(workspace: &Workspace<'a>) -> Result<RegistrySource<'a>> {
    let whitelist = workspace.members().map(|c| c.package_id()).collect();
    let config = workspace.gctx();

    let mut reg = RegistrySource::remote(SourceId::crates_io(config)?, &whitelist, config)?;
    reg.invalidate_cache();

    Ok(reg)
}

/// Look up every index entry for a crate, *including yanked ones*.
///
/// `QueryKind::RejectedVersions` is used instead of `QueryKind::AlternativeNames`
/// because cargo filters yanked versions out of every other query kind. A yanked
/// version still owns its version number forever — crates.io answers a re-upload
/// with `crate version 'x' is already uploaded` — so version planning has to be
/// able to see them.
///
/// Callers that want "the version people can depend on" rather than "the version
/// numbers that are taken" must filter with [`is_usable`].
pub fn get_crate(reg: &mut RegistrySource, name: InternedString) -> Result<Vec<IndexSummary>> {
    match reg.query_vec(
        &Dependency::parse(name, None, reg.source_id())?,
        QueryKind::RejectedVersions,
    )? {
        Poll::Ready(c) if c.is_empty() => Err(anyhow!("not found")),
        Poll::Ready(c) => Ok(c),
        Poll::Pending => Err(anyhow!("pending")),
    }
}

/// Whether a release is available to depend on: not yanked, and not rejected by
/// the index for any other reason.
pub fn is_usable(summary: &IndexSummary) -> bool {
    matches!(summary, IndexSummary::Candidate(_))
}

/// Whether `version` is already taken on the registry.
///
/// Yanked versions count as taken: the number can never be reused.
pub fn version_taken(summaries: &[IndexSummary], version: &Version) -> bool {
    find_version(summaries, version).is_some()
}

/// The index entry for `version`, if that version was ever published.
pub fn find_version<'a>(
    summaries: &'a [IndexSummary],
    version: &Version,
) -> Option<&'a IndexSummary> {
    summaries
        .iter()
        .find(|s| s.as_summary().version() == version)
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

#[cfg(test)]
mod tests {
    use super::*;
    use cargo::util::cache_lock::CacheLockMode;

    /// cumulus-pov-validator 0.4.0 was published and then yanked. The planner
    /// bumped a later release onto that same number and the publish failed with
    /// `crate version '0.4.0' is already uploaded`, so the index lookup has to
    /// keep reporting it.
    #[test]
    #[ignore = "queries the crates.io index"]
    fn yanked_versions_are_visible_in_the_index() {
        let gctx = cargo::GlobalContext::default().unwrap();
        let manifest = std::env::current_dir().unwrap().join("Cargo.toml");
        let workspace = Workspace::new(&manifest, &gctx).unwrap();
        let _lock = gctx
            .acquire_package_cache_lock(CacheLockMode::DownloadExclusive)
            .unwrap();

        let mut reg = get_registry(&workspace).unwrap();
        let name: InternedString = "cumulus-pov-validator".into();

        // A query only schedules the index fetch. A cold cache needs more than
        // one round: the registry config has to arrive before the crate's own
        // index file can be requested.
        let mut summaries = None;
        for _ in 0..10 {
            match get_crate(&mut reg, name) {
                Ok(s) => {
                    summaries = Some(s);
                    break;
                }
                Err(_) => reg.block_until_ready().unwrap(),
            }
        }
        let summaries = summaries.expect("index lookup never became ready");
        let yanked = Version::parse("0.4.0").unwrap();

        assert!(
            version_taken(&summaries, &yanked),
            "yanked 0.4.0 must still count as a taken version number"
        );
        assert!(find_version(&summaries, &yanked).unwrap().is_yanked());
        assert!(
            !summaries
                .iter()
                .filter(|s| is_usable(s))
                .any(|s| s.as_summary().version() == &yanked),
            "a yanked version is not usable"
        );
        assert!(
            summaries.iter().any(is_usable),
            "the crate has usable releases too"
        );
    }
}
