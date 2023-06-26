use std::env;

const PARITY_CRATE_OWNER_ID: u64 = 150167;

pub fn parity_crate_owner_id() -> u64 {
    env::var("PARITY_CRATE_OWNER_ID")
        .ok()
        .and_then(|var| var.parse().ok())
        .unwrap_or(PARITY_CRATE_OWNER_ID)
}
