//! The Kubernetes golden test, and the invariants the renderer must never regress (REVIEW #11,
//! deck ch. 13 slice 4).
//!
//! The fixture is the unified Dallas definition (`tests/fixtures/dallas`) merged with its
//! `kubernetes` profile; `golden-k8s/` is its committed rendered output. Beyond byte equality this
//! asserts the decisions the renderer encodes: one YAML file per component carrying a
//! ServiceAccount + ConfigMap + Deployment + Service, the CONFIGMAP config source mounted as a whole
//! volume, Downward-API identity, non-root/read-only security, and named-node placement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ec_deploy::Platform;
use ec_deploy::render::render;
use ec_deploy::workspace::{Workspace, parse_authored, referenced_paths};
use serde::Deserialize;
use serde_json::Value;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dallas")
}

fn golden_dir() -> PathBuf {
    fixture_dir().join("golden-k8s")
}

fn load() -> Workspace {
    let root = fixture_dir();
    let text = std::fs::read_to_string(root.join("definition.yaml")).unwrap();
    let authored = parse_authored(&text).expect("fixture definition parses");
    let doc = authored
        .effective("kubernetes")
        .expect("kubernetes profile merges");
    let mut files = BTreeMap::new();
    for rel in referenced_paths(&doc) {
        let content = std::fs::read_to_string(root.join(&rel))
            .unwrap_or_else(|e| panic!("reading referenced {rel}: {e}"));
        files.insert(rel, content);
    }
    Workspace {
        definition: doc,
        files,
    }
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

#[test]
fn kubernetes_renders_byte_for_byte_to_the_committed_golden() {
    let ws = load();
    let output = render(&ws, "cluster", Platform::Kubernetes, "initial").expect("render succeeds");
    let golden_root = golden_dir();

    let mut mismatches = Vec::new();
    let mut produced = Vec::new();
    for f in &output.files {
        produced.push(f.path.clone());
        match std::fs::read_to_string(golden_root.join(&f.path)) {
            Ok(want) => {
                if normalize(&want) != normalize(&f.text) {
                    mismatches.push(format!("{}: rendered bytes differ from golden", f.path));
                }
            }
            Err(_) => mismatches.push(format!("{}: no golden file", f.path)),
        }
    }
    for entry in walk(&golden_root) {
        let rel = entry
            .strip_prefix(&golden_root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !produced.contains(&rel) {
            mismatches.push(format!("{rel}: in golden but no longer produced"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "Kubernetes golden mismatch:\n{}\n\nIf the renderer changed intentionally, re-render and \
         move render/kubernetes over golden-k8s/.",
        mismatches.join("\n")
    );
}

#[test]
fn every_component_gets_the_four_managed_objects() {
    let ws = load();
    let output = render(&ws, "cluster", Platform::Kubernetes, "initial").unwrap();

    // One YAML file per deployed component (plus plan.json + requirements.json).
    let manifests: Vec<&ec_deploy::render::RenderedFile> = output
        .files
        .iter()
        .filter(|f| f.path.ends_with(".yaml"))
        .collect();
    let total_components: usize = ws.definition.nodes.iter().map(|n| n.components.len()).sum();
    assert_eq!(manifests.len(), total_components);

    for m in &manifests {
        let docs: Vec<Value> = serde_yaml::Deserializer::from_str(&m.text)
            .map(|d| Value::deserialize(d).expect("manifest doc is YAML"))
            .collect();
        let kinds: Vec<&str> = docs.iter().filter_map(|d| d["kind"].as_str()).collect();
        assert_eq!(
            kinds,
            ["ServiceAccount", "ConfigMap", "Deployment", "Service"],
            "{}: the four managed objects, in order",
            m.path
        );

        // The Deployment carries the identity + config-source contract.
        let deployment = docs.iter().find(|d| d["kind"] == "Deployment").unwrap();
        let container = &deployment["spec"]["template"]["spec"]["containers"][0];
        let args: Vec<&str> = container["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert!(
            args.windows(2).any(|w| w == ["-c", "CONFIGMAP"]),
            "{}: config source is delivered as CONFIGMAP: {args:?}",
            m.path
        );
        assert!(
            args.contains(&"KUBERNETES"),
            "{}: the component is told its platform",
            m.path
        );
        let env: Vec<&str> = container["env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(
            env.contains(&"EDGECOMMONS_THING_NAME") && env.contains(&"POD_NAME"),
            "{}: Downward-API identity is wired: {env:?}",
            m.path
        );
        // Named-node placement pins the component to its gateway.
        let node_selector = &deployment["spec"]["template"]["spec"]["nodeSelector"];
        assert!(
            node_selector["edgecommons.io/gateway"].is_string(),
            "{}: nodeSelector pins the gateway",
            m.path
        );
        // Non-root, read-only rootfs — the shipped security posture.
        let sc = &container["securityContext"];
        assert_eq!(
            sc["readOnlyRootFilesystem"],
            Value::Bool(true),
            "{}",
            m.path
        );
    }
}

#[test]
fn the_configmap_carries_the_derived_effective_config_and_hot_reloads() {
    let ws = load();
    let output = render(&ws, "cluster", Platform::Kubernetes, "initial").unwrap();

    let opcua = output
        .files
        .iter()
        .find(|f| f.path == "gw-fill-01/opcua-adapter.yaml")
        .unwrap();
    let docs: Vec<Value> = serde_yaml::Deserializer::from_str(&opcua.text)
        .map(|d| Value::deserialize(d).unwrap())
        .collect();
    let cm = docs.iter().find(|d| d["kind"] == "ConfigMap").unwrap();
    let config: Value = serde_json::from_str(cm["data"]["config.json"].as_str().unwrap())
        .expect("config.json is JSON");
    // The effective config is the placement-derived merge, not the raw leaf.
    assert_eq!(config["identity"]["site"], "dallas");
    assert_eq!(config["identity"]["line"], "filling-line");
    assert_eq!(config["component"]["token"], "opcua-adapter");

    // A whole-volume CONFIGMAP mount hot-reloads in place — so the plan does NOT restart the
    // component on a config change (§8.5.4, and the test chart's deliberate no-checksum choice).
    let config_entries: Vec<&ec_deploy::PlanEntry> = output
        .plan
        .entries
        .iter()
        .filter(|e| matches!(e.consequence, ec_deploy::Consequence::Config))
        .collect();
    assert!(!config_entries.is_empty());
    assert!(
        config_entries.iter().all(|e| !e.restarts_component),
        "CONFIGMAP config delivery hot-reloads, so it must not restart the component"
    );
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
