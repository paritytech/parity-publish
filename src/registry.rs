use std::task::Poll;

use anyhow::{anyhow, Result};
use cargo::{
    core::{Dependency, QueryKind, Source, SourceId, Summary, Workspace},
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

pub fn get_crate(reg: &mut RegistrySource, name: InternedString) -> Result<Vec<Summary>> {
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
    for c in workspace.members().filter(|c| c.publish().is_none()) {
        let _ = get_crate(reg, c.name());
    }

    if deps {
        for cra in workspace.members() {
            for dep in cra.dependencies() {
                if dep.source_id().is_git() || dep.source_id().is_path() {
                    let _ = get_crate(reg, cra.name());
                }
            }
        }
    }

    reg.block_until_ready()?;
    Ok(())
}
