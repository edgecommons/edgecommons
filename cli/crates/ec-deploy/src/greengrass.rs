//! The Greengrass renderer: **one deployment document per thing** (DESIGN-cli §8.5.1,
//! REVIEW #3). Thing groups are never used, so a definition's `nodes[]` map 1:1 onto
//! deployments, `targetArn` is a thing ARN, and failure is per node.
//!
//! # Recipes are deliberately not produced here
//!
//! A recipe carries a component's dependencies, `accessControl`, lifecycle, platform, and
//! default configuration — **component packaging facts, not deployment intent** — and every
//! component repo already authors one beside a `gdk-config.json`. Producing it belongs to
//! `component package|release` (RM-013 / GDK). A deployment references an already-published
//! `componentVersion` and carries the per-thing configuration in `configurationUpdate`, which
//! is exactly the split Greengrass itself makes (§8.5.2): the recipe holds defaults, the
//! deployment overrides them. This is a deliberate deviation from the deck's earlier
//! "the renderer emits recipes" framing, recorded in the decision register.

use serde_json::{Map, Value, json};

use crate::model::Component;
use crate::render::{RenderError, RenderOutput, RenderedFile, effective_configs, pretty};
use crate::workspace::{Workspace, collect_tokens, lookup};
use crate::{ConfigSource, Consequence, Plan, PlanEntry};

pub(crate) fn render(
    ws: &Workspace,
    environment: &str,
    config_release: &str,
) -> Result<RenderOutput, RenderError> {
    let bindings = ws.bindings(environment)?;
    let region = required_binding(&bindings, "aws.region")?;
    let account = required_binding(&bindings, "aws.accountId")?;

    // Effective config per (node, component), derived once from placement + leaf.
    let effective = effective_configs(ws, environment)?;

    let mut files = Vec::new();
    let mut plan = Plan::default();

    for node in &ws.definition.nodes {
        let mut components = Map::new();

        for comp in &node.components {
            // CONFIG_COMPONENT delivers config by catalog push, which needs the catalog itself
            // to reach the device — an open delivery question on Greengrass (§8.5.3). Say so
            // rather than emitting a deployment that silently carries no configuration.
            if comp.config_source == ConfigSource::ConfigComponent {
                return Err(RenderError::ConfigSourceNotRenderable {
                    node: node.key.clone(),
                    component: comp.name.clone(),
                    config_source: comp.config_source.as_contract_str(),
                });
            }

            let gg_name =
                greengrass_name(comp).ok_or_else(|| RenderError::MissingGreengrassName {
                    component: comp.name.clone(),
                })?;
            let version = comp
                .artifact
                .as_ref()
                .and_then(|a| a.version.clone())
                .ok_or_else(|| RenderError::MissingComponentVersion {
                    node: node.key.clone(),
                    component: comp.name.clone(),
                })?;

            let mut entry = Map::new();
            entry.insert("componentVersion".into(), json!(version));

            // GG_CONFIG components take their effective config through the deployment's
            // configurationUpdate. The merge payload is a JSON *string* under `ComponentConfig`
            // — the key the runtime's GG_CONFIG source reads.
            if comp.config_source == ConfigSource::GgConfig {
                let cfg = effective
                    .iter()
                    .find(|(n, c, _)| n == &node.key && c == &comp.name)
                    .map(|(_, _, v)| v.clone())
                    .unwrap_or_else(|| Value::Object(Map::new()));
                let merge = serde_json::to_string(&json!({ "ComponentConfig": cfg }))
                    .expect("effective config serializes");
                let mut update = Map::new();
                update.insert("merge".into(), json!(merge));
                entry.insert("configurationUpdate".into(), Value::Object(update));
            }

            components.insert(gg_name, Value::Object(entry));

            plan.entries.push(PlanEntry {
                node: node.key.clone(),
                component: comp.name.clone(),
                consequence: Consequence::Artifact,
                summary: format!(
                    "deploy componentVersion {version} to thing {}",
                    node.thing_name()
                ),
                restarts_component: true,
            });
            plan.entries.push(PlanEntry {
                node: node.key.clone(),
                component: comp.name.clone(),
                consequence: Consequence::Config,
                summary: format!(
                    "deliver effective config via {}",
                    comp.config_source.as_contract_str()
                ),
                restarts_component: !comp.config_source.hot_reloads(),
            });
        }

        let mut doc = Map::new();
        doc.insert(
            "targetArn".into(),
            json!(format!(
                "arn:aws:iot:{region}:{account}:thing/{}",
                node.thing_name()
            )),
        );
        doc.insert(
            "deploymentName".into(),
            json!(format!(
                "{}-{}-{config_release}",
                ws.definition.metadata.name, node.key
            )),
        );
        doc.insert("components".into(), Value::Object(components));
        files.push(RenderedFile {
            path: format!("{}/deployment.json", node.key),
            text: pretty(&Value::Object(doc)),
        });
    }

    // The handshake's published half: what infrastructure must answer for this render.
    let mut tokens: Vec<String> = vec!["aws.accountId".into(), "aws.region".into()];
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

    Ok(RenderOutput { files, plan })
}

/// The component's Greengrass component name.
///
/// Authored on the component (`artifact.greengrassName`) because it is **not derivable**:
/// `opcua-adapter` publishes as `OpcUaAdapter`, not `OpcuaAdapter`. The canonical home is the
/// registry's `greengrassComponentName`; `deployment lock` resolves it from there and commits
/// it once that verb lands, at which point this override becomes optional rather than required.
fn greengrass_name(comp: &Component) -> Option<String> {
    comp.artifact
        .as_ref()
        .and_then(|a| a.greengrass_name.clone())
}

fn required_binding(bindings: &Value, path: &str) -> Result<String, RenderError> {
    match lookup(bindings, path) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Ok(other.to_string()),
        None => Err(RenderError::MissingBinding(path.to_string())),
    }
}

/// Every `${binding:…}` token the workspace's layers reference.
fn collect_binding_tokens(ws: &Workspace, out: &mut Vec<String>) -> Result<(), RenderError> {
    for scope in &ws.definition.hierarchy.scopes {
        if let Some(rel) = &scope.layer {
            collect_tokens(&Value::Object(ws.layer(rel, &scope.id)?), "binding", out);
        }
    }
    for node in &ws.definition.nodes {
        if let Some(cp) = &node.config_provider {
            collect_tokens(
                &Value::Object(ws.layer(&cp.layer, "configProvider")?),
                "binding",
                out,
            );
        }
        for comp in &node.components {
            if let Some(rel) = &comp.layer {
                collect_tokens(&Value::Object(ws.layer(rel, &comp.name)?), "binding", out);
            }
        }
    }
    Ok(())
}
