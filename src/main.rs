use std::env::set_current_dir;

use anyhow::{Context, Result};
use clap::Parser;
use log::debug;

mod apply;
mod changed;
mod check;
mod claim;
mod cli;
mod config;
mod edit;
mod plan;
mod prdoc;
mod public_api;
mod registry;
mod shared;
mod status;
mod workspace;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    let args = cli.args;

    if let Some(path) = &args.chdir {
        set_current_dir(path).with_context(|| format!("cd {}", path.display()))?;
    }

    if args.debug {
        simple_logger::init()?;
    }

    debug!("{}-v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

    match cli.comamnd {
        cli::Command::Status(status) => status::handle_status(args, status).await,
        cli::Command::Claim(claim) => claim::handle_claim(args, claim).await,
        cli::Command::Changed(changed) => changed::handle_changed(args, changed).await,
        cli::Command::Prdoc(prdoc) => prdoc::handle_prdoc(args, prdoc),
        cli::Command::Semver(semver) => public_api::handle_public_api(args, semver),
        cli::Command::Plan(plan) => plan::handle_plan(args, plan).await,
        cli::Command::Apply(apply) => apply::handle_apply(args, apply).await,
        cli::Command::Check(check) => check::handle_check(args, check).await,
        cli::Command::Config(config) => config::handle_config(args, config),
        cli::Command::Workspace(workspace) => workspace::handle_workspace(args, workspace),
    }
}
