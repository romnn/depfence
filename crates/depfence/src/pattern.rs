//! Package-name patterns, each an exact name or a prefix ending in `*`.

/// Returns whether `name` matches `pattern`.
pub(crate) fn matches(name: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// Returns whether `name` matches any of `patterns`.
///
/// An empty list matches nothing, which is how a `restrict` rule rejects every dependent.
pub(crate) fn matches_any(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|pattern| matches(name, pattern))
}

#[cfg(test)]
mod tests {
    use googletest::prelude::*;

    use super::{matches, matches_any};

    #[test]
    fn matches_exact_and_prefix_patterns() {
        assert_that!(matches("tree-sitter-rust", "tree-sitter-*"), eq(true));
        assert_that!(matches("tree-sitter", "tree-sitter-*"), eq(false));
        assert_that!(matches("ort", "ort"), eq(true));
        assert_that!(matches("ort-sys", "ort"), eq(false));
    }

    /// A glob entry lets one rule cover a family whose membership changes, so adding a sibling
    /// crate does not require editing the policy.
    #[test]
    fn lists_accept_prefix_globs() {
        let only = ["e2e-*".to_owned(), "outbox-runner".to_owned()];

        assert_that!(matches_any(&only, "e2e-billing-env"), eq(true));
        assert_that!(matches_any(&only, "outbox-runner"), eq(true));
        assert_that!(matches_any(&only, "api-service"), eq(false));
    }

    /// An empty list is how a rule rejects every dependency, so it must not behave like a glob.
    #[test]
    fn empty_list_matches_nothing() {
        assert_that!(matches_any(&[], "tree-sitter-rust"), eq(false));
    }
}
