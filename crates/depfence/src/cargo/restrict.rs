//! `restrict` rules, which limit the crates that may depend directly on a guarded crate.

use std::collections::{BTreeSet, HashMap, HashSet};

use color_eyre::eyre;
use guppy::PackageId;
use guppy::graph::{PackageGraph, PackageMetadata};
use owo_colors::OwoColorize as _;
use serde::Deserialize;

use crate::pattern;
use crate::report::{Finding, Severity};

/// Allows only the crates in `only` to depend directly on `crate`.
///
/// Both `crate` and the `only` entries accept an exact crate name or a prefix ending in `*`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct Rule {
    #[serde(rename = "crate")]
    crate_pattern: String,
    #[serde(default)]
    only: Vec<String>,
    /// Lets any crate use the guarded crate as a dev-dependency.
    ///
    /// `only` then governs normal and build dependencies alone, which is how a test harness stays
    /// open to every test suite while staying out of production code.
    #[serde(default)]
    any_dev_dependent: bool,
    #[serde(default)]
    dependents: Dependents,
    reason: String,
}

impl Rule {
    /// Returns whether the rule governs an edge into the guarded crate.
    ///
    /// An edge that is a dev-dependency and also a normal or build dependency is not dev-only, so
    /// it stays governed.
    fn governs(&self, link_is_dev_only: bool) -> bool {
        !(self.any_dev_dependent && link_is_dev_only)
    }
}

/// The dependents a `restrict` rule judges.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Dependents {
    /// Only workspace members; third-party crates may use the guarded crate freely.
    #[default]
    Workspace,
    /// Every package in the graph, for a crate that must not enter the graph through anyone.
    All,
}

impl Dependents {
    fn judges(self, candidate: &PackageMetadata<'_>) -> bool {
        match self {
            Self::Workspace => candidate.in_workspace(),
            Self::All => true,
        }
    }
}

