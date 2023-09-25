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
    /// add a pre release part to the published version
    #[arg(long, short)]
    pub pre: Option<String>,
    /// publish all crates
    #[arg(long, short)]
    pub all: bool,
    /// Publish crates that have changed
    #[arg(long)]
    pub changed: bool,
    /// don't verify before publishing
    #[arg(long)]
    pub no_verify: bool,
    /// Use exact version for deps instead of semver
    #[arg(long, short)]
    pub exact: bool,
    /// Create a new plan even if one exists.
    #[arg(long)]
    pub new: bool,
    #[arg(long)]
    pub skip_check: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
    pub crates: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct Apply {
    /// Don't actually publish crates
    #[arg(long, short)]
    pub dry_run: bool,
    /// Publish the crates
    #[arg(long, short)]
    pub publish: bool,
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Check {
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
    /// Dont exit 1 on errors that don't prevent publish
    #[arg(long, short)]
    pub allow_nonfatal: bool,
}
