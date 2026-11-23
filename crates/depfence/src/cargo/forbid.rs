//! `forbid` rules, which reject any dependency path from a root package to a forbidden crate.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;

use color_eyre::eyre;
use guppy::PackageId;
use guppy::graph::feature::{ConditionalLink, FeatureId};
use guppy::graph::{DependencyDirection, PackageGraph, PackageLink, PackageMetadata};
use owo_colors::OwoColorize as _;
use serde::Deserialize;

use crate::pattern;
use crate::report::{Finding, Severity};

/// Prevents `root` from reaching any crate named in `crates`.
///
/// Both `root` and the `crates` entries accept an exact crate name or a prefix ending in `*`.
///
/// Without `features` and `default-features`, the rule judges the all-features graph.
/// With either key, it judges only what a build of the root with exactly those features compiles.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct Rule {
    root: String,
    crates: Vec<String>,
    /// Features of the root to enable, which every matching root must declare.
    features: Option<Vec<String>>,
    /// Whether the root's `default` feature is enabled, which is `true` unless set, as in Cargo.
    default_features: Option<bool>,
    reason: String,
}

impl Rule {
    /// Returns whether the rule selects the root's features instead of enabling all of them.
    fn selects_features(&self) -> bool {
        self.features.is_some() || self.default_features.is_some()
    }
}

/// A dependency edge between two packages, however many declarations it carries.
#[derive(PartialEq, Eq, Hash)]
struct Edge<'graph> {
    from: &'graph PackageId,
    to: &'graph PackageId,
}

/// Reports the shortest path from `rule.root` to each forbidden crate.
///
/// Normal and build dependencies participate because both can affect a production build.
/// Dev-dependency edges are skipped because they affect only development targets.
pub(super) fn check(
    graph: &PackageGraph,
    rule: &Rule,
    hakari: Option<&PackageId>,
) -> eyre::Result<Vec<Finding>> {
    let roots: Vec<PackageMetadata<'_>> = graph
        .packages()
        .filter(|package| pattern::matches(package.name(), &rule.root))
        // The workspace-hack depends on every third-party crate without using any, so a root glob
        // that matches its name does not select it.
        .filter(|package| Some(package.id()) != hakari)
        .collect();
    if roots.is_empty() && !rule.root.ends_with('*') {
        eyre::bail!("policy names unknown root package `{}`", rule.root);
    }

    // An unknown forbidden crate would make this entry assert nothing, so report it as drift.
    // This catches obsolete names and typos without treating deliberate preemptive bans as
    // violations.
    // A glob that matches nothing may be a healthy preemptive ban, so only exact names count.
    let mut findings: Vec<Finding> = rule
        .crates
        .iter()
        .filter(|name| !name.ends_with('*') && package_by_name(graph, name).is_none())
        .map(|name| Finding {
            severity: Severity::Drift,
            headline: format!(
                "`{}` forbids `{}`, which is not in the graph",
                rule.root.cyan(),
                name.yellow()
            ),
            detail: "  the entry asserts nothing; drop it or fix the crate name\n".to_owned(),
            reason: rule.reason.clone(),
        })
        .collect();

    for root in roots {
        findings.extend(forbidden_paths(graph, rule, root, hakari)?);
    }
    Ok(findings)
}

/// Reports the shortest path from one root to each crate `rule` forbids.
///
/// A rule that selects the root's features walks only the edges that build activates.
fn forbidden_paths(
    graph: &PackageGraph,
    rule: &Rule,
    root: PackageMetadata<'_>,
    hakari: Option<&PackageId>,
) -> eyre::Result<Vec<Finding>> {
    let activated = if rule.selects_features() {
        Some(activated_edges(graph, rule, root, hakari)?)
    } else {
        None
    };
    let mut findings = Vec::new();

    // Track parents during one breadth-first walk so each violation reports a shortest path
    // instead of only identifying a reachable crate.
    let mut came_from: HashMap<&PackageId, Option<&PackageId>> = HashMap::new();
    came_from.insert(root.id(), None);
    let mut queue = VecDeque::from([root.id()]);

    while let Some(current) = queue.pop_front() {
        let package = graph.metadata(current)?;
        if package.id() != root.id() && pattern::matches_any(&rule.crates, package.name()) {
            findings.push(Finding {
                severity: Severity::Violation,
                headline: format!(
                    "`{}` can reach `{}`",
                    root.name().cyan(),
                    package.name().yellow()
                ),
                detail: render_path(graph, &came_from, current)?,
                reason: rule.reason.clone(),
            });
            // Keep walking rather than treating a forbidden crate as a wall: reporting every
            // distinct violation in one run beats failing on whichever came first.
        }
        for link in package
            .direct_links()
            .filter(links_into_artifact)
            .filter(|link| Some(link.to().id()) != hakari)
            .filter(|link| {
                activated.as_ref().is_none_or(|edges| {
                    edges.contains(&Edge {
                        from: link.from().id(),
                        to: link.to().id(),
                    })
                })
            })
        {
            let next = link.to().id();
            if !came_from.contains_key(next) {
                came_from.insert(next, Some(current));
                queue.push_back(next);
            }
        }
    }

    Ok(findings)
}

