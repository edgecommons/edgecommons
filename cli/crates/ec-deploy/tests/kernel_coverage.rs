//! Exercises the deployment kernel across its branches: model helpers, workspace derivation
//! and token resolution, every semantic rule (S-1..S-9) and its warnings, the render error
//! paths, and the release builder (determinism, manifest integrity, the two streams). The
//! Dallas golden test proves the happy render byte-for-byte; this file drives the edges.

use std::collections::BTreeMap;

use ec_deploy::model::DefinitionDoc;
use ec_deploy::release::build_release;
use ec_deploy::render::{effective_configs, render};
use ec_deploy::validate::validate;
use ec_deploy::workspace::{
    Workspace, collect_tokens, lookup, parse_definition, referenced_paths, resolve_tokens,
};
use ec_deploy::{ConfigSource, Platform, Stream};
use serde_json::json;

// --- helpers -------------------------------------------------------------------------------

fn ws(yaml: &str, files: &[(&str, &str)]) -> Workspace {
    let definition: DefinitionDoc = parse_definition(yaml).expect("definition parses");
    let mut map = BTreeMap::new();
    for (p, c) in files {
        map.insert((*p).to_string(), (*c).to_string());
    }
    Workspace {
        definition,
        files: map,
    }
}

/// A minimal but complete HOST workspace: enterprise→site scopes, one node with a config
/// provider and one CONFIG_COMPONENT component. Enough to render, validate, and release.
fn complete() -> Workspace {
    let yaml = r#"apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata: { name: mini }
hierarchy:
  levels: [site, device]
  scopes:
    - { id: site/lab, parent: null, layer: layers/site.json }
targetStandard: { family: HOST }
environments:
  - { name: local, bindings: bindings/local.json }
nodes:
  - key: box-01
    scope: site/lab
    localBroker: { kind: emqx, port: 1883, launch: { order: 10 } }
    configProvider:
      configSource: FILE
      layer: layers/provider.json
      catalogPath: /config/config-catalog.json
      launch: { order: 15, waitFor: ["localhost:1883"] }
    components:
      - name: telemetry-processor
        artifact: { source: { kind: sibling, repo: telemetry-processor } }
        configSource: CONFIG_COMPONENT
        layer: layers/leaf.json
        launch: { order: 30, waitFor: ["localhost:1883"], settleSeconds: 3 }
"#;
    ws(
        yaml,
        &[
            (
                "layers/site.json",
                r#"{ "heartbeat": { "intervalSecs": 10 } }"#,
            ),
            (
                "layers/provider.json",
                r#"{ "component": { "token": "cc", "global": { "configComponent": { "catalogSource": { "type": "file", "path": "${provider:catalog.path}" } } } } }"#,
            ),
            (
                "layers/leaf.json",
                r#"{ "component": { "token": "telemetry-processor", "instances": [] } }"#,
            ),
            ("bindings/local.json", "{}"),
        ],
    )
}

// --- model ---------------------------------------------------------------------------------

