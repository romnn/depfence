//! `deny-feature` rules, which reject a crate feature that a workspace build enables for target
//! code.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use color_eyre::eyre;
use guppy::PackageId;
use guppy::graph::PackageGraph;
use guppy::graph::cargo::{CargoOptions, CargoSet};
use guppy::graph::feature::{FeatureId, StandardFeatures};
use owo_colors::OwoColorize as _;
use serde::Deserialize;

use crate::report::{Finding, Severity};

/// Denies `feature` of `crate` in target code, which is everything a workspace build compiles for
/// the target platform, tests included.
///
/// Build scripts and proc macros may still enable it, because Cargo resolves their features apart
/// from the target's.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct Rule {
    #[serde(rename = "crate")]
    crate_name: String,
    feature: String,
    /// Workspace members that only build scripts depend on, whose own target build is exempt.
    #[serde(default)]
    build_tools: Vec<String>,
    reason: String,
}

/// Reports each version of the rule's crate whose denied feature a workspace build enables for
/// target code.
///
/// The simulated build compiles every workspace member except the rule's build tools, with all
/// features and dev-dependencies, as `cargo test --workspace --all-features` would on any platform.
/// Cargo's resolver keeps build-dependency and proc-macro features on the host, so a build script
/// that asks for the feature leaves the target untouched.
///
/// The workspace-hack's edges stay in the simulation, because the features it requests reach every
/// member's build.
pub(super) fn check(graph: &PackageGraph, rule: &Rule) -> eyre::Result<Vec<Finding>> {
    let denied: Vec<FeatureId<'_>> = graph
        .packages()
        .filter(|package| package.name() == rule.crate_name)
        .filter_map(|package| {
            package
                .named_features()
                .find(|name| *name == rule.feature)
                .map(|name| FeatureId::named(package.id(), name))
        })
        .collect();
    // A crate or feature name that matches nothing would make the rule assert nothing.
    if denied.is_empty() {
        return Ok(vec![Finding {
            severity: Severity::Drift,
            headline: format!(
                "no package `{}` declares feature `{}`",
                rule.crate_name.yellow(),
                rule.feature.yellow()
            ),
            detail: "  the rule denies nothing; drop it or fix the crate or feature name\n"
                .to_owned(),
            reason: rule.reason.clone(),
        }]);
    }

    let mut findings = Vec::new();
    let mut tools = Vec::new();
    for name in &rule.build_tools {
        match graph
            .workspace()
            .iter()
            .find(|member| member.name() == name)
        {
            Some(member) => tools.push(member),
            None => findings.push(Finding {
                severity: Severity::Drift,
                headline: format!(
                    "`{}/{}` exempts `{}`, which is not a workspace member",
                    rule.crate_name.cyan(),
                    rule.feature.cyan(),
                    name.yellow()
                ),
                detail: "  the exemption covers nothing; drop it or fix the crate name\n"
                    .to_owned(),
                reason: rule.reason.clone(),
            }),
        }
    }

    let members = graph
        .workspace()
        .iter()
        .filter(|member| tools.iter().all(|tool| tool.id() != member.id()))
        .map(|member| member.id());
    let build = target_build(graph, members)?;
    for feature in &denied {
        if build.target_features().contains(*feature)? {
            findings.push(Finding {
                severity: Severity::Violation,
                headline: format!(
                    "`{}/{}` is enabled for target code",
                    rule.crate_name.cyan(),
                    rule.feature.yellow()
                ),
                detail: render_enablers(&build, feature.package_id(), rule)?,
                reason: rule.reason.clone(),
            });
        }
    }

    // An exemption stops being needed once the tool's own target build no longer enables the
    // feature.
    for tool in tools {
        let build = target_build(graph, [tool.id()])?;
        let mut enabled = false;
        for feature in &denied {
            enabled |= build.target_features().contains(*feature)?;
        }
        if !enabled {
            findings.push(Finding {
                severity: Severity::Drift,
                headline: format!(
                    "`{}/{}` exempts `{}`, whose target build does not enable it",
                    rule.crate_name.cyan(),
                    rule.feature.cyan(),
                    tool.name().yellow()
                ),
                detail: "  the exemption is unnecessary; drop it\n".to_owned(),
                reason: rule.reason.clone(),
            });
        }
    }
    Ok(findings)
}

/// Simulates building `members` with all features and dev-dependencies for any platform.
fn target_build<'graph>(
    graph: &'graph PackageGraph,
    members: impl IntoIterator<Item = &'graph PackageId>,
) -> eyre::Result<CargoSet<'graph>> {
    let mut options = CargoOptions::new();
    options.set_include_dev(true);
    let initials = graph
        .resolve_ids(members)?
        .to_feature_set(StandardFeatures::All);
    Ok(initials.into_cargo_set(&options)?)
}

