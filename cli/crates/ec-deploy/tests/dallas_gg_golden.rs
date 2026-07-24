//! The Greengrass golden test, and the per-thing invariants that must never regress
//! (DESIGN-cli §8.5.1–§8.5.3, REVIEW #3).
//!
//! The fixture is the unified Dallas definition (`tests/fixtures/dallas`) merged with its
//! `greengrass` profile; `golden-gg/` is its committed rendered output. Beyond byte equality this
//! asserts the decisions the renderer
//! encodes: one deployment per thing, thing ARNs never group ARNs, pinned component versions,
//! and the effective config carried as a stringified `ComponentConfig` merge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ec_deploy::Platform;
use ec_deploy::render::render;
use ec_deploy::workspace::{Workspace, parse_authored, referenced_paths};
use serde_json::Value;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dallas")
}

/// The Greengrass golden lives beside the HOST one, under `golden-gg/`, since both render from the
/// single unified definition.
fn golden_dir() -> PathBuf {
    fixture_dir().join("golden-gg")
}

fn load() -> Workspace {
    let root = fixture_dir();
    let text = std::fs::read_to_string(root.join("definition.yaml")).unwrap();
    let authored = parse_authored(&text).expect("fixture definition parses");
    let doc = authored
        .effective("greengrass")
        .expect("greengrass profile merges");
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
fn greengrass_renders_byte_for_byte_to_the_committed_golden() {
    let ws = load();
    let output = render(&ws, "prod", Platform::Greengrass, "initial").expect("render succeeds");
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
        "Greengrass golden mismatch:\n{}\n\nIf the renderer changed intentionally, re-render \
         the fixture and move render/greengrass over golden/.",
        mismatches.join("\n")
    );
}

#[test]
fn every_node_gets_its_own_thing_targeted_deployment() {
    let ws = load();
    let output = render(&ws, "prod", Platform::Greengrass, "initial").unwrap();

    let docs: Vec<(&String, Value)> = output
        .files
        .iter()
        .filter(|f| f.path.ends_with("deployment.json"))
        .map(|f| {
            (
                &f.path,
                serde_json::from_str(&f.text).expect("deployment is JSON"),
            )
        })
        .collect();

    // One deployment document per node — thing groups are never used (REVIEW #3).
    assert_eq!(docs.len(), ws.definition.nodes.len());

    for (path, doc) in &docs {
        let arn = doc["targetArn"].as_str().unwrap();
        assert!(
            arn.contains(":thing/"),
            "{path}: targetArn must be a thing ARN, got {arn}"
        );
        assert!(
            !arn.contains("thinggroup"),
            "{path}: thing groups are never used, got {arn}"
        );

        for (name, entry) in doc["components"].as_object().unwrap() {
            assert!(
                name.starts_with("com."),
                "{path}: component key must be the full Greengrass name, got {name}"
            );
            assert!(
                entry["componentVersion"].is_string(),
                "{path}/{name}: a deployment references a published componentVersion"
            );
            // GG_CONFIG components carry their effective config as a stringified merge under
            // ComponentConfig — the key the runtime's GG_CONFIG source reads.
            let merge = entry["configurationUpdate"]["merge"]
                .as_str()
                .unwrap_or_else(|| panic!("{path}/{name}: missing configurationUpdate.merge"));
            let parsed: Value = serde_json::from_str(merge).expect("merge is a JSON string");
            assert!(
                parsed["ComponentConfig"]["identity"]["site"] == "dallas",
                "{path}/{name}: merge must carry the derived effective config"
            );
        }
    }

    // Both gateways deploy the shared components at the same pinned versions.
    let gw1 = &docs
        .iter()
        .find(|(p, _)| p.starts_with("gw-fill-01"))
        .unwrap()
        .1;
    let gw2 = &docs
        .iter()
        .find(|(p, _)| p.starts_with("gw-fill-02"))
        .unwrap()
        .1;
    assert_eq!(
        gw1["components"]["com.mbreissi.edgecommons.TelemetryProcessor"]["componentVersion"],
        gw2["components"]["com.mbreissi.edgecommons.TelemetryProcessor"]["componentVersion"]
    );
    // ...but only gw-fill-01 runs the adapter, so the deployments are genuinely per-device.
    assert!(
        gw1["components"]
            .as_object()
            .unwrap()
            .contains_key("com.mbreissi.edgecommons.OpcUaAdapter")
    );
    assert!(
        !gw2["components"]
            .as_object()
            .unwrap()
            .contains_key("com.mbreissi.edgecommons.OpcUaAdapter")
    );
}

#[test]
fn the_plan_records_per_node_artifact_and_config_consequences() {
    let ws = load();
    let output = render(&ws, "prod", Platform::Greengrass, "initial").unwrap();
    // 12 component assignments across the complete plant's 4 things (1 console + 5 + 4 + 2), each
    // contributing an artifact and a config entry.
    assert_eq!(output.plan.entries.len(), 24);
    // GG_CONFIG does not hot-reload, so every config change restarts its component (§8.5.4).
    assert!(output.plan.entries.iter().all(|e| e.restarts_component));
    assert_eq!(output.plan.restarts().len(), 24);
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
