# 2. Rules are grouped by package manager, and Cargo is the only one implemented

**Status:** accepted · **Date:** 2026-11-23

## Context

`forbid` and `restrict` apply to any package graph with dependency kinds, such as pnpm or uv
workspaces and Go modules.
`deny-feature` and the `features` and `default-features` keys of `forbid` are specific to Cargo's
feature resolution.
The policy file's shape is the tool's public interface, so changing it later would break every
existing policy.

## Decision

- The policy file is `depfence.toml`, and it groups rules under `[tool.<package manager>]`, such as
  `[[tool.cargo.forbid]]`.
- Cargo is the only package manager implemented.
  The name of the tool and of its policy file are not tied to Rust.
- Unknown tables and keys are errors, so a misspelled rule or an unsupported package manager fails
  instead of silently checking nothing.
- Rule keys are kebab-case.
- There is no package-manager-neutral graph model, adapter trait or per-package-manager crate.
  The second package manager adds its own module and table when it is implemented, and only then do
  shared parts move into common code.

## Consequences

- A second package manager is an additive change to the policy file.
- Until then, the Cargo checks use Guppy's types directly, including its model of Cargo's feature
  resolution.

## References

- `crates/depfence/src/policy.rs`
- `crates/depfence/src/cargo/mod.rs`
