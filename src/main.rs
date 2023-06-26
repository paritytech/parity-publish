use anyhow::Result;
use clap::Parser;

mod changed;
mod claim;
mod cli;
mod shared;
mod status;
mod apply;
mod plan;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Args::parse();

    match cli.comamnd {
        cli::Command::Status(status) => status::handle_status(status).await,
        cli::Command::Claim(claim) => claim::handle_claim(claim).await,
        cli::Command::Changed(changed) => changed::handle_changed(changed).await,
        cli::Command::Plan(plan) => plan::handle_plan(plan).await,
        cli::Command::Apply(apply) => apply::handle_apply(apply).await,
    }
}
