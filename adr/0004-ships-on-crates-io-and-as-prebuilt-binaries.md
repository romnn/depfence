# 4. depfence ships as a binary crate on crates.io and as prebuilt binaries

**Status:** accepted · **Date:** 2026-11-23 · **Supersedes:** [ADR 3](0003-ships-as-a-cli-binary.md)

## Context

ADR 3 kept depfence off crates.io and shipped it only as prebuilt binaries and a Homebrew cask.
The name `depfence` was free on crates.io, and publishing the crate claims it and lets Cargo users
install depfence with `cargo install` or `cargo binstall`.
A library API would still make the rule and finding types a semver surface.

## Decision

- The workspace has one binary crate and no library target.
- Each release publishes the crate to crates.io through the `publish-crate` job, and the `test`
  workflow checks every change with a publish dry run.
- goreleaser cross-builds the binaries with cargo-zigbuild, attaches them to each GitHub release,
  and updates the `romnn/homebrew-tap` cask.

## Consequences

- Consumers install through Homebrew, crates.io, mise's `github:` backend or the release archives.
- The release needs the `CARGO_REGISTRY_TOKEN` and `TAP_GITHUB_TOKEN` repository secrets.
- Without a library target there are no doctests; the tests live in the modules and in
  `crates/depfence/tests/`.
- Revisit when a consumer needs depfence as a library.

## References

- `crates/depfence/Cargo.toml`
- `.goreleaser.yaml`
- `.github/workflows/release.yaml`
- `.github/workflows/test.yaml`
