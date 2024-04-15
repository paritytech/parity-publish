use std::path::PathBuf;

use clap::{ArgAction, Parser};

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
    /// Find crates marked as changed in prdoc
    Prdoc(Prdoc),
    /// Find what crates have changed since last crates.io release
    Changed(Changed),
    /// Plan a publish
    Plan(Plan),
    /// Execute a publish
    Apply(Apply),
    /// Check crates are okay to publish
    Check(Check),
    /// Manage Plan.config
    Config(Config),
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
pub struct Prdoc {
    /// Don't include packages that have has a dependency change
    #[arg(long, short = 'd')]
    pub no_deps: bool,
    /// Just print paths, pass twice to print manifests
    #[arg(long, short, action = ArgAction::Count)]
    pub paths: u8,
    /// Only print crate names
    #[arg(long, short)]
    pub quiet: bool,
    /// Path to the cargo workspace
    pub path: PathBuf,
    /// Path to prdoc dir
    pub prdoc_path: PathBuf,
}

#[derive(Parser, Debug)]
pub struct Changed {
    /// Just print paths, pass twice to print manifests
    #[arg(long, short, action = ArgAction::Count)]
    pub paths: u8,
    /// Only print crate names
    #[arg(long, short)]
    pub quiet: bool,
    #[arg(long, short)]
    pub verbose: bool,
    /// Don't include packages that have has a dependency change
    #[arg(long, short = 'd')]
    pub no_deps: bool,
    /// Only show packages where the manifest changed
    #[arg(long, short)]
    pub manifests: bool,
    /// Path to the cargo workspace
    pub path: PathBuf,
    /// The git commit to look for changes from
    pub from: String,
    /// The git commit to look for changes to
    #[arg(default_value = "HEAD")]
    pub to: String,
}

#[derive(Parser, Debug)]
pub struct Plan {
    /// Suffix crate descriptions with given string
    #[arg(long, short)]
    pub description: Option<String>,
    /// add a pre release part to the published version
    #[arg(long, short)]
    pub pre: Option<String>,
    /// publish all crates
    #[arg(long, short)]
    pub all: bool,
    /// Publish crates that have changed since git ref
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    /// Calculate changes from prdocs
    pub prdoc: Option<PathBuf>,
    /// don't verify before publishing
    #[arg(long)]
    pub no_verify: bool,
    /// Create a new plan even if one exists
    #[arg(long)]
    pub new: bool,
    /// Don't run check during plan
    #[arg(long)]
    pub skip_check: bool,
    /// Patch bump the specified crates
    #[arg(long)]
    pub patch: bool,
    /// Don't bump versions when generating plan
    #[arg(long)]
    pub hold_version: bool,
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
    /// Allow dirty working directories to be published
    #[arg(long)]
    pub allow_dirty: bool,
    /// Don't verify packages before publish
    #[arg(long)]
    pub no_verify: bool,
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
    #[arg(long)]
    pub allow_nonfatal: bool,
    #[arg(long, short)]
    /// Only print crate names
    pub quiet: bool,
    #[arg(long, short, action = ArgAction::Count)]
    /// just print paths, pass twice to print manifests
    pub paths: u8,
    #[arg(long)]
    /// Dont check ownership status
    pub no_check_owner: bool,
    #[arg(long)]
    /// Dont exit 1 when crate is unpublished
    pub allow_unpublished: bool,
    #[arg(long, short)]
    /// recursively find what crates depend on unpublished crates
    pub recursive: bool,
}

#[derive(Parser, Debug)]
pub struct Config {
    #[arg(default_value = ".")]
    /// Path to the cargo workspace
    pub path: PathBuf,
    #[arg(long)]
    /// Apply changes specified in Plan.config
    pub apply: bool,
}
