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

pub fn get_registry<'a>(workspace: &Workspace<'a>) -> Result<RegistrySource<'a>> {
    let whitelist = workspace.members().map(|c| c.package_id()).collect();
    let config = workspace.config();

    let mut reg = RegistrySource::remote(SourceId::crates_io(config)?, &whitelist, config)?;
    reg.invalidate_cache();

    Ok(reg)
}

pub fn get_crate(reg: &mut RegistrySource, name: InternedString) -> Result<Vec<IndexSummary>> {
    match reg.query_vec(
        &Dependency::parse(name, None, reg.source_id())?,
        QueryKind::Fuzzy,
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
