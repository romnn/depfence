//! Findings and how they are printed.

use color_eyre::eyre;
use owo_colors::OwoColorize as _;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    /// The resolved graph violates a rule.
    Violation,
    /// A rule or allow-list entry no longer matches the resolved graph.
    ///
    /// Drift remains distinct from a violation because an absent crate may be a deliberate
    /// preemptive ban rather than a broken graph invariant.
    Drift,
}

pub(crate) struct Finding {
    pub(crate) severity: Severity,
    pub(crate) headline: String,
    pub(crate) detail: String,
    pub(crate) reason: String,
}

/// Prints every finding and fails when one is a violation, or drift under `strict`.
///
/// Output goes through `anstream`, which by default drops the colors when stdout is not a terminal
/// or `NO_COLOR` is set.
pub(crate) fn print(findings: &[Finding], rules: usize, strict: bool) -> eyre::Result<()> {
    if findings.is_empty() {
        anstream::println!(
            "{} {rules} dependency policy rules hold",
            "ok".green().bold()
        );
        return Ok(());
    }

    for finding in findings {
        let label = match finding.severity {
            Severity::Violation => "violation".red().bold().to_string(),
            Severity::Drift => "stale policy".yellow().bold().to_string(),
        };
        anstream::println!("{label}: {}", finding.headline.bold());
        anstream::println!("{}", finding.detail);
        anstream::println!("  {} {}\n", "why:".dimmed(), finding.reason.trim());
    }

    let violations = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Violation)
        .count();
    let drift = findings.len() - violations;
    if violations > 0 {
        eyre::bail!("{violations} dependency policy violations, {drift} stale policy entries");
    }
    if strict {
        eyre::bail!("{drift} stale policy entries (--strict)");
    }
    anstream::println!(
        "{} {rules} rules hold, but {drift} policy entries are stale",
        "ok".green().bold()
    );
    Ok(())
}