/// Returns the edges a build of `root` with the rule's features compiles.
///
/// Guppy's feature graph models Cargo's feature unification, so an optional dependency counts only
/// once a selected feature, or a feature another dependency requests, activates it.
/// The resolve follows the edges an all-features walk follows: normal and build dependencies, but
/// none into the workspace-hack, whose features would otherwise activate everything.
fn activated_edges<'graph>(
    graph: &'graph PackageGraph,
    rule: &Rule,
    root: PackageMetadata<'graph>,
    hakari: Option<&PackageId>,
) -> eyre::Result<HashSet<Edge<'graph>>> {
    let mut initials = vec![FeatureId::base(root.id())];
    if rule.default_features.unwrap_or(true) && root.has_default_feature() {
        initials.push(root.default_feature_id());
    }
    for name in rule.features.iter().flatten() {
        let Some(feature) = root.named_features().find(|declared| declared == name) else {
            eyre::bail!(
                "policy enables unknown feature `{name}` of root package `{}`",
                root.name()
            );
        };
        initials.push(FeatureId::named(root.id(), feature));
    }

    let follows = |link: &ConditionalLink<'_>| {
        (link.normal().is_present() || link.build().is_present())
            && Some(link.to().package().id()) != hakari
    };
    let resolved = graph
        .feature_graph()
        .query_forward(initials)?
        .resolve_with_fn(|_, link| follows(&link));
    // A followed link into a dependency's base feature compiles that dependency.
    // Links into its named features only choose what it compiles with, and a weak `dep?/feature`
    // link can join two resolved features without activating the dependency.
    let mut edges = HashSet::new();
    for link in resolved
        .conditional_links(DependencyDirection::Forward)
        .filter(|link| link.to().feature_id().is_base() && follows(link))
    {
        edges.extend(link.package_links().map(|link| Edge {
            from: link.from().id(),
            to: link.to().id(),
        }));
    }
    Ok(edges)
}

/// Returns whether `link` can affect a production build.
///
/// Normal and build dependencies count; dev dependencies do not.
fn links_into_artifact(link: &PackageLink<'_>) -> bool {
    link.normal().is_present() || link.build().is_present()
}

fn package_by_name<'graph>(
    graph: &'graph PackageGraph,
    name: &str,
) -> Option<PackageMetadata<'graph>> {
    graph.packages().find(|package| package.name() == name)
}