#[test]
fn model_helpers_and_config_source_contract() {
    assert_eq!(Platform::from_family("HOST"), Some(Platform::Host));
    assert_eq!(
        Platform::from_family("GREENGRASS"),
        Some(Platform::Greengrass)
    );
    assert_eq!(
        Platform::from_family("KUBERNETES"),
        Some(Platform::Kubernetes)
    );
    assert_eq!(Platform::from_family("nope"), None);

    for (src, s, hot) in [
        (ConfigSource::File, "FILE", true),
        (ConfigSource::Env, "ENV", false),
        (ConfigSource::GgConfig, "GG_CONFIG", false),
        (ConfigSource::Shadow, "SHADOW", true),
        (ConfigSource::ConfigComponent, "CONFIG_COMPONENT", true),
        (ConfigSource::ConfigMap, "CONFIGMAP", true),
    ] {
        assert_eq!(src.as_contract_str(), s);
        assert_eq!(src.hot_reloads(), hot);
    }
    assert!(ConfigSource::ConfigMap.is_legal_on(Platform::Kubernetes));
    assert!(!ConfigSource::ConfigMap.is_legal_on(Platform::Host));
    assert!(ConfigSource::GgConfig.is_legal_on(Platform::Greengrass));
    assert!(!ConfigSource::GgConfig.is_legal_on(Platform::Host));
    assert!(ConfigSource::File.is_legal_on(Platform::Greengrass));

    let w = complete();
    let node = &w.definition.nodes[0];
    assert_eq!(node.thing_name(), "box-01");
    let comp = &node.components[0];
    assert_eq!(comp.catalog_key(), "TelemetryProcessor");
    assert_eq!(comp.messaging_file(), "telemetry-processor-messaging.json");

    // Overrides.
    let w2 = ws(
        r#"apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata: { name: o }
hierarchy: { levels: [site, device], scopes: [ { id: site/lab, parent: null } ] }
targetStandard: { family: HOST }
environments: [ { name: local, bindings: b.json } ]
nodes:
  - key: k
    scope: site/lab
    identity: { thingName: thing-x }
    components:
      - name: opcua-adapter
        catalogKey: MyOpc
        artifact: { version: "1.2.3" }
        configSource: FILE
        messaging: { file: opc-msg.json }
"#,
        &[("b.json", "{}")],
    );
    let n = &w2.definition.nodes[0];
    assert_eq!(n.thing_name(), "thing-x");
    assert_eq!(n.components[0].catalog_key(), "MyOpc");
    assert_eq!(n.components[0].messaging_file(), "opc-msg.json");
    assert_eq!(referenced_paths(&w2.definition), vec!["b.json".to_string()]);
}

// --- workspace derivation + tokens ---------------------------------------------------------

#[test]
fn workspace_chain_levels_and_layer_errors() {
    let w = complete();
    let chain = w.chain("site/lab").unwrap();
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].level(), "site");
    assert_eq!(chain[0].value(), "lab");
    assert_eq!(
        w.levels_for(&w.definition.nodes[0]).unwrap(),
        vec!["site", "device"]
    );
    assert!(w.scope("nope").is_err());
    assert!(w.chain("nope").is_err());
    // Missing / malformed layers.
    assert!(w.layer("does/not/exist.json", "x").is_err());
    let bad = ws(
        "apiVersion: edgecommons.io/v1alpha1\nkind: DeploymentDefinition\nmetadata: { name: b }\nhierarchy: { levels: [site, device], scopes: [ { id: site/lab, parent: null } ] }\ntargetStandard: { family: HOST }\nenvironments: [ { name: local, bindings: b.json } ]\nnodes: [ { key: k, scope: site/lab, components: [ { name: c, artifact: { version: \"1\" }, configSource: FILE, layer: bad.json } ] } ]",
        &[
            ("b.json", "{}"),
            ("bad.json", "not json"),
            ("arr.json", "[]"),
        ],
    );
    assert!(bad.layer("bad.json", "x").is_err());
    assert!(bad.layer("arr.json", "x").is_err()); // not an object
    assert!(bad.bindings("nope").is_err());
    assert!(bad.bindings("local").is_ok());
}

#[test]
fn token_resolution_preserves_type_and_reports_unresolved() {
    let source = json!({ "a": { "host": "h", "port": 5021 } });

    // Whole-token -> value type preserved (number stays a number).
    let mut v = json!({ "p": "${binding:a.port}", "h": "${binding:a.host}" });
    resolve_tokens(&mut v, "binding", &source).unwrap();
    assert_eq!(v["p"], json!(5021));
    assert_eq!(v["h"], json!("h"));

    // Embedded token -> textual substitution.
    let mut e = json!({ "u": "tcp://${binding:a.host}:${binding:a.port}" });
    resolve_tokens(&mut e, "binding", &source).unwrap();
    assert_eq!(e["u"], json!("tcp://h:5021"));

    // Unresolved -> error, in both whole and embedded position.
    let mut miss = json!("${binding:a.missing}");
    assert!(resolve_tokens(&mut miss, "binding", &source).is_err());
    let mut miss2 = json!("x-${binding:a.missing}");
    assert!(resolve_tokens(&mut miss2, "binding", &source).is_err());

    let mut tokens = Vec::new();
    collect_tokens(
        &json!({ "k": ["${binding:a.host}", 1], "n": "${binding:a.port}" }),
        "binding",
        &mut tokens,
    );
    tokens.sort();
    assert_eq!(tokens, vec!["a.host", "a.port"]);
    assert_eq!(lookup(&source, "a.port"), Some(&json!(5021)));
    assert_eq!(lookup(&source, "a.nope"), None);
}

