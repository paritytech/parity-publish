use anyhow::Result;
use clap::Parser;

mod claim;
mod cli;
mod diff;
mod shared;
mod status;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Args::parse();

    match cli.comamnd {
        cli::Command::Status(status) => status::handle_status(status).await,
        cli::Command::Claim(claim) => claim::handle_claim(claim).await,
        cli::Command::Diff(diff) => diff::handle_diff(diff).await,
    }
}
