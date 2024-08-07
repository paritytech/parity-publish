use std::{
    env,
    io::{stdin, BufRead},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use cargo::core::Workspace;
use crates_io_api::AsyncClient;
use futures::future::join_all;

const PARITY_CRATE_OWNER_ID: u64 = 150167;

#[derive(Clone)]
pub enum Owner {
    Us,
    None,
    Other,
}

pub fn read_stdin(args: &mut Vec<String>) -> Result<()> {
    if let Some(n) = args.iter().position(|a| a == "-") {
        let stdin = stdin().lock();

        let lines = stdin.lines().collect::<std::result::Result<Vec<_>, _>>()?;
        let rest = args.split_off(n);
        args.extend(lines);
        args.extend(rest.into_iter().skip(1));
    }
    Ok(())
}

pub fn parity_crate_owner_id() -> u64 {
    env::var("PARITY_CRATE_OWNER_ID")
        .ok()
        .and_then(|var| var.parse().ok())
        .unwrap_or(PARITY_CRATE_OWNER_ID)
}

pub fn cratesio() -> Result<AsyncClient> {
    Ok(AsyncClient::new(
        &format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        Duration::from_millis(0),
    )?)
}

pub async fn get_owners(
    workspace: &Workspace<'_>,
    cratesio: &Arc<crates_io_api::AsyncClient>,
) -> Vec<Owner> {
    let owners = workspace
        .members()
        .map(|c| {
            let name = c.name().to_string();
            let cio = Arc::clone(cratesio);
            async move { cio.crate_owners(&name).await }
        })
        .collect::<Vec<_>>();
    let owners = join_all(owners).await;
    let owners = owners
        .into_iter()
        .map(|o| match o {
            Err(_) => Owner::None,
            Ok(v) if v.iter().any(|user| user.id == parity_crate_owner_id()) => Owner::Us,
            _ => Owner::Other,
        })
        .collect();
    owners
}

pub fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == Default::default()
}

pub fn is_not_default<T: Default + PartialEq>(t: &T) -> bool {
    *t != Default::default()
}

pub fn bool_true() -> bool {
    true
}
