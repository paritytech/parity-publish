use std::{env, time::Duration};

use anyhow::Result;
use crates_io_api::AsyncClient;

const PARITY_CRATE_OWNER_ID: u64 = 150167;

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