/// Reports every crate that depends on a guarded crate without being on its `only` list.
///
/// Only workspace members count unless the rule opts into judging every dependent: the policy
/// governs the crates we own, and a third-party crate's own dependencies are not ours to list.
///
/// Unlike a `forbid` rule, this check includes dev-dependency edges because a `restrict` rule is a
/// trust boundary that applies to test binaries too, unless the rule opts out with
/// [`Rule::any_dev_dependent`].
///
/// Iterating packages ensures that each guarded crate is evaluated only against its most specific
/// rule, so a catch-all does not also reject a crate that a narrower rule permits.
///
/// `only` entries accept the same trailing `*` glob as `crate`, which lets a rule name a family
/// whose membership changes without the policy having to track every member.
pub(super) fn check(
    graph: &PackageGraph,
    rules: &[Rule],
    hakari: Option<&PackageId>,
) -> eyre::Result<Vec<Finding>> {
    // Two rules with the same pattern tie for every crate they match, so one of them would
    // silently govern nothing.
    let mut patterns = HashSet::new();
    for rule in rules {
        if !patterns.insert(rule.crate_pattern.as_str()) {
            eyre::bail!(
                "policy restricts `{}` in more than one rule",
                rule.crate_pattern
            );
        }
    }

    let mut findings = Vec::new();
    // The direct dependents of every crate a rule governs, which decide whether each of its `only`
    // entries is still used.
    let mut dependents_by_rule = HashMap::<usize, BTreeSet<&str>>::new();

    for guarded in graph.packages() {
        let Some((rule_index, rule)) = governing_rule(rules, guarded.name()) else {
            continue;
        };

        let dependents: BTreeSet<&str> = guarded
            .reverse_direct_links()
            .filter(|link| {
                rule.dependents.judges(&link.from())
                    && rule.governs(link.dev_only())
                    && Some(link.from().id()) != hakari
            })
            .map(|link| link.from().name())
            .collect();
        dependents_by_rule
            .entry(rule_index)
            .or_default()
            .extend(dependents.iter().copied());

        let offenders: Vec<&str> = dependents
            .iter()
            .filter(|dependent| !pattern::matches_any(&rule.only, dependent))
            .copied()
            .collect();
        if !offenders.is_empty() {
            findings.push(Finding {
                severity: Severity::Violation,
                headline: format!(
                    "`{}` is depended on by crates not on its list",
                    guarded.name().yellow()
                ),
                detail: offenders
                    .iter()
                    .map(|name| format!("  {} {name}", "-".dimmed()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                reason: rule.reason.clone(),
            });
        }
    }

    for (rule_index, rule) in rules.iter().enumerate() {
        let Some(dependents) = dependents_by_rule.get(&rule_index) else {
            continue;
        };

        // An `only` entry unused across the complete governed family grants permission nobody
        // needs.
        // Report it as drift, like cargo-deny's `unused-wrapper` check.
        let stale: Vec<&str> = rule
            .only
            .iter()
            .filter(|pattern| {
                !dependents
                    .iter()
                    .any(|dependent| pattern::matches(dependent, pattern))
            })
            .map(String::as_str)
            .collect();
        if !stale.is_empty() {
            findings.push(Finding {
                severity: Severity::Drift,
                headline: format!(
                    "`{}` allows crates that do not depend on it",
                    rule.crate_pattern.yellow()
                ),
                detail: stale
                    .iter()
                    .map(|name| format!("  {} {name}", "-".dimmed()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                reason: rule.reason.clone(),
            });
        }
    }

    // An exact pattern that matches no package asserts nothing.
    // A nonmatching glob may be a healthy preemptive catch-all, so only exact patterns count as
    // drift.
    findings.extend(
        rules
            .iter()
            .enumerate()
            .filter(|(rule_index, rule)| {
                !rule.crate_pattern.ends_with('*') && !dependents_by_rule.contains_key(rule_index)
            })
            .map(|(_rule_index, rule)| rule)
            .map(|rule| Finding {
                severity: Severity::Drift,
                headline: format!(
                    "no package matches restricted crate `{}`",
                    rule.crate_pattern.yellow()
                ),
                detail: "  the rule guards nothing; drop it or fix the crate name\n".to_owned(),
                reason: rule.reason.clone(),
            }),
    );

    Ok(findings)
}

/// Returns the most specific rule for `name`.
///
/// Exact matches outrank prefix globs; among globs, the longest prefix wins.
fn governing_rule<'rules>(rules: &'rules [Rule], name: &str) -> Option<(usize, &'rules Rule)> {
    rules
        .iter()
        .enumerate()
        .filter(|(_index, rule)| pattern::matches(name, &rule.crate_pattern))
        .max_by_key(|(_index, rule)| {
            if rule.crate_pattern.ends_with('*') {
                // Among globs the longest prefix is the most specific.
                rule.crate_pattern.len()
            } else {
                // An exact pattern outranks every glob.
                usize::MAX
            }
        })
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use googletest::prelude::*;

    use guppy::graph::PackageGraph;

    use super::{Dependents, Rule, check, governing_rule};
    use crate::cargo::fixture::{dep, graph, policy};
    use crate::report::Severity;

    fn rule(crate_pattern: &str, only: &[&str]) -> Rule {
        Rule {
            crate_pattern: crate_pattern.to_owned(),
            only: only.iter().map(|name| (*name).to_owned()).collect(),
            any_dev_dependent: false,
            dependents: Dependents::Workspace,
            reason: "test".to_owned(),
        }
    }

    /// An open harness exempts dev-only edges and nothing else.
    #[test]
    fn any_dev_dependent_exempts_only_dev_only_edges() {
        let open = Rule {
            any_dev_dependent: true,
            ..rule("test-env-*", &["e2e-*"])
        };
        let closed = rule("testcontainers", &["test-env-*"]);

        // Dev-only edge
        assert_that!(open.governs(true), eq(false));
        assert_that!(closed.governs(true), eq(true));

        // Normal or build edge, including one also declared as a dev-dependency
        assert_that!(open.governs(false), eq(true));
        assert_that!(closed.governs(false), eq(true));
    }

    /// Exact-rule usage is credited only to the override, not to its catch-all glob.
    #[test]
    fn exact_rule_outranks_catch_all_glob() -> eyre::Result<()> {
        let rules = vec![
            rule("tree-sitter-*", &[]),
            rule("tree-sitter-yaml", &["config-loader"]),
        ];

        let Some((rule_index, governing)) = governing_rule(&rules, "tree-sitter-yaml") else {
            eyre::bail!("no rule governs `tree-sitter-yaml`");
        };
        assert_that!(rule_index, eq(1));
        assert_that!(governing.crate_pattern.as_str(), eq("tree-sitter-yaml"));
        assert_that!(governing.only.len(), eq(1));
        Ok(())
    }

    #[test]
    fn catch_all_governs_unlisted_crates() -> eyre::Result<()> {
        let rules = vec![
            rule("tree-sitter-*", &[]),
            rule("tree-sitter-yaml", &["config-loader"]),
        ];

        let Some((rule_index, governing)) = governing_rule(&rules, "tree-sitter-python") else {
            eyre::bail!("no rule governs `tree-sitter-python`");
        };
        assert_that!(rule_index, eq(0));
        assert_that!(governing.crate_pattern.as_str(), eq("tree-sitter-*"));
        Ok(())
    }

    #[test]
    fn unguarded_crate_has_no_governing_rule() {
        let rules = vec![rule("tree-sitter-*", &[])];
        assert_that!(governing_rule(&rules, "tantivy").is_none(), eq(true));
    }

    /// Returns whether each finding is a violation, with its detail naming the crates involved.
    fn findings(graph: &PackageGraph, rules: &[Rule]) -> eyre::Result<Vec<(bool, String)>> {
        Ok(check(graph, rules, None)?
            .into_iter()
            .map(|finding| (finding.severity == Severity::Violation, finding.detail))
            .collect())
    }

    /// The check exempts a dev-only dependent of an open harness and still judges production code.
    #[test]
    fn any_dev_dependent_exempts_dev_only_dependents() -> eyre::Result<()> {
        let graph = graph(&[
            ("app", &[dep("test-env-db", None)]),
            ("api", &[dep("test-env-db", Some("dev"))]),
            ("e2e-suite", &[dep("test-env-db", None)]),
            ("test-env-db", &[]),
        ])?;
        let open = Rule {
            any_dev_dependent: true,
            ..rule("test-env-*", &["e2e-*"])
        };

        // Only the production dependent is reported: the dev-only edge from `api` is exempt, and
        // `e2e-suite` is on the list.
        assert_that!(
            findings(&graph, &[open])?,
            elements_are![(
                eq(&true),
                all!(contains_substring(" app"), not(contains_substring("api")))
            )]
        );
        Ok(())
    }

    /// An allowance counts as used when it matches a dependent of any crate the rule governs, and
    /// a dependent of a crate that a narrower rule governs is credited to that rule alone.
    #[test]
    fn allowance_use_is_credited_to_the_governing_rule() -> eyre::Result<()> {
        let rules = [
            rule("e2e-*", &["e2e-*"]),
            rule("e2e-client", &["e2e-*", "load-test"]),
        ];

        // `e2e-suite` uses the family allowance through `e2e-env`, and both allowances of the
        // exact rule through `e2e-client`.
        let used = graph(&[
            (
                "e2e-suite",
                &[dep("e2e-env", None), dep("e2e-client", None)],
            ),
            ("load-test", &[dep("e2e-client", None)]),
            ("e2e-env", &[]),
            ("e2e-client", &[]),
        ])?;
        assert_that!(findings(&used, &rules)?, is_empty());

        // Without `e2e-env`, only `e2e-client` has `e2e-*` dependents, and the exact rule governs
        // it, so the family allowance is stale.
        let unused = graph(&[
            ("e2e-suite", &[dep("e2e-client", None)]),
            ("load-test", &[dep("e2e-client", None)]),
            ("e2e-client", &[]),
        ])?;
        assert_that!(
            findings(&unused, &rules)?,
            elements_are![(eq(&false), contains_substring("e2e-*"))]
        );
        Ok(())
    }

    /// Two rules for one pattern tie, so one of them would silently govern nothing.
    #[test]
    fn duplicate_patterns_are_an_error() -> eyre::Result<()> {
        let graph = graph(&[("app", &[dep("mime", None)]), ("mime", &[])])?;
        let rules = [rule("mime", &[]), rule("mime", &["app"])];

        let Err(err) = check(&graph, &rules, None) else {
            eyre::bail!("duplicate patterns were accepted");
        };
        assert_that!(err.to_string(), contains_substring("`mime`"));
        Ok(())
    }

    /// A rule judges workspace members unless it opts into every dependent, and reads its
    /// kebab-case keys.
    #[test]
    fn restrict_rules_parse_their_keys() -> eyre::Result<()> {
        let policy = policy(
            r#"
            [[tool.cargo.restrict]]
            crate = "mime"
            only = ["mime-type"]
            reason = "wrapper"

            [[tool.cargo.restrict]]
            crate = "tree-sitter-*"
            dependents = "all"
            any-dev-dependent = true
            reason = "sandbox"
            "#,
        )?;
        let [wrapper, sandbox] = policy.restrict.as_slice() else {
            eyre::bail!(
                "expected two restrict rules, found {}",
                policy.restrict.len()
            );
        };

        // Defaults
        assert_that!(wrapper.dependents, eq(Dependents::Workspace));
        assert_that!(wrapper.any_dev_dependent, eq(false));

        // Explicit keys
        assert_that!(sandbox.dependents, eq(Dependents::All));
        assert_that!(sandbox.any_dev_dependent, eq(true));
        Ok(())
    }
}
