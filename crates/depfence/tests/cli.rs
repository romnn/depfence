//! Runs the `depfence` binary against a small Cargo workspace in a temporary directory.

use std::path::Path;
use std::process::{Command, Output};

use color_eyre::eyre;
use googletest::prelude::*;
use indoc::{formatdoc, indoc};
use tempfile::TempDir;

/// Writes a workspace where `app` depends on `client`, which depends on `runtime`, with `policy`
/// as its `depfence.toml`.
fn workspace(policy: &str) -> eyre::Result<TempDir> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        indoc! {r#"
            [workspace]
            resolver = "3"
            members = ["app", "client", "runtime"]
        "#},
    )?;
    for (name, dependency) in [
        ("app", r#"client = { path = "../client" }"#),
        ("client", r#"runtime = { path = "../runtime" }"#),
        ("runtime", ""),
    ] {
        let member = root.join(name);
        std::fs::create_dir_all(member.join("src"))?;
        std::fs::write(
            member.join("Cargo.toml"),
            formatdoc! {r#"
                [package]
                name = "{name}"
                version = "0.1.0"
                edition = "2024"

                [dependencies]
                {dependency}
            "#},
        )?;
        std::fs::write(member.join("src").join("lib.rs"), "")?;
    }
    std::fs::write(root.join("depfence.toml"), policy)?;
    Ok(dir)
}

/// Runs `depfence check` on the workspace's policy, from a directory outside the workspace.
fn check(workspace: &TempDir, flags: &[&str]) -> eyre::Result<Output> {
    check_from(&std::env::temp_dir(), workspace, flags)
}

/// Runs `depfence check` on the workspace's policy from `caller_dir`, with plain output so that
/// assertions can match text.
fn check_from(caller_dir: &Path, workspace: &TempDir, flags: &[&str]) -> eyre::Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_depfence"))
        .arg("check")
        .arg("--policy")
        .arg(workspace.path().join("depfence.toml"))
        .args(flags)
        .current_dir(caller_dir)
        .env("NO_COLOR", "1")
        .output()?)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A forbidden transitive path fails the check and is printed edge by edge.
#[test]
fn a_forbidden_path_fails_with_the_path() -> eyre::Result<()> {
    let workspace = workspace(indoc! {r#"
        [[tool.cargo.forbid]]
        root = "app"
        crates = ["runtime"]
        reason = "the app stays free of the runtime"
    "#})?;
    let output = check(&workspace, &[])?;

    // The workspace beside the policy file is checked, and the path names the edge to cut.
    assert_that!(output.status.success(), eq(false));
    assert_that!(
        stdout(&output),
        all!(
            contains_substring("violation: `app` can reach `runtime`"),
            contains_substring("└─ client"),
            contains_substring("why: the app stays free of the runtime"),
        )
    );
    Ok(())
}

/// A policy that holds passes, and stale entries fail only under `--strict`.
#[test]
fn stale_entries_fail_only_under_strict() -> eyre::Result<()> {
    let workspace = workspace(indoc! {r#"
        [[tool.cargo.forbid]]
        root = "runtime"
        crates = ["app"]
        reason = "the runtime knows nothing of the app"

        [[tool.cargo.restrict]]
        crate = "client"
        only = ["app", "runtime"]
        reason = "only the app uses the client"
    "#})?;

    // The rules hold, so the `runtime` allowance is only stale.
    let lenient = check(&workspace, &[])?;
    assert_that!(lenient.status.success(), eq(true));
    assert_that!(
        stdout(&lenient),
        all!(
            contains_substring("stale policy: `client` allows crates that do not depend on it"),
            contains_substring("2 rules hold, but 1 policy entries are stale"),
        )
    );

    // Strict mode turns the stale entry into a failure.
    let strict = check(&workspace, &["--strict"])?;
    assert_that!(strict.status.success(), eq(false));
    Ok(())
}

/// A rule kind the tool does not know is an error, not a rule that silently checks nothing.
#[test]
fn an_unknown_rule_kind_is_an_error() -> eyre::Result<()> {
    let workspace = workspace(indoc! {r#"
        [[tool.cargo.forbids]]
        root = "app"
        crates = ["runtime"]
        reason = "misspelled"
    "#})?;
    let output = check(&workspace, &[])?;

    assert_that!(output.status.success(), eq(false));
    assert_that!(
        String::from_utf8_lossy(&output.stderr).into_owned(),
        contains_substring("failed to parse policy file")
    );
    Ok(())
}

/// Cargo resolves the workspace from the workspace's directory, so configuration in the caller's
/// directory does not apply to it.
#[test]
fn cargo_runs_in_the_workspace_directory() -> eyre::Result<()> {
    let workspace = workspace(indoc! {r#"
        [[tool.cargo.forbid]]
        root = "runtime"
        crates = ["app"]
        reason = "the runtime knows nothing of the app"
    "#})?;
    // Cargo fails on a malformed configuration file in its working directory.
    let caller = tempfile::tempdir()?;
    std::fs::create_dir(caller.path().join(".cargo"))?;
    std::fs::write(caller.path().join(".cargo").join("config.toml"), "not toml")?;

    let output = check_from(caller.path(), &workspace, &[])?;
    assert_that!(output.status.success(), eq(true));
    Ok(())
}
