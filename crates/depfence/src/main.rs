//! Checks dependency-graph rules against the resolved package graph.
//!
//! The policy file groups its rules by package manager under `[tool.<name>]`.
//! Cargo is the only package manager supported so far.

mod cargo;
mod pattern;
mod policy;
mod report;

use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre;

use crate::policy::Policy;

#[derive(Parser)]
#[command(
    version,
    about = "check dependency-graph rules against the resolved package graph"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Check the policy against the project's dependency graph.
    Check(CheckArgs),
}

#[derive(clap::Args)]
struct CheckArgs {
    /// Path to the policy file.
    #[arg(long, default_value = "depfence.toml")]
    policy: PathBuf,
    /// Path to the root `Cargo.toml` of the workspace to check [default: `Cargo.toml` beside the
    /// policy file]
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Treat stale policy entries, such as allowances no crate uses, as a failure.
    #[arg(long)]
    strict: bool,
}

fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let Args {
        command: Command::Check(args),
    } = Args::parse();
    check(&args)
}

fn check(args: &CheckArgs) -> eyre::Result<()> {
    let policy = Policy::load(&args.policy)?;

    let mut findings = Vec::new();
    if let Some(cargo) = &policy.tool.cargo {
        let manifest_path = args
            .manifest_path
            .clone()
            .unwrap_or_else(|| args.policy.with_file_name("Cargo.toml"));
        findings.extend(cargo::check(cargo, &manifest_path)?);
    }

    report::print(&findings, policy.rule_count(), args.strict)
}
