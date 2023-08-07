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
    /// Find what crates have changed since last crates.io release
    Changed(Changed),
    /// Plan a publish
    Plan(Plan),
    /// Execute a publish
    Apply(Apply),
    /// Check crates are okay to publish
    Check(Check),
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
    /// Cache crates.io info
    #[arg(long, short)]
    pub cache: bool,
    /// Refresh the cache
    #[arg(long, short)]
    pub refresh: bool,
    /// add a pre release part to the published version
    #[arg(long, short)]
    pub pre: Option<String>,
    /// publish all crates even if they have not Changed
    #[arg(long, short)]
    pub all: bool,
    /// don't verify before publishing
    #[arg(long, short)]
    pub no_verify: bool,
    /// Use exact version for deps instead of semver
    #[arg(long, short)]
    pub exact: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Apply {
    /// Don't actually publish crates
    #[arg(long, short)]
    pub dry_run: bool,
    /// run changes to allow local testing
    #[arg(long, short)]
    pub local: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Check {
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}