// --- validate: rules S-1..S-9 + warnings ---------------------------------------------------

fn defn(hierarchy: &str, nodes: &str, family: &str) -> String {
    format!(
        "apiVersion: edgecommons.io/v1alpha1\nkind: DeploymentDefinition\nmetadata: {{ name: t }}\nhierarchy: {hierarchy}\ntargetStandard: {{ family: {family} }}\nenvironments: [ {{ name: local, bindings: bindings/local.json }} ]\nnodes: {nodes}"
    )
}

#[test]
fn validate_accepts_a_complete_workspace() {
    let f = validate(&complete(), Some("local"));
    assert!(f.ok(), "expected valid: {:?}", f.errors);
}

#[test]
fn validate_flags_every_rule() {
    let bindings = &[("bindings/local.json", "{}")];

    // S-1: levels must end with device.
    let w = ws(
        &defn(
            "{ levels: [site, edge], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        bindings,
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.starts_with("S-1"))
    );

    // S-2: device used as a scope level.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: device/x, parent: null } ] }",
            "[ { key: k, scope: device/x, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        bindings,
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.starts_with("S-2"))
    );

    // S-3: unknown parent.
    let w = ws(
        &defn(
            "{ levels: [site, line, device], scopes: [ { id: line/l, parent: site/ghost } ] }",
            "[ { key: k, scope: line/l, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        bindings,
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.starts_with("S-3"))
    );

    // S-4: a derived key inside an authored layer.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null, layer: l.json } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        &[
            ("bindings/local.json", "{}"),
            ("l.json", r#"{ "identity": { "site": "a" } }"#),
        ],
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.starts_with("S-4"))
    );

    // S-5: an unresolved binding token.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null, layer: l.json } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        &[
            ("bindings/local.json", "{}"),
            ("l.json", r#"{ "x": "${binding:missing.key}" }"#),
        ],
    );
    assert!(
        validate(&w, Some("local"))
            .errors
            .iter()
            .any(|e| e.starts_with("S-5"))
    );

    // S-6: a component with neither version nor source.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, configSource: ENV } ] } ]",
            "HOST",
        ),
        bindings,
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.starts_with("S-6"))
    );

    // S-8: duplicate node key + unknown scope.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] }, { key: k, scope: site/ghost, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        bindings,
    );
    let errs = validate(&w, None).errors;
    assert!(errs.iter().any(|e| e.contains("duplicate node key")));
    assert!(errs.iter().any(|e| e.contains("unknown scope")));

    // S-9: a config provider bootstrapping from CONFIG_COMPONENT, and CC without a provider.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, configProvider: { configSource: CONFIG_COMPONENT, layer: p.json, catalogPath: /c }, components: [ { name: c, artifact: { version: \"1\" }, configSource: CONFIG_COMPONENT, layer: leaf.json } ] } ]",
            "HOST",
        ),
        &[
            ("bindings/local.json", "{}"),
            ("p.json", "{}"),
            ("leaf.json", "{}"),
        ],
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.starts_with("S-9"))
    );

    // CC component but no provider on the node.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: CONFIG_COMPONENT, layer: leaf.json } ] } ]",
            "HOST",
        ),
        &[("bindings/local.json", "{}"), ("leaf.json", "{}")],
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.contains("no configProvider"))
    );

    // Platform legality: CONFIGMAP on HOST is illegal.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: CONFIGMAP } ] } ]",
            "HOST",
        ),
        bindings,
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.contains("not legal on"))
    );

    // Warning: thingName diverges from key.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, identity: { thingName: other }, components: [ { name: c, artifact: { version: \"1\" }, configSource: ENV } ] } ]",
            "HOST",
        ),
        bindings,
    );
    assert!(
        validate(&w, None)
            .warnings
            .iter()
            .any(|w| w.contains("thingName"))
    );

    // CC component missing its leaf layer.
    let w = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, configProvider: { configSource: FILE, layer: p.json, catalogPath: /c }, components: [ { name: c, artifact: { version: \"1\" }, configSource: CONFIG_COMPONENT } ] } ]",
            "HOST",
        ),
        &[("bindings/local.json", "{}"), ("p.json", "{}")],
    );
    assert!(
        validate(&w, None)
            .errors
            .iter()
            .any(|e| e.contains("requires a layer"))
    );
}