/// Renders a dependency path as an indented chain that identifies the edge to cut.
fn render_path(
    graph: &PackageGraph,
    came_from: &HashMap<&PackageId, Option<&PackageId>>,
    target: &PackageId,
) -> eyre::Result<String> {
    let mut chain = vec![target];
    let mut cursor = target;
    while let Some(Some(parent)) = came_from.get(cursor) {
        chain.push(parent);
        cursor = parent;
    }
    chain.reverse();

    let mut rendered = String::new();
    for (depth, id) in chain.iter().enumerate() {
        let name = graph.metadata(id)?.name().to_owned();
        let indent = "  ".repeat(depth);
        let arrow = if depth == 0 { "  " } else { "  └─ " };
        writeln!(rendered, "{indent}{arrow}{name}")?;
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use googletest::prelude::*;
    use guppy::graph::PackageGraph;
    use serde_json::json;

    use guppy::PackageId;

    use super::{Rule, check};
    use crate::cargo::fixture::{
        Member, dep, graph, graph_with_features, member, optional_dep, policy,
    };
    use crate::cargo::workspace_member;
    use crate::report::Severity;

    /// Builds a rule that keeps `app` from reaching `crates` in the all-features graph.
    fn forbid(crates: &[&str]) -> Rule {
        Rule {
            root: "app".to_owned(),
            crates: crates.iter().map(|name| (*name).to_owned()).collect(),
            features: None,
            default_features: None,
            reason: "test".to_owned(),
        }
    }

    /// Returns the path each violation reports as `root -> ... -> crate`, and fails on drift.
    fn violation_paths(
        graph: &PackageGraph,
        rule: &Rule,
        hakari: Option<&PackageId>,
    ) -> eyre::Result<Vec<String>> {
        let mut paths = Vec::new();
        for finding in check(graph, rule, hakari)? {
            assert_that!(
                &finding.severity,
                eq(&Severity::Violation),
                "unexpected drift: {}",
                finding.headline
            );
            // The detail draws the path one crate per line, root first.
            let path: Vec<&str> = finding
                .detail
                .lines()
                .map(|line| line.trim().trim_start_matches("└─ "))
                .collect();
            paths.push(path.join(" -> "));
        }
        Ok(paths)
    }

    /// A normal or build path reaches a forbidden crate through any number of crates, while a
    /// dev-dependency and the workspace-hack's unifying edges reach nothing, with or without a
    /// feature selection.
    #[test]
    fn forbid_follows_normal_and_build_paths_only() -> eyre::Result<()> {
        let graph = graph(&[
            (
                "app",
                &[
                    dep("client", None),
                    dep("tool", Some("build")),
                    dep("harness", Some("dev")),
                    dep("hack", None),
                ],
            ),
            ("client", &[dep("runtime", None)]),
            ("tool", &[dep("codegen", None)]),
            ("harness", &[dep("mock", None)]),
            ("hack", &[dep("unified", None)]),
            ("runtime", &[]),
            ("codegen", &[]),
            ("mock", &[]),
            ("unified", &[]),
        ])?;
        let all_features = forbid(&["runtime", "codegen", "mock", "unified"]);
        let selected_features = Rule {
            default_features: Some(false),
            ..forbid(&["runtime", "codegen", "mock", "unified"])
        };

        for rule in [all_features, selected_features] {
            assert_that!(
                violation_paths(&graph, &rule, workspace_member(&graph, "hack"))?,
                unordered_elements_are![
                    eq("app -> client -> runtime"),
                    eq("app -> tool -> codegen"),
                ],
                "selects features: {}",
                rule.selects_features()
            );
        }
        Ok(())
    }

    /// A root glob that matches the workspace-hack's name does not make it a root, so its unifying
    /// edges report nothing.
    #[test]
    fn forbid_never_roots_at_the_workspace_hack() -> eyre::Result<()> {
        let graph = graph(&[
            ("app-gui", &[dep("app-hack", None)]),
            ("app-hack", &[dep("unified", None)]),
            ("unified", &[]),
        ])?;
        let rule = Rule {
            root: "app-*".to_owned(),
            ..forbid(&["unified"])
        };

        assert_that!(
            violation_paths(&graph, &rule, workspace_member(&graph, "app-hack"))?,
            is_empty()
        );
        Ok(())
    }

    /// Builds a fixture whose root `app` reaches `runtime` through a required dependency, `ort`
    /// through an optional one its `ocr` feature enables, and `onnx` through an optional one its
    /// default `vision` feature enables.
    fn optional_dependency_graph() -> eyre::Result<PackageGraph> {
        graph_with_features(&[
            Member {
                name: "app",
                deps: &[
                    dep("client", None),
                    optional_dep("ocr-engine"),
                    optional_dep("vision-engine"),
                ],
                features: json!({
                    "default": ["vision"],
                    "vision": ["dep:vision-engine"],
                    "ocr": ["dep:ocr-engine"],
                }),
            },
            member("client", &[dep("runtime", None)]),
            member("ocr-engine", &[dep("ort", None)]),
            member("vision-engine", &[dep("onnx", None)]),
            member("runtime", &[]),
            member("ort", &[]),
            member("onnx", &[]),
        ])
    }

    /// A rule that selects features reports an optional dependency only when a selected feature
    /// enables it, directly or through `default`, and reports a required dependency always.
    #[test]
    fn forbid_with_features_judges_only_activated_optional_dependencies() -> eyre::Result<()> {
        let graph = optional_dependency_graph()?;
        let all_features = forbid(&["runtime", "ort", "onnx"]);
        let ocr = || Some(vec!["ocr".to_owned()]);

        // A rule without feature keys judges every optional dependency.
        assert_that!(
            violation_paths(&graph, &all_features, None)?,
            unordered_elements_are![
                eq("app -> client -> runtime"),
                eq("app -> ocr-engine -> ort"),
                eq("app -> vision-engine -> onnx"),
            ]
        );

        // Default features alone enable `vision` but not `ocr`.
        let defaults_only = Rule {
            features: Some(Vec::new()),
            ..forbid(&["runtime", "ort", "onnx"])
        };
        assert_that!(
            violation_paths(&graph, &defaults_only, None)?,
            unordered_elements_are![
                eq("app -> client -> runtime"),
                eq("app -> vision-engine -> onnx"),
            ]
        );

        // Selecting `ocr` adds its dependency to the default ones.
        let with_ocr = Rule {
            features: ocr(),
            ..forbid(&["runtime", "ort", "onnx"])
        };
        assert_that!(
            violation_paths(&graph, &with_ocr, None)?,
            unordered_elements_are![
                eq("app -> client -> runtime"),
                eq("app -> ocr-engine -> ort"),
                eq("app -> vision-engine -> onnx"),
            ]
        );

        // Turning off default features drops what only `default` enables.
        let no_defaults = Rule {
            default_features: Some(false),
            ..forbid(&["runtime", "ort", "onnx"])
        };
        assert_that!(
            violation_paths(&graph, &no_defaults, None)?,
            elements_are![eq("app -> client -> runtime")]
        );
        let only_ocr = Rule {
            features: ocr(),
            default_features: Some(false),
            ..forbid(&["runtime", "ort", "onnx"])
        };
        assert_that!(
            violation_paths(&graph, &only_ocr, None)?,
            unordered_elements_are![
                eq("app -> client -> runtime"),
                eq("app -> ocr-engine -> ort"),
            ]
        );
        Ok(())
    }

    /// A crate that the build reaches only through a required path is reported along that path,
    /// even when an inactive optional edge would reach it in fewer steps.
    #[test]
    fn forbid_with_features_reports_a_path_through_activated_edges() -> eyre::Result<()> {
        let graph = graph_with_features(&[
            Member {
                name: "app",
                deps: &[dep("client", None), optional_dep("ort")],
                features: json!({"ocr": ["dep:ort"]}),
            },
            member("client", &[dep("ort", None)]),
            member("ort", &[]),
        ])?;
        let selected_features = Rule {
            default_features: Some(false),
            ..forbid(&["ort"])
        };

        // The all-features walk takes the optional shortcut.
        assert_that!(
            violation_paths(&graph, &forbid(&["ort"]), None)?,
            elements_are![eq("app -> ort")]
        );
        // The selected build compiles `ort` only for `client`, so that is the edge to cut.
        assert_that!(
            violation_paths(&graph, &selected_features, None)?,
            elements_are![eq("app -> client -> ort")]
        );
        Ok(())
    }

    /// A feature the root does not declare is a policy error, like an unknown root.
    #[test]
    fn forbid_with_unknown_feature_is_an_error() -> eyre::Result<()> {
        let graph = optional_dependency_graph()?;
        let rule = Rule {
            features: Some(vec!["speech".to_owned()]),
            ..forbid(&["ort"])
        };

        let Err(err) = check(&graph, &rule, None) else {
            eyre::bail!("an unknown feature unexpectedly resolved");
        };
        assert_that!(
            err.to_string(),
            all!(contains_substring("`speech`"), contains_substring("`app`"))
        );
        Ok(())
    }

    /// A forbid rule reads its kebab-case feature keys, and a rule without them selects nothing.
    #[test]
    fn forbid_rules_parse_feature_keys() -> eyre::Result<()> {
        let policy = policy(
            r#"
            [[tool.cargo.forbid]]
            root = "query-service"
            crates = ["ort"]
            default-features = false
            features = ["local-inference"]
            reason = "the local build links no ONNX Runtime"

            [[tool.cargo.forbid]]
            root = "desktop-app"
            crates = ["ort"]
            reason = "inference runs in a separate process"
            "#,
        )?;
        let [selected, unselected] = policy.forbid.as_slice() else {
            eyre::bail!("expected two forbid rules, found {}", policy.forbid.len());
        };

        assert_that!(selected.default_features, eq(Some(false)));
        assert_that!(
            selected.features,
            some(elements_are![eq("local-inference")])
        );
        assert_that!(unselected.selects_features(), eq(false));
        Ok(())
    }
}
