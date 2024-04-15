use anyhow::Result;
use clap::Parser;

mod apply;
mod changed;
mod check;
mod claim;
mod cli;
mod config;
mod edit;
mod plan;
mod prdoc;
mod registry;
mod shared;
mod status;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Args::parse();

    match cli.comamnd {
        cli::Command::Status(status) => status::handle_status(status).await,
        cli::Command::Claim(claim) => claim::handle_claim(claim).await,
        cli::Command::Changed(changed) => changed::handle_changed(changed).await,
        cli::Command::Prdoc(prdoc) => prdoc::handle_prdoc(prdoc),
        cli::Command::Plan(plan) => plan::handle_plan(plan).await,
        cli::Command::Apply(apply) => apply::handle_apply(apply).await,
        cli::Command::Check(check) => check::handle_check(check).await,
        cli::Command::Config(config) => config::handle_config(config),
    }
}