// --- render error paths --------------------------------------------------------------------

#[test]
fn render_rejects_target_mismatch_and_unbuilt_targets() {
    let w = complete(); // family HOST
    assert!(render(&w, "local", Platform::Greengrass, "initial").is_err()); // mismatch
    // A KUBERNETES definition rendered to KUBERNETES: matches the standard, but the renderer
    // is not built -> a different error than a mismatch.
    let k = ws(
        &defn(
            "{ levels: [site, device], scopes: [ { id: site/a, parent: null } ] }",
            "[ { key: k, scope: site/a, components: [ { name: c, artifact: { version: \"1\" }, configSource: CONFIGMAP } ] } ]",
            "KUBERNETES",
        ),
        &[("bindings/local.json", "{}")],
    );
    assert!(render(&k, "local", Platform::Kubernetes, "initial").is_err());
    // effective_configs computes the merged documents.
    let cfgs = effective_configs(&w, "local").unwrap();
    assert_eq!(cfgs.len(), 1);
    assert_eq!(cfgs[0].0, "box-01");
}

// --- release -------------------------------------------------------------------------------

#[test]
fn release_is_deterministic_correlates_two_streams_and_refuses_invalid() {
    let w = complete();
    let a = build_release(
        &w,
        "local",
        Platform::Host,
        Stream::Config,
        "initial",
        "abc123def456",
        &[],
        0,
    )
    .unwrap();
    let b = build_release(
        &w,
        "local",
        Platform::Host,
        Stream::Config,
        "initial",
        "abc123def456",
        &[],
        0,
    )
    .unwrap();
    assert_eq!(a.tag, "config-abc123def456");
    assert_eq!(a.files.len(), b.files.len());
    for ((pa, ta), (pb, tb)) in a.files.iter().zip(&b.files) {
        assert_eq!(pa, pb);
        assert_eq!(ta, tb, "release output must be deterministic: {pa}");
    }

    let manifest: serde_json::Value = serde_json::from_str(
        &a.files
            .iter()
            .find(|(p, _)| p == "manifest.json")
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(manifest["promotedStream"], "config");
    assert_eq!(manifest["devMode"], true, "source-form artifact => devMode");
    assert!(
        manifest["streams"]["config"]
            .as_object()
            .unwrap()
            .contains_key("box-01")
    );
    assert_eq!(manifest["streams"]["artifact"].as_array().unwrap().len(), 1);
    assert!(a.files.iter().any(|(p, _)| p == "evidence.json"));

    // Every manifest file hash matches the committed snapshot bytes.
    use sha2::{Digest, Sha256};
    for entry in manifest["files"].as_array().unwrap() {
        let path = entry["path"].as_str().unwrap();
        let want = entry["sha256"].as_str().unwrap();
        let (_, text) = a
            .files
            .iter()
            .find(|(p, _)| p == &format!("rendered/{path}"))
            .unwrap();
        let hex: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(format!("sha256:{hex}"), want, "hash mismatch for {path}");
    }

    // The artifact stream promotes the same lock; a pre-counted error refuses the build.
    assert!(
        build_release(
            &w,
            "local",
            Platform::Host,
            Stream::Artifact,
            "initial",
            "c",
            &[],
            0
        )
        .is_ok()
    );
    assert!(
        build_release(
            &w,
            "local",
            Platform::Host,
            Stream::Config,
            "initial",
            "c",
            &[],
            3
        )
        .is_err()
    );
}
