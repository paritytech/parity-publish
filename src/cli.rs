use std::path::PathBuf;

use clap::Parser;

/// A tool to help with publishing crates
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    #[command(subcommand)]
    pub comamnd: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Checkes the status of the crates in a given workspace path against crates.io
    Status(Status),
    /// Claim ownership of unpublished crates on crates.io
    Claim(Claim),
    Changed(Changed),
    Plan(Plan),
    Apply(Apply),
}

#[derive(Parser, Debug)]
pub struct Status {
    /// Filter to only crates that are not on crates.io
    #[arg(long, short)]
    pub missing: bool,
    #[arg(long, short)]
    /// Filter to only crates that are not owned by parity on crates.io
    pub external: bool,
    #[arg(long, short)]
    /// Filter to only crates that do not match the version on crates.io
    pub version: bool,
    #[arg(long, short)]
    /// Only print crate names
    pub quiet: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Claim {
    /// Don't actually claim crates
    #[arg(long, short)]
    pub dry_run: bool,
    /// Yank crates that we already own
    #[arg(long, short)]
    pub yank: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Changed {
    #[arg(long, short)]
    pub verbose: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Plan {
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Apply {
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}
