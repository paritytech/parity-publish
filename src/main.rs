use anyhow::Result;
use clap::Parser;

mod claim;
mod cli;
mod shared;
mod status;

fn main() -> Result<()> {
    let cli = cli::Args::parse();

    match cli.comamnd {
        cli::Command::Status(status) => status::handle_status(status),
        cli::Command::Claim(claim) => claim::handle_claim(claim),
    }
}
