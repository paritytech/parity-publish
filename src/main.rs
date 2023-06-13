use anyhow::Result;
use clap::Parser;

mod cli;
mod status;

fn main() -> Result<()> {
    let cli = cli::Args::parse();

    match cli.comamnd {
        cli::Command::Status(status) => status::handle_status(status),
    }
}
