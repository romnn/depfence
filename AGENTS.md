# Repository guidelines

Conventions for `depfence`.
They apply on top of the personal agent guidelines (`~/CLAUDE.md` / `~/.codex/AGENTS.md`); where
this file is silent, those still hold.

## Decisions

Read the architecture decision records in [`adr/`](adr/) before changing what they govern: the rule
semantics, the policy file's shape, or how depfence ships.
A change that alters a recorded decision needs a new ADR that supersedes the old one.

## Layout

The workspace has one crate, `crates/depfence`, a binary.

- `src/policy.rs` parses `depfence.toml`, which groups rules under `[tool.<package manager>]`.
- `src/cargo/` holds the Cargo rules, one module per rule kind, checked against the graph Guppy
  resolves from `cargo metadata`.
- `src/report.rs` prints findings and decides the exit status.
- `tests/cli.rs` runs the binary against a Cargo workspace written to a temporary directory.

A second package manager gets its own module beside `cargo/` and its own `[tool.<name>]` table.

## Task runner

The repo drives everything through [Task](https://taskfile.dev) (`taskfile.yaml`).
Run a `task …` command exactly as written; do not substitute a hand-rolled `cargo …` equivalent.

| Task                    | Purpose                                                    |
| ----------------------- | ---------------------------------------------------------- |
| `task format`           | format the workspace                                       |
| `task check`            | fast type-check on the host                                |
| `task test`             | run the tests with nextest                                 |
| `task test:fc`          | run the tests for every feature combination (the CI gate)  |
| `task lint:fc`          | clippy on every target and feature combination (the gate)  |
| `task typos`            | spellcheck                                                 |
| `task audit`            | security advisories                                        |
| `task unused`           | unused dependencies                                        |
| `task outdated`         | dependency updates, respecting the cooldown window         |
| `task build:goreleaser` | cross-build every release target in the zigbuild container |

The crate has no library target, so there are no doctests.

## Tests

- Rule tests build a package graph from `cargo metadata` JSON (`src/cargo/fixture.rs`), so they need
  no Cargo invocation.
- Assertions use `googletest` matchers.
- The CLI tests in `tests/cli.rs` spawn `cargo metadata` on a path-only workspace; they need no
  network.

## Errors and lints

- Errors are `color_eyre::eyre` reports; the crate is a binary.
- `unwrap`, `expect`, `panic!` and indexing are denied outside tests.
- Fix clippy findings rather than silencing them; an `#[allow]` must carry a `reason`.
