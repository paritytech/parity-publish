use std::{
    io::{stderr, stdout, IsTerminal},
    path::PathBuf,
};

use clap::{ArgAction, Parser};
use termcolor::{ColorChoice, StandardStream};

fn color(s: &str) -> Result<ColorChoice, &'static str> {
    match s {
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        "auto" if stdout().is_terminal() && stderr().is_terminal() => Ok(ColorChoice::Auto),
        "auto" => Ok(ColorChoice::Never),
        _ => Err("invalid value"),
    }
}

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(long, short = 'C')]
    pub chdir: Option<PathBuf>,
    #[arg(long, value_parser = color, default_value = "auto")]
    pub color: ColorChoice,
    #[arg(long)]
    pub debug: bool,
}

impl Args {
    pub fn stdout(&self) -> StandardStream {
        StandardStream::stdout(self.color)
    }
    pub fn stderr(&self) -> StandardStream {
        StandardStream::stdout(self.color)
    }
}

/// A tool to help with publishing crates
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Cli {
    #[command(flatten)]
    pub args: Args,
    #[command(subcommand)]
    pub comamnd: Command,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Checkes the status of the crates in a given workspace path against crates.io
    Status(Status),
    /// Claim ownership of unpublished crates on crates.io
    Claim(Claim),
    /// Find crates that have major or minor semver changes
    Semver(Semver),
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
    /// Query a workspace
    Workspace(Workspace),
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
}

#[derive(Parser, Debug)]
pub struct Claim {
    /// Don't actually claim crates
    #[arg(long, short)]
    pub dry_run: bool,
}

#[derive(Parser, Debug)]
pub struct Workspace {
    /// Only print crate names
    #[arg(long, short)]
    pub quiet: bool,
    /// Print packages that own given files
    #[arg(long, short)]
    pub owns: bool,
    /// targets to act on
    #[arg(default_values_t = Vec::<String>::new())]
    pub targets: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct Semver {
    /// Just print paths, pass twice to print manifests
    #[arg(long, short, action = ArgAction::Count)]
    pub paths: u8,
    /// Only print crate names
    #[arg(long, short)]
    pub quiet: bool,
    /// Only print breaking changes
    #[arg(long, short)]
    pub major: bool,
    /// Verbose output
    #[arg(long, short)]
    pub verbose: bool,
    /// Old version to compare against
    #[arg(long)]
    pub since: Option<String>,
    /// Rust toolchain to use
    #[arg(long, default_value = public_api::MINIMUM_NIGHTLY_RUST_VERSION)]
    pub toolchain: String,
    /// Print the minimum nightly rust version needed for semver checks
    #[arg(long)]
    pub minimum_nightly_rust_version: bool,
    /// Crates to check
    #[arg(default_values_t = Vec::<String>::new())]
    pub crates: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct Prdoc {
    /// Don't include packages that have has a dependency change
    #[arg(long, short = 'd')]
    pub no_deps: bool,
    /// Just print paths, pass twice to print manifests
    #[arg(long, short, action = ArgAction::Count)]
    pub paths: u8,
    /// Only print breaking changes
    #[arg(long, short)]
    pub major: bool,
    /// Verbose output
    #[arg(long, short)]
    pub verbose: bool,
    /// Only print crate names
    #[arg(long, short)]
    pub quiet: bool,
    /// Validate crate changes specified in prdocs
    #[arg(long)]
    pub since: Option<String>,
    /// Validate crate changes specified in prdocs
    #[arg(long)]
    pub validate: bool,
    /// Path to prdoc dir
    pub prdoc_path: PathBuf,
    /// Limit output to specified crates
    /// Rust toolchain to use
    #[arg(long, default_value = public_api::MINIMUM_NIGHTLY_RUST_VERSION)]
    pub toolchain: String,
    #[arg(default_values_t = Vec::<String>::new())]
    pub crates: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct Changed {
    /// Just print paths, pass twice to print manifests
    #[arg(long, short, action = ArgAction::Count)]
    pub paths: u8,
    /// Only print crate names
    #[arg(long, short)]
    pub quiet: bool,
    /// Verbose output
    #[arg(long, short)]
    pub verbose: bool,
    /// Don't include packages that have has a dependency change
    #[arg(long, short = 'd')]
    pub no_deps: bool,
    /// Only show packages where the manifest changed
    #[arg(long, short)]
    pub manifests: bool,
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
}

#[derive(Parser, Debug)]
pub struct Check {
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
    #[arg(long)]
    /// Apply changes specified in Plan.config
    pub apply: bool,
}
