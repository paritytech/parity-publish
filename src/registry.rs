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

    // `SourceId::crates_io` is hardwired to the *git* index. Since every call
    // here follows with `invalidate_cache`, that means refreshing a multi-GB git
    // repository on every single invocation. Use the sparse index instead, which
    // fetches only the index files for the crates actually being looked up.
    //
    // `crates_io_maybe_sparse_http` honours `registries.crates-io.protocol`, so
    // anyone who has deliberately pinned the git protocol still gets it.
    let mut reg = RegistrySource::remote(
        SourceId::crates_io_maybe_sparse_http(config)?,
        &whitelist,
        config,
    )?;
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
    match query_crate(reg, name)? {
        Poll::Ready(c) if c.is_empty() => Err(anyhow!("not found")),
        Poll::Ready(c) => Ok(c),
        Poll::Pending => Err(anyhow!("pending")),
    }
}

/// Same lookup as [`get_crate`], keeping the distinction between "this crate has
/// no releases" (`Ready` and empty) and "the index fetch has not finished yet"
/// (`Pending`, needs [`RegistrySource::block_until_ready`]).
///
/// The two must not be conflated: treating an unfinished fetch as an unpublished
/// crate makes the planner fall back to local manifest versions and plan
/// versions that are already published.
pub fn query_crate(
    reg: &mut RegistrySource,
    name: InternedString,
) -> Result<Poll<Vec<IndexSummary>>> {
    Ok(reg.query_vec(
        &Dependency::parse(name, None, reg.source_id())?,
        QueryKind::RejectedVersions,
    )?)
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

/// Number of schedule-then-fetch rounds [`download_crates`] will run before
/// giving up. Two is normally enough; the limit only guards against looping.
const DOWNLOAD_ROUNDS: usize = 5;

/// Populate the index cache so later [`get_crate`] calls resolve without
/// blocking.
///
/// A query only *schedules* the fetch, so this repeats until nothing is pending:
/// on a cold cache the registry config has to arrive before any crate's index
/// file can even be requested, and a single round leaves every lookup pending.
/// Each round schedules every crate before blocking, so the fetches stay
/// multiplexed rather than turning into one round trip per crate.
pub fn download_crates(reg: &mut RegistrySource, workspace: &Workspace, deps: bool) -> Result<()> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();

    for c in workspace.members().filter(|c| c.publish().is_none()) {
        names.push(c.name());
        seen.insert(c.name());
    }

    if deps {
        for cra in workspace.members() {
            for dep in cra.dependencies() {
                if dep.source_id().is_git() || dep.source_id().is_path() {
                    if !seen.contains(dep.package_name().as_str()) {
                        names.push(dep.package_name());
                        seen.insert(dep.package_name());
                    }
                }
            }
        }
    }

    for _ in 0..DOWNLOAD_ROUNDS {
        let mut pending = false;

        for name in &names {
            pending |= query_crate(reg, *name)?.is_pending();
        }

        if !pending {
            return Ok(());
        }

        reg.block_until_ready()?;
    }

    // Carrying on would mean planning from local manifest versions instead of
    // what the registry actually has, so fail loudly instead.
    Err(anyhow!(
        "registry index still not ready after {} fetch rounds",
        DOWNLOAD_ROUNDS
    ))
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
        for _ in 0..DOWNLOAD_ROUNDS {
            match query_crate(&mut reg, name).unwrap() {
                Poll::Ready(s) => {
                    summaries = Some(s);
                    break;
                }
                Poll::Pending => reg.block_until_ready().unwrap(),
            }
        }
        let summaries = summaries.expect("index lookup never became ready");
        assert!(
            !summaries.is_empty(),
            "cumulus-pov-validator has releases on crates.io"
        );
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
