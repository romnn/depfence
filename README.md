# depfence

depfence checks dependency-graph rules against the resolved package graph.
It runs in CI next to linters and fails when a package can reach a dependency it must not reach,
when an unlisted package depends on a guarded one, or when a build enables a denied feature.

Rules are grouped by package manager.
Cargo workspaces are supported today.

## Install

```bash
brew install --cask romnn/tap/depfence

# Or install from source
cargo install --locked --git https://github.com/romnn/depfence depfence
```

With [mise](https://mise.jdx.dev):

```toml
[tools]
"github:romnn/depfence" = "latest"
```

Prebuilt archives for Linux, macOS and Windows are attached to every
[release](https://github.com/romnn/depfence/releases).

## Usage

```bash
depfence check                                  # reads depfence.toml in the current directory
depfence check --policy ../service/depfence.toml # checks ../service/Cargo.toml
depfence check --strict                         # stale policy entries fail too
```

The Cargo workspace checked is the `Cargo.toml` beside the policy file, or the one `--manifest-path`
names.
Cargo runs in that workspace's directory, so its `.cargo/config.toml` applies wherever depfence is
started.

The exit code is non-zero when a rule is violated.
Entries that no longer match the graph, such as an allowance no crate uses or a forbidden crate that
is not in the graph, are reported as stale.
They fail the check only with `--strict`.
A glob that matches no crate is not stale, because it can guard against crates that do not exist
yet, and neither are the `only` entries of a `restrict` rule whose glob matches nothing.
Unknown tables and keys are errors, so a misspelled rule cannot silently check nothing.

## Policy file

`depfence.toml` groups its rules under `[tool.<package manager>]`:

```toml
[[tool.cargo.forbid]]
root = "gui"
crates = ["llama-cpp-2", "ort"]
reason = "inference runs in a separate process"

[[tool.cargo.restrict]]
crate = "mime"
only = ["mime-type"]
reason = "workspace crates use the mime-type wrapper"

[[tool.cargo.deny-feature]]
crate = "serde_json"
feature = "arbitrary_precision"
reason = "breaks internally tagged enums with float fields"
```

Every rule carries a `reason`, which is printed with each finding.

## Cargo rules

### `forbid`

No path may exist from `root` to any crate in `crates`.
depfence reports the shortest path, so the output names the edge to cut.
Normal and build dependencies count because both can affect a production build.
Dev dependencies are skipped because they affect only development targets.

`root` and `crates` entries accept a trailing `*`, so one rule can keep a whole family of crates away
from another family:

```toml
[[tool.cargo.forbid]]
root = "test-env-*"
crates = ["billing-*", "e2e-*"]
reason = "the generic test environments carry no product code"
```

By default a `forbid` rule judges the graph with every feature enabled.
A rule that guards one particular build of its root names that build's features instead: `features`
lists the root features to enable, and `default-features = false` leaves out the root's `default`
feature, as in Cargo.
With either key, depfence resolves those features through Guppy's model of Cargo's feature
unification and walks only the edges they activate, so an optional dependency that no selected
feature enables does not count.
The walk still spans every platform, as described under
[feature and target coverage](#feature-and-target-coverage).
Every root the rule matches must declare each listed feature; an unknown feature is an error.

```toml
[[tool.cargo.forbid]]
root = "query-service"
default-features = false
features = ["local"]
crates = ["ort", "ort-sys", "fastembed"]
reason = "the local build runs no model and links no ONNX Runtime"
```

### `restrict`

Only the crates in `only` may depend on `crate` directly.
Dev dependencies count, because a `restrict` rule draws a trust boundary that applies to test
binaries too.

Only workspace members are judged by default: a third-party crate that depends on the guarded crate
is not yours to list.
A rule that must keep every package, third-party crates included, from depending on a crate sets
`dependents = "all"`:

```toml
[[tool.cargo.restrict]]
crate = "tree-sitter-*"
only = []
dependents = "all"
reason = "grammars are untrusted C and run only inside the wasm sandbox"
```

A test harness that every test suite may use, but production code may not, sets
`any-dev-dependent = true`.
A dev-only edge into the guarded crate is then exempt, and `only` governs normal and build
dependencies alone.
An edge declared both as a dev and a normal dependency is not dev-only, so it stays governed.

```toml
[[tool.cargo.restrict]]
crate = "test-env-*"
only = ["test-env-*", "e2e-*"]
any-dev-dependent = true
reason = "test environments serve every test suite, never production code"
```

`crate` accepts a trailing `*`.
When several rules match one crate, the most specific wins: an exact pattern outranks every glob,
and among globs the longest prefix wins.
Two rules with the same pattern are an error.
That ordering lets a narrow exception coexist with a family rule:

```toml
[[tool.cargo.restrict]]
crate = "e2e-*"
only = ["e2e-*"]
reason = "service composition belongs to the end-to-end layer"

[[tool.cargo.restrict]]
crate = "e2e-api-client"
only = ["e2e-*", "load-test"]
reason = "the typed API client also serves the load test"
```

`only` entries accept the same trailing `*`.
An `only` entry counts as used when it matches a direct dependent of any crate the rule governs, so
a shared family dependency justifies the allowance without every leaf crate depending on it.
An entry unused across the whole family is reported as stale, like an unused exact entry.

### `deny-feature`

No workspace build may enable `feature` of `crate` for target code, which covers every member's
library, binaries and tests on any platform.
Build scripts and proc macros may still enable it, because Cargo resolves their features apart from
the target's, so a build dependency that asks for the feature passes.
`build-tools` names workspace members that only build scripts depend on.
Their own target build is exempt, while a crate that depends on one normally is still judged.

```toml
[[tool.cargo.deny-feature]]
crate = "serde_json"
feature = "arbitrary_precision"
build-tools = ["schema-build"]
reason = "breaks internally tagged enums with float fields"
```

The check simulates `cargo test --workspace --all-features` with Guppy's model of Cargo's feature
resolver 2, which keeps the features of build dependencies apart from the target's.
A workspace on resolver 1 unifies them, so there a build dependency that asks for the feature does
enable it for target code, and depfence does not report it.
A violation lists the packages whose dependency requests the feature and prints a `cargo tree`
query that shows the enabling edges.
A feature no package declares, a `build-tools` entry that is not a member, and an exemption whose
target build does not enable the feature are reported as stale.

### Feature and target coverage

depfence resolves the graph with `cargo metadata --all-features`, so a feature-gated edge cannot hide
a violation behind a flag nobody enabled locally.
A `forbid` rule that names `features` or `default-features` follows only the edges those features
enable.

Cargo metadata also keeps target-specific dependency edges, so the rules cover every declared target
rather than only the host running the check.
Every rule judges all platforms at once, so a reported path can join edges that no single build
follows, such as a `cfg(unix)` edge and a `cfg(windows)` edge.
`forbid` also joins host and target edges, while `deny-feature` keeps the features of build scripts
and proc macros apart from those of target code.

### cargo-hakari

A workspace-hack package generated by [cargo-hakari](https://docs.rs/cargo-hakari) depends on the
union of every member's third-party dependencies, so following it would make every crate reach
everything.
When `.config/hakari.toml` names a `hakari-package`, `forbid` neither follows edges into that
workspace member nor takes it as a root, and `restrict` does not count it as a dependent.
`deny-feature` keeps them, because the features it requests reach every member's build.

## Why not cargo-deny

`cargo deny check bans` is edge-local.
`{ crate = "X", wrappers = ["W"] }` means "W may depend on X directly, nobody else may".
It cannot express "R must not reach X by any path", and the gap between the two is not academic:

```text
search -> embeddings -> fastembed -> ort
```

If `embeddings` is an allowed wrapper for `fastembed`, cargo-deny reports `bans ok` for that graph
even though `search` reaches ONNX Runtime.
Closing the path with `wrappers` alone requires guarding the entire transitive closure, which can
reach workspace roots that no wrapper can name.

depfence complements cargo-deny rather than replacing it.
Keep cargo-deny for duplicate versions, licenses, advisories and sources.

## License

MIT
