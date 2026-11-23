# 3. depfence ships as a CLI binary, not as a library or a crates.io package

**Status:** accepted · **Date:** 2026-11-23

## Context

depfence runs as a command in CI and local checks, configured by its policy file.
A library API would make the rule and finding types a semver surface.

## Decision

- The workspace has one binary crate and no library target.
- The crate is not published to crates.io (`publish = false`).
- goreleaser cross-builds the binaries with cargo-zigbuild, attaches them to each GitHub release,
  and updates the `romnn/homebrew-tap` cask.
- Source installs use `cargo install --git`.

## Consequences

- Consumers pin a release through Homebrew, mise's `github:` backend or the release archives.
- Without a library target there are no doctests; the tests live in the modules and in
  `crates/depfence/tests/`.
- Revisit when a consumer needs depfence as a library or as a crates.io install.

## References

- `.goreleaser.yaml`
- `.github/workflows/release.yaml`
