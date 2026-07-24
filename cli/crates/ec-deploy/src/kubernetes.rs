//! The Kubernetes renderer (REVIEW #11, deck ch. 13 slice 4): the semantic compiler emitting
//! **plain deterministic manifests**. Each deployed component becomes a ServiceAccount, a ConfigMap
//! carrying its effective config, a Deployment, and a ClusterIP Service — one multi-document YAML
//! file per component. kubectl-from-CI and Argo/Flux are delivery adapters over this output.
//!
//! The shape follows the proven test chart (`test-infra/k8s/chart`) and the hierarchical-config
//! interop manifests: Downward-API identity, the `CONFIGMAP` config source mounted as a **whole
//! volume** at `/etc/edgecommons` (so the kubelet's atomic `..data` swap drives in-process
//! hot-reload), non-root / read-only-rootfs security, and named-node placement mapping the topology
//! node key to a `nodeSelector` label. Deliberately **no ConfigMap checksum annotation**: rolling
//! the pod on a config edit would defeat the hot-reload the `CONFIGMAP` source exists for.

use serde_json::{Map, Value, json};

use crate::model::{Component, Node};
use crate::render::{RenderError, RenderOutput, RenderedFile, effective_configs, pretty};
use crate::workspace::{Workspace, collect_tokens};
use crate::{Consequence, Plan, PlanEntry};

const CONFIG_MOUNT: &str = "/etc/edgecommons";
const CONFIG_KEY: &str = "config.json";
const HEALTH_PORT: i64 = 8081;
const METRICS_PORT: i64 = 9090;

pub(crate) fn render(
    ws: &Workspace,
    environment: &str,
    config_release: &str,
) -> Result<RenderOutput, RenderError> {
    let effective = effective_configs(ws, environment)?;
    let mut files = Vec::new();
    let mut plan = Plan::default();

    for node in &ws.definition.nodes {
        for comp in &node.components {
            let image = comp
                .image
                .clone()
                .ok_or_else(|| RenderError::MissingImage {
                    node: node.key.clone(),
                    component: comp.name.clone(),
                })?;
            let cfg = effective
                .iter()
                .find(|(n, c, _)| n == &node.key && c == &comp.name)
                .map(|(_, _, v)| v.clone())
                .unwrap_or_else(|| Value::Object(Map::new()));

            let name = format!("{}-{}", node.key, comp.name);
            let manifests = [
                service_account(&name, node, comp),
                config_map(&name, node, comp, &cfg),
                deployment(&name, node, comp, &image),
                service(&name, node, comp),
            ];
            let text = manifests
                .iter()
                .map(yaml_doc)
                .collect::<Vec<_>>()
                .join("---\n");
            files.push(RenderedFile {
                path: format!("{}/{}.yaml", node.key, comp.name),
                text,
            });

            plan.entries.push(PlanEntry {
                node: node.key.clone(),
                component: comp.name.clone(),
                consequence: Consequence::Artifact,
                summary: format!("apply Deployment {name} (image {image})"),
                restarts_component: true,
            });
            plan.entries.push(PlanEntry {
                node: node.key.clone(),
                component: comp.name.clone(),
                consequence: Consequence::Config,
                summary: format!(
                    "deliver effective config via {} (ConfigMap {name})",
                    comp.config_source.as_contract_str()
                ),
                // A whole-volume CONFIGMAP mount hot-reloads in place; anything else rolls the pod.
                restarts_component: !comp.config_source.hot_reloads(),
            });
        }
    }

    let mut tokens = Vec::new();
    collect_binding_tokens(ws, &mut tokens)?;
    tokens.sort();
    tokens.dedup();
    files.push(RenderedFile {
        path: "requirements.json".into(),
        text: pretty(&json!({
            "definition": ws.definition.metadata.name,
            "bindings": tokens,
        })),
    });
    files.push(RenderedFile {
        path: "plan.json".into(),
        text: {
            let mut s = serde_json::to_string_pretty(&plan).expect("plan serializes");
            s.push('\n');
            s
        },
    });
    let _ = config_release; // the release tag is not part of a k8s object name (labels/annotations are stable)

    Ok(RenderOutput { files, plan })
}

/// Common labels every object for a component carries — the standard `app.kubernetes.io/*` recommended
/// set, plus the topology node the component belongs to.
fn labels(node: &Node, comp: &Component) -> Value {
    json!({
        "app.kubernetes.io/name": comp.name,
        "app.kubernetes.io/instance": format!("{}-{}", node.key, comp.name),
        "app.kubernetes.io/part-of": node.key,
        "app.kubernetes.io/managed-by": "edgecommons",
    })
}