/// Lists the packages whose target dependency on `package` requests the denied feature directly.
///
/// A `[features]` entry that forwards the feature leaves no such request, so the detail also prints
/// the `cargo tree` query that shows every edge.
fn render_enablers(build: &CargoSet<'_>, package: &PackageId, rule: &Rule) -> eyre::Result<String> {
    let requests =
        |req: guppy::graph::DependencyReq<'_>| req.features().any(|name| name == rule.feature);
    let enablers: BTreeSet<&str> = build
        .target_links()
        .filter(|link| link.to().id() == package)
        .filter(|link| {
            // Cargo follows dev-dependencies of workspace members only.
            requests(link.normal()) || (link.from().in_workspace() && requests(link.dev()))
        })
        .map(|link| link.from().name())
        .collect();
    let metadata = build.package_graph().metadata(package)?;
    let mut rendered = String::new();
    for enabler in &enablers {
        writeln!(rendered, "  {} {enabler}", "-".dimmed())?;
    }
    // The query mirrors the simulated build, and names the version so that it stays unambiguous
    // when the graph holds several.
    write!(
        rendered,
        "  {} cargo tree --workspace --all-features",
        "trace:".dimmed()
    )?;
    for tool in &rule.build_tools {
        write!(rendered, " --exclude {tool}")?;
    }
    writeln!(
        rendered,
        " -e normal,dev,features -i {}@{} --target all",
        metadata.name(),
        metadata.version()
    )?;
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use googletest::prelude::*;
    use guppy::graph::PackageGraph;

    use super::{Rule, check};
    use crate::cargo::fixture::{Dep, graph, policy};
    use crate::report::Severity;

    fn deny(feature: &str, build_tools: &[&str]) -> Rule {
        Rule {
            crate_name: "serde_json".to_owned(),
            feature: feature.to_owned(),
            build_tools: build_tools.iter().map(|name| (*name).to_owned()).collect(),
            reason: "test".to_owned(),
        }
    }

    /// Returns whether each finding is a violation, with its detail naming the enabling packages.
    fn findings(graph: &PackageGraph, rule: &Rule) -> eyre::Result<Vec<(bool, String)>> {
        Ok(check(graph, rule)?
            .into_iter()
            .map(|finding| (finding.severity == Severity::Violation, finding.detail))
            .collect())
    }

    /// A normal or dev-dependency that asks for the feature reaches target code and fails.
    #[test]
    fn target_and_dev_dependency_requests_are_violations() -> eyre::Result<()> {
        for kind in [None, Some("dev")] {
            let graph = graph(&[(
                "app",
                &[Dep {
                    name: "serde_json",
                    kind,
                    optional: false,
                    features: &["arbitrary_precision"],
                }],
            )])?;
            assert_that!(
                findings(&graph, &deny("arbitrary_precision", &[]))?,
                elements_are![(eq(&true), contains_substring(" app\n"))],
                "kind: {kind:?}"
            );
        }
        Ok(())
    }

    /// A build tool's request stays on the host for its dependents, and only its own target build
    /// needs the exemption.
    #[test]
    fn build_dependency_requests_stay_on_the_host() -> eyre::Result<()> {
        let graph = graph(&[
            (
                "app",
                &[
                    Dep {
                        name: "serde_json",
                        kind: None,
                        optional: false,
                        features: &[],
                    },
                    Dep {
                        name: "tool",
                        kind: Some("build"),
                        optional: false,
                        features: &[],
                    },
                ],
            ),
            (
                "tool",
                &[Dep {
                    name: "serde_json",
                    kind: None,
                    optional: false,
                    features: &["arbitrary_precision"],
                }],
            ),
        ])?;
        // The exempt tool's own build is the only target activation, so the rule holds.
        assert_that!(
            findings(&graph, &deny("arbitrary_precision", &["tool"]))?,
            is_empty()
        );
        // Without the exemption, the tool's own target build enables the feature.
        assert_that!(
            findings(&graph, &deny("arbitrary_precision", &[]))?,
            elements_are![(eq(&true), contains_substring(" tool\n"))]
        );
        Ok(())
    }

    /// Exemptions that grant nothing and features nobody declares are reported as drift.
    #[test]
    fn unused_exemptions_and_unknown_features_are_drift() -> eyre::Result<()> {
        let graph = graph(&[(
            "app",
            &[Dep {
                name: "serde_json",
                kind: None,
                optional: false,
                features: &[],
            }],
        )])?;
        // `app` is a member whose build never enables the feature, and `gone` is no member.
        assert_that!(
            findings(&graph, &deny("arbitrary_precision", &["app", "gone"]))?,
            elements_are![(eq(&false), anything()), (eq(&false), anything())]
        );
        // A misspelled feature would otherwise deny nothing.
        assert_that!(
            // spellcheck:ignore-next-line
            findings(&graph, &deny("arbitrary_precison", &[]))?,
            elements_are![(eq(&false), anything())]
        );
        Ok(())
    }

    /// The rule reads its kebab-case keys, and an omitted exemption list exempts nobody.
    #[test]
    fn deny_feature_rules_parse_kebab_case_keys() -> eyre::Result<()> {
        let policy = policy(
            r#"
            [[tool.cargo.deny-feature]]
            crate = "serde_json"
            feature = "arbitrary_precision"
            build-tools = ["schema-build"]
            reason = "tagged enums"

            [[tool.cargo.deny-feature]]
            crate = "serde_json"
            feature = "unbounded_depth"
            reason = "stack"
            "#,
        )?;
        let tools: Vec<_> = policy
            .deny_feature
            .iter()
            .map(|rule| rule.build_tools.clone())
            .collect();
        assert_that!(
            tools,
            elements_are![eq(&vec!["schema-build".to_owned()]), is_empty()]
        );
        Ok(())
    }
}
