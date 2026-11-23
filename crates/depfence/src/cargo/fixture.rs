//! Fixtures for the rule tests.
//!
//! Package graphs are spelled as the JSON `cargo metadata` prints, so the tests need no Cargo
//! invocation.

use color_eyre::eyre::{self, OptionExt as _};
use guppy::graph::PackageGraph;
use serde_json::{Value, json};

const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";
const SERDE_JSON: &str = "registry+https://github.com/rust-lang/crates.io-index#serde_json@1.0.0";

/// One dependency of a fixture workspace member, as its manifest declares it.
pub(super) struct Dep {
    pub(super) name: &'static str,
    pub(super) kind: Option<&'static str>,
    pub(super) optional: bool,
    pub(super) features: &'static [&'static str],
}

/// One fixture workspace member, with the `[features]` table its manifest declares.
pub(super) struct Member<'fixture> {
    pub(super) name: &'fixture str,
    pub(super) deps: &'fixture [Dep],
    pub(super) features: Value,
}

pub(super) fn member<'fixture>(name: &'fixture str, deps: &'fixture [Dep]) -> Member<'fixture> {
    Member {
        name,
        deps,
        features: json!({}),
    }
}

pub(super) fn dep(name: &'static str, kind: Option<&'static str>) -> Dep {
    Dep {
        name,
        kind,
        optional: false,
        features: &[],
    }
}

pub(super) fn optional_dep(name: &'static str) -> Dep {
    Dep {
        optional: true,
        ..dep(name, None)
    }
}

/// Parses `text` as a policy file and returns its `[tool.cargo]` table.
pub(super) fn policy(text: &str) -> eyre::Result<super::Policy> {
    crate::policy::Policy::parse(text)?
        .tool
        .cargo
        .ok_or_eyre("the policy has no [tool.cargo] table")
}

fn member_id(name: &str) -> String {
    format!("path+file:///ws/{name}#0.1.0")
}

fn dep_id(name: &str) -> String {
    if name == "serde_json" {
        SERDE_JSON.to_owned()
    } else {
        member_id(name)
    }
}

/// Spells one `cargo metadata` package entry with a library target, and a build script when
/// `build_script` is set.
fn package(
    id: &str,
    name: &str,
    source: &Value,
    root: &str,
    deps: &Value,
    features: &Value,
    build_script: bool,
) -> Value {
    let mut targets = vec![
        json!({"kind": ["lib"], "crate_types": ["lib"], "name": name,
        "src_path": format!("{root}/src/lib.rs"), "edition": "2021",
        "doc": true, "doctest": true, "test": true}),
    ];
    if build_script {
        targets.push(json!({"kind": ["custom-build"], "crate_types": ["bin"],
            "name": "build-script-build", "src_path": format!("{root}/build.rs"),
            "edition": "2021", "doc": false, "doctest": false, "test": false}));
    }
    json!({
        "name": name, "version": if source.is_null() { "0.1.0" } else { "1.0.0" },
        "id": id, "source": source, "dependencies": deps, "features": features,
        "manifest_path": format!("{root}/Cargo.toml"), "edition": "2021",
        "targets": targets,
        "license": null, "license_file": null, "description": null, "metadata": null,
        "publish": null, "authors": [], "categories": [], "keywords": [], "readme": null,
        "repository": null, "homepage": null, "documentation": null, "links": null,
        "default_run": null, "rust_version": null,
    })
}

/// Builds the graph `cargo metadata` reports for path-only workspace members without features
/// over a registry `serde_json` that declares `arbitrary_precision`.
///
/// A member with a build dependency gets a build script, as Cargo requires.
pub(super) fn graph(members: &[(&str, &[Dep])]) -> eyre::Result<PackageGraph> {
    let members: Vec<Member<'_>> = members
        .iter()
        .map(|&(name, deps)| member(name, deps))
        .collect();
    graph_with_features(&members)
}

/// Builds the graph `cargo metadata --all-features` reports for path-only workspace members over
/// a registry `serde_json` that declares `arbitrary_precision`.
pub(super) fn graph_with_features(members: &[Member<'_>]) -> eyre::Result<PackageGraph> {
    let mut packages = vec![package(
        SERDE_JSON,
        "serde_json",
        &json!(REGISTRY),
        "/registry/serde_json-1.0.0",
        &json!([]),
        &json!({"arbitrary_precision": []}),
        false,
    )];
    let mut nodes = vec![json!({"id": SERDE_JSON, "dependencies": [], "deps": []})];
    for Member {
        name,
        deps,
        features,
    } in members
    {
        let declared: Vec<Value> = deps
            .iter()
            .map(|dep| {
                let registry = dep.name == "serde_json";
                json!({
                    "name": dep.name, "req": "*", "kind": dep.kind, "rename": null,
                    "optional": dep.optional, "uses_default_features": true,
                    "features": dep.features, "target": null, "registry": null,
                    "source": if registry { json!(REGISTRY) } else { Value::Null },
                    "path": if registry { Value::Null } else { json!(format!("/ws/{}", dep.name)) },
                })
            })
            .collect();
        // The resolve section names a dependency by its library name, as Rust code imports it.
        let resolved: Vec<Value> = deps
            .iter()
            .map(|dep| {
                json!({"name": dep.name.replace('-', "_"), "pkg": dep_id(dep.name),
                    "dep_kinds": [{"kind": dep.kind, "target": null}]})
            })
            .collect();
        let ids: Vec<String> = deps.iter().map(|dep| dep_id(dep.name)).collect();
        packages.push(package(
            &member_id(name),
            name,
            &Value::Null,
            &format!("/ws/{name}"),
            &json!(declared),
            features,
            // Cargo builds a package's build dependencies only for its build script.
            deps.iter().any(|dep| dep.kind == Some("build")),
        ));
        nodes.push(json!({"id": member_id(name), "dependencies": ids, "deps": resolved}));
    }
    let members: Vec<String> = members
        .iter()
        .map(|member| member_id(member.name))
        .collect();
    let metadata = json!({
        "packages": packages, "workspace_members": members,
        "workspace_default_members": members,
        "resolve": {"nodes": nodes, "root": null},
        "target_directory": "/ws/target", "version": 1, "workspace_root": "/ws",
        "metadata": null,
    });
    Ok(PackageGraph::from_json(metadata.to_string())?)
}