fn selector_labels(node: &Node, comp: &Component) -> Value {
    json!({
        "app.kubernetes.io/name": comp.name,
        "app.kubernetes.io/instance": format!("{}-{}", node.key, comp.name),
    })
}

fn service_account(name: &str, node: &Node, comp: &Component) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": { "name": name, "labels": labels(node, comp) },
        // The projected token is what makes the component auto-detect platform=KUBERNETES.
        "automountServiceAccountToken": true,
    })
}

fn config_map(name: &str, node: &Node, comp: &Component, cfg: &Value) -> Value {
    let mut data = Map::new();
    // The CONFIGMAP source reads this key; the value is the effective config, pretty-printed so an
    // operator can `kubectl edit` it to drive the ..data hot-reload.
    data.insert(CONFIG_KEY.into(), Value::String(pretty(cfg)));
    json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": { "name": name, "labels": labels(node, comp) },
        "data": Value::Object(data),
    })
}

fn deployment(name: &str, node: &Node, comp: &Component, image: &str) -> Value {
    let args = json!([
        "--platform",
        "KUBERNETES",
        "-c",
        comp.config_source.as_contract_str(),
        CONFIG_MOUNT,
        CONFIG_KEY,
    ]);
    let env = json!([
        // Downward-API identity (FR-RT-7): the library reads EDGECOMMONS_THING_NAME, then POD_NAME.
        { "name": "POD_NAME", "valueFrom": { "fieldRef": { "fieldPath": "metadata.name" } } },
        { "name": "POD_NAMESPACE", "valueFrom": { "fieldRef": { "fieldPath": "metadata.namespace" } } },
        { "name": "NODE_NAME", "valueFrom": { "fieldRef": { "fieldPath": "spec.nodeName" } } },
        { "name": "EDGECOMMONS_THING_NAME", "value": node.thing_name() },
    ]);
    let container = json!({
        "name": "component",
        "image": image,
        "args": args,
        "env": env,
        "ports": [
            { "name": "health", "containerPort": HEALTH_PORT },
            { "name": "metrics", "containerPort": METRICS_PORT },
        ],
        "volumeMounts": [
            { "name": "config", "mountPath": CONFIG_MOUNT, "readOnly": true },
        ],
        "securityContext": {
            "allowPrivilegeEscalation": false,
            "readOnlyRootFilesystem": true,
            "capabilities": { "drop": ["ALL"] },
        },
    });
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": { "name": name, "labels": labels(node, comp) },
        "spec": {
            "replicas": 1,
            "selector": { "matchLabels": selector_labels(node, comp) },
            "template": {
                "metadata": { "labels": labels(node, comp) },
                "spec": {
                    "serviceAccountName": name,
                    "automountServiceAccountToken": true,
                    // Named-node placement: pin the component to its gateway's node (REVIEW #11).
                    "nodeSelector": { "edgecommons.io/gateway": node.key },
                    "securityContext": {
                        "runAsNonRoot": true,
                        "seccompProfile": { "type": "RuntimeDefault" },
                    },
                    "containers": [container],
                    "volumes": [
                        { "name": "config", "configMap": { "name": name } },
                    ],
                },
            },
        },
    })
}

fn service(name: &str, node: &Node, comp: &Component) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": { "name": name, "labels": labels(node, comp) },
        "spec": {
            "type": "ClusterIP",
            "selector": selector_labels(node, comp),
            "ports": [
                { "name": "health", "port": HEALTH_PORT, "targetPort": "health" },
                { "name": "metrics", "port": METRICS_PORT, "targetPort": "metrics" },
            ],
        },
    })
}

/// Serialize one manifest to YAML with LF endings — the determinism contract (§8.3).
fn yaml_doc(v: &Value) -> String {
    serde_yaml::to_string(v).expect("manifest serializes to YAML")
}

/// Every `${binding:…}` token the workspace's layers reference — the handshake's published half.
fn collect_binding_tokens(ws: &Workspace, out: &mut Vec<String>) -> Result<(), RenderError> {
    for scope in &ws.definition.hierarchy.scopes {
        if let Some(rel) = &scope.layer {
            collect_tokens(&Value::Object(ws.layer(rel, &scope.id)?), "binding", out);
        }
    }
    for node in &ws.definition.nodes {
        for comp in &node.components {
            if let Some(rel) = &comp.layer {
                collect_tokens(&Value::Object(ws.layer(rel, &comp.name)?), "binding", out);
            }
        }
    }
    Ok(())
}
