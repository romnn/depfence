//! The policy file, which groups its rules by package manager under `[tool.<name>]`.

use std::path::Path;

use color_eyre::eyre::{self, WrapErr as _};
use serde::Deserialize;

/// A parsed policy file.
///
/// Unknown keys are errors at every level, so a misspelled table, rule kind or rule key cannot
/// leave a rule silently asserting nothing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    #[serde(default)]
    pub(crate) tool: Tools,
}

/// The rules of each package manager, keyed by its name.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tools {
    pub(crate) cargo: Option<crate::cargo::Policy>,
}

impl Policy {
    /// Reads and parses the policy file at `path`.
    pub(crate) fn load(path: &Path) -> eyre::Result<Self> {
        let text = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("failed to read policy file {}", path.display()))?;
        Self::parse(&text)
            .wrap_err_with(|| format!("failed to parse policy file {}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> eyre::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub(crate) fn rule_count(&self) -> usize {
        self.tool
            .cargo
            .as_ref()
            .map_or(0, crate::cargo::Policy::rule_count)
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use googletest::prelude::*;

    use super::Policy;

    /// A package manager depfence does not support is an error, not a table that checks nothing.
    #[test]
    fn unsupported_package_managers_are_errors() {
        let policy = Policy::parse(
            r#"
            [[tool.npm.forbid]]
            root = "app"
            crates = ["runtime"]
            reason = "test"
            "#,
        );
        assert_that!(policy.is_err(), eq(true));
    }

    /// A policy without any `[tool]` table holds no rules.
    #[test]
    fn an_empty_policy_holds_no_rules() -> eyre::Result<()> {
        assert_that!(Policy::parse("")?.rule_count(), eq(0));
        Ok(())
    }
}
