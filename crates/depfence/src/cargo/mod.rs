//! Rules for Cargo workspaces, checked against the graph `cargo metadata` resolves.
//!
//! A `forbid` rule rejects any path from a root package to a named crate.
//!
//! A `restrict` rule limits the crates that may depend directly on a guarded crate.
//!
//! A `deny-feature` rule rejects a crate feature that a workspace build enables for target code.
//!
//! `cargo deny` can express only the direct-dependency restriction.
//! Allowing a wrapper there would permit every other workspace crate to reach the guarded crate
//! transitively, so these checks query the graph directly.

mod deny_feature;
#[cfg(test)]
mod fixture;
mod forbid;
mod restrict;

use std::path::Path;

use color_eyre::eyre::{self, WrapErr as _};
use guppy::PackageId;
use guppy::graph::PackageGraph;
use serde::Deserialize;

use crate::report::Finding;

/// The `[tool.cargo]` table of the policy file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    #[serde(default)]
    forbid: Vec<forbid::Rule>,
    #[serde(default)]
    restrict: Vec<restrict::Rule>,
    #[serde(default, rename = "deny-feature")]
    deny_feature: Vec<deny_feature::Rule>,
}

impl Policy {
    pub(crate) fn rule_count(&self) -> usize {
        self.forbid.len() + self.restrict.len() + self.deny_feature.len()
    }
}

/// Checks every rule against the workspace whose root manifest is `manifest_path`.
///
/// # Errors
///
/// Fails when `cargo metadata` fails, when a rule names an unknown root package or an unknown
/// feature of a root, or when two `restrict` rules share a pattern.
pub(crate) fn check(policy: &Policy, manifest_path: &Path) -> eyre::Result<Vec<Finding>> {
    let manifest_path = std::path::absolute(manifest_path)
        .wrap_err_with(|| format!("failed to resolve {}", manifest_path.display()))?;
    // Guppy requests every feature, and Cargo metadata retains target-specific edges, so gated
    // dependencies cannot hide from the policy.
    let mut command = guppy::MetadataCommand::new();
    command.manifest_path(&manifest_path);
    // Cargo reads `.cargo/config.toml` from its working directory and that directory's ancestors,
    // so it runs in the workspace's directory, wherever depfence was started.
    if let Some(workspace_dir) = manifest_path.parent() {
        command.current_dir(workspace_dir);
    }
    let graph = command
        .build_graph()
        .wrap_err_with(|| format!("failed to resolve {}", manifest_path.display()))?;

    let hakari = hakari_package(&graph);
    let mut findings = Vec::new();
    for rule in &policy.forbid {
        findings.extend(forbid::check(&graph, rule, hakari)?);
    }
    findings.extend(restrict::check(&graph, &policy.restrict, hakari)?);
    for rule in &policy.deny_feature {
        findings.extend(deny_feature::check(&graph, rule)?);
    }
    Ok(findings)
}

/// The workspace member `cargo hakari` generates for this workspace, if it has one.
///
/// Every member depends on it and it depends on the union of their third-party dependencies, so a
/// walk that follows it reaches every dependency in the workspace from every crate.
/// Those edges exist so that Cargo resolves a dependency the same way everywhere and reuses the
/// artifact; they say nothing about which crate uses what, which is what the policy is about.
///
/// The rules compare package IDs, so a package from another source with the same name, such as the
/// `workspace-hack` placeholder on crates.io, is never exempt.
fn hakari_package(graph: &PackageGraph) -> Option<&PackageId> {
    let path = graph.workspace().root().join(".config/hakari.toml");
    let config: toml::Table = toml::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    workspace_member(graph, config.get("hakari-package")?.as_str()?)
}

fn workspace_member<'graph>(graph: &'graph PackageGraph, name: &str) -> Option<&'graph PackageId> {
    graph
        .workspace()
        .member_by_name(name)
        .ok()
        .map(|member| member.id())
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use googletest::prelude::*;

    use super::fixture::{dep, graph};
    use super::workspace_member;

    /// Only a workspace member can be the workspace-hack, so a package from another source that
    /// shares its name is never exempt from the rules.
    #[test]
    fn the_workspace_hack_is_a_workspace_member() -> eyre::Result<()> {
        let graph = graph(&[("app", &[dep("serde_json", None)])])?;

        assert_that!(workspace_member(&graph, "app").is_some(), eq(true));
        // The fixture's `serde_json` comes from the registry.
        assert_that!(workspace_member(&graph, "serde_json"), none());
        Ok(())
    }
}
