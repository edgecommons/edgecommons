//! The HOST renderer (DESIGN-cli §8, first target; deck ch. 13 slice 1): compiles a
//! definition + an environment's bindings into per-node native artifacts — the
//! ConfigComponent catalog, the rendered bootstrap config, the messaging files, and the
//! supervisord conf — plus the normalized [`Plan`] and the handshake `requirements.json`.
//!
//! Determinism is a build gate (§8.3): no timestamps, no hostnames, no randomness, LF
//! endings, stable key order (authored order preserved — see the deviation note in the
//! design doc). Golden proof: the Dallas fixture regenerates the adopted
//! `bottling-company-test` site byte for byte.

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::merge::deep_merge;
use crate::model::{Component, Launch, Node};
use crate::workspace::{Workspace, WorkspaceError, collect_tokens, resolve_tokens};
use crate::{Consequence, Plan, PlanEntry, Platform};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("target {0:?} renderer is not built yet")]
    TargetNotBuilt(Platform),
    #[error("definition targets {def} but --target {req:?} was requested (no exception covers it)")]
    TargetMismatch { def: String, req: Platform },
    #[error(
        "node {node}/{component}: config source {config_source} cannot be delivered by a Greengrass deployment; use GG_CONFIG, or deliver the catalog another way"
    )]
    ConfigSourceNotRenderable {
        node: String,
        component: String,
        config_source: &'static str,
    },
    #[error(
        "component {component}: no Greengrass component name; set artifact.greengrassName (the registry's greengrassComponentName is the canonical source, resolved by `deployment lock` once it lands)"
    )]
    MissingGreengrassName { component: String },
    #[error("node {node}/{component}: a Greengrass deployment needs a pinned artifact.version")]
    MissingComponentVersion { node: String, component: String },
    #[error(
        "node {node}/{component}: a Kubernetes Deployment needs a container image; set the component's `image` in the kubernetes profile"
    )]
    MissingImage { node: String, component: String },
    #[error("environment binding '{0}' is required to build a thing ARN")]
    MissingBinding(String),
}

pub struct RenderedFile {
    /// Path relative to the render output root (`<node-key>/<file>` or a workspace file).
    pub path: String,
    pub text: String,
}

pub struct RenderOutput {
    pub files: Vec<RenderedFile>,
    pub plan: Plan,
}

pub(crate) fn pretty(value: &Value) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("JSON serialization cannot fail");
    s.push('\n');
    s
}

/// Render a definition to one target's native artifacts plus the normalized plan.
///
/// The definition's `targetStandard.family` must match the requested target (governed
/// exceptions are a later slice); the per-target renderers live below.
pub fn render(
    ws: &Workspace,
    environment: &str,
    target: Platform,
    config_release: &str,
) -> Result<RenderOutput, RenderError> {
    render_with_lock(ws, environment, target, config_release, None)
}

/// As [`render`], with a lock file supplying what the definition does not carry — today the
/// Greengrass component name (DESIGN-cli §8.7).
pub fn render_with_lock(
    ws: &Workspace,
    environment: &str,
    target: Platform,
    config_release: &str,
    lock: Option<&crate::lock::LockFile>,
) -> Result<RenderOutput, RenderError> {
    match Platform::from_family(&ws.definition.target_standard.family) {
        Some(def_target) if def_target == target => {}
        Some(_) | None => {
            return Err(RenderError::TargetMismatch {
                def: ws.definition.target_standard.family.clone(),
                req: target,
            });
        }
    }
    match target {
        Platform::Host => render_host(ws, environment, config_release),
        Platform::Greengrass => crate::greengrass::render(ws, environment, config_release, lock),
        Platform::Kubernetes => crate::kubernetes::render(ws, environment, config_release),
    }
}

fn render_host(
    ws: &Workspace,
    environment: &str,
    config_release: &str,
) -> Result<RenderOutput, RenderError> {
    let bindings = ws.bindings(environment)?;
    let mut files = Vec::new();
    let mut all_tokens: Vec<String> = Vec::new();
    let mut plan = Plan::default();

    for node in &ws.definition.nodes {
        let chain = ws.chain(&node.scope)?;
        let levels = ws.levels_for(node)?;

        // ---- Load + resolve this node's scope-layer set.
        let mut scope_layers: Vec<(String, Map<String, Value>)> = Vec::new();
        for scope in &chain {
            let mut content = match &scope.layer {
                Some(rel) => Value::Object(ws.layer(rel, &scope.id)?),
                None => Value::Object(Map::new()),
            };
            collect_tokens(&content, "binding", &mut all_tokens);
            resolve_tokens(&mut content, "binding", &bindings)?;
            match content {
                Value::Object(map) => scope_layers.push((scope.id.clone(), map)),
                _ => unreachable!(),
            }
        }

        // ---- Per-node catalog + rendered bootstrap (when the node has a config provider).
        if let Some(provider) = &node.config_provider {
            let catalog_version = catalog_version(ws, node, config_release)?;

            let mut nodes_map = Map::new();
            for (idx, (scope_id, layer)) in scope_layers.iter().enumerate() {
                let scope = chain[idx];
                let mut entry = Map::new();
                if idx > 0 {
                    entry.insert("parent".into(), json!(chain[idx - 1].id));
                }
                let mut scope_map = Map::new();
                for ancestor in &chain[..=idx] {
                    scope_map.insert(ancestor.level().into(), json!(ancestor.value()));
                }
                entry.insert("scope".into(), Value::Object(scope_map));
                let mut config = Map::new();
                if idx == 0 {
                    config.insert("hierarchy".into(), json!({ "levels": levels }));
                }
                config.insert("identity".into(), json!({ scope.level(): scope.value() }));
                deep_merge(&mut config, layer);
                entry.insert("config".into(), Value::Object(config));
                nodes_map.insert(scope_id.clone(), Value::Object(entry));
            }

            let mut components_map = Map::new();
            for comp in &node.components {
                if comp.config_source != crate::ConfigSource::ConfigComponent {
                    continue;
                }
                let rel = comp
                    .layer
                    .as_ref()
                    .expect("validated: CC components carry a layer");
                let mut leaf = Value::Object(ws.layer(rel, &comp.name)?);
                collect_tokens(&leaf, "binding", &mut all_tokens);
                resolve_tokens(&mut leaf, "binding", &bindings)?;
                let mut entry = Map::new();
                entry.insert("parent".into(), json!(node.scope));
                entry.insert("config".into(), leaf);
                components_map.insert(comp.catalog_key(), Value::Object(entry));
            }

            let mut catalog = Map::new();
            catalog.insert("schemaVersion".into(), json!(1));
            catalog.insert("version".into(), json!(catalog_version));
            catalog.insert(
                "provenance".into(),
                json!({ "source": "file", "uri": provider.catalog_path }),
            );
            catalog.insert("hierarchy".into(), json!({ "levels": levels }));
            catalog.insert("nodes".into(), Value::Object(nodes_map));
            catalog.insert("components".into(), Value::Object(components_map));
            files.push(RenderedFile {
                path: format!("{}/config-catalog.json", node.key),
                text: pretty(&Value::Object(catalog)),
            });

            // Rendered bootstrap: derived keys stamped, then chain merge, then the provider
            // overlay (its `${provider:catalog.path}` token resolved from catalogPath).
            let mut bootstrap = Map::new();
            bootstrap.insert("hierarchy".into(), json!({ "levels": levels }));
            let mut identity = Map::new();
            for scope in &chain {
                identity.insert(scope.level().into(), json!(scope.value()));
            }
            bootstrap.insert("identity".into(), Value::Object(identity));
            for (_, layer) in &scope_layers {
                deep_merge(&mut bootstrap, layer);
            }
            let mut provider_layer = Value::Object(ws.layer(&provider.layer, "configProvider")?);
            let provider_source = json!({ "catalog": { "path": provider.catalog_path } });
            resolve_tokens(&mut provider_layer, "provider", &provider_source)?;
            collect_tokens(&provider_layer, "binding", &mut all_tokens);
            resolve_tokens(&mut provider_layer, "binding", &bindings)?;
            if let Value::Object(overlay) = provider_layer {
                deep_merge(&mut bootstrap, &overlay);
            }
            files.push(RenderedFile {
                path: format!("{}/config-component-config.json", node.key),
                text: pretty(&Value::Object(bootstrap)),
            });

            files.push(RenderedFile {
                path: format!("{}/config-component-messaging.json", node.key),
                text: messaging_json(
                    provider.messaging.as_ref().and_then(|m| m.type_.clone()),
                    node.local_broker.as_ref().map(|b| b.port).unwrap_or(1883),
                    provider
                        .messaging
                        .as_ref()
                        .and_then(|m| m.client_id.clone())
                        .unwrap_or_else(|| format!("{}-config-component", node.key)),
                    provider
                        .messaging
                        .as_ref()
                        .and_then(|m| m.request_timeout_seconds),
                ),
            });
        }

        // ---- Component messaging files + plan entries.
        for comp in &node.components {
            files.push(RenderedFile {
                path: format!("{}/{}", node.key, comp.messaging_file()),
                text: messaging_json(
                    comp.messaging.as_ref().and_then(|m| m.type_.clone()),
                    node.local_broker.as_ref().map(|b| b.port).unwrap_or(1883),
                    comp.messaging
                        .as_ref()
                        .and_then(|m| m.client_id.clone())
                        .unwrap_or_else(|| format!("{}-{}", node.key, comp.name)),
                    comp.messaging
                        .as_ref()
                        .and_then(|m| m.request_timeout_seconds),
                ),
            });

            let artifact = comp
                .artifact
                .as_ref()
                .and_then(|a| a.version.clone())
                .unwrap_or_else(|| "source-form".into());
            plan.entries.push(PlanEntry {
                node: node.key.clone(),
                component: comp.name.clone(),
                consequence: Consequence::Artifact,
                summary: format!("stage artifact {artifact} into the HOST bundle"),
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

        files.push(RenderedFile {
            path: format!("{}/supervisord.conf", node.key),
            text: supervisord_conf(node),
        });
    }

    all_tokens.sort();
    all_tokens.dedup();
    files.push(RenderedFile {
        path: "requirements.json".into(),
        text: pretty(&json!({
            "definition": ws.definition.metadata.name,
            "bindings": all_tokens,
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

/// The per-node catalog version: `versionBase` override, else the scope-chain values joined,
/// plus the config-release tag.
pub fn catalog_version(
    ws: &Workspace,
    node: &Node,
    release_tag: &str,
) -> Result<String, WorkspaceError> {
    let base = node
        .config_provider
        .as_ref()
        .and_then(|cp| cp.version_base.clone());
    let base = match base {
        Some(b) => b,
        None => ws
            .chain(&node.scope)?
            .iter()
            .map(|s| s.value())
            .collect::<Vec<_>>()
            .join("-"),
    };
    Ok(format!("{base}-{release_tag}"))
}

/// Every component's effective runtime config (chain merge + leaf, derived keys stamped) —
/// the documents `deployment validate` checks against the strict config schema (§8.1).
pub fn effective_configs(
    ws: &Workspace,
    environment: &str,
) -> Result<Vec<(String, String, Value)>, RenderError> {
    let bindings = ws.bindings(environment)?;
    let mut out = Vec::new();
    for node in &ws.definition.nodes {
        let chain = ws.chain(&node.scope)?;
        let levels = ws.levels_for(node)?;
        let mut base = Map::new();
        base.insert("hierarchy".into(), json!({ "levels": levels }));
        let mut identity = Map::new();
        for scope in &chain {
            identity.insert(scope.level().into(), json!(scope.value()));
        }
        base.insert("identity".into(), Value::Object(identity));
        for scope in &chain {
            if let Some(rel) = &scope.layer {
                let mut layer = Value::Object(ws.layer(rel, &scope.id)?);
                resolve_tokens(&mut layer, "binding", &bindings)?;
                if let Value::Object(map) = layer {
                    deep_merge(&mut base, &map);
                }
            }
        }
        for comp in &node.components {
            let Some(rel) = &comp.layer else { continue };
            let mut leaf = Value::Object(ws.layer(rel, &comp.name)?);
            resolve_tokens(&mut leaf, "binding", &bindings)?;
            let mut effective = base.clone();
            if let Value::Object(map) = leaf {
                deep_merge(&mut effective, &map);
            }
            out.push((
                node.key.clone(),
                comp.name.clone(),
                Value::Object(effective),
            ));
        }
    }
    Ok(out)
}

fn messaging_json(
    type_: Option<String>,
    port: u16,
    client_id: String,
    request_timeout: Option<u32>,
) -> String {
    let mut local = Map::new();
    if let Some(t) = type_ {
        local.insert("type".into(), json!(t));
    }
    local.insert("host".into(), json!("localhost"));
    local.insert("port".into(), json!(port));
    local.insert("clientId".into(), json!(client_id));
    let mut messaging = Map::new();
    messaging.insert("local".into(), Value::Object(local));
    if let Some(t) = request_timeout {
        messaging.insert("requestTimeoutSeconds".into(), json!(t));
    }
    let mut root = Map::new();
    root.insert("messaging".into(), Value::Object(messaging));
    pretty(&Value::Object(root))
}

struct Program {
    name: String,
    command: String,
    user: Option<String>,
    directory: Option<String>,
    environment: Vec<(String, String)>,
    priority: u32,
    start_secs: Option<u32>,
    start_retries: Option<u32>,
    stop_wait_secs: Option<u32>,
}

fn launch_command(
    exec: &str,
    messaging_file: &str,
    config_source: crate::ConfigSource,
    thing: &str,
    launch: Option<&Launch>,
) -> String {
    let source = config_source.as_contract_str();
    let source_arg = match config_source {
        crate::ConfigSource::File => " /config/config-component-config.json".to_string(),
        _ => String::new(),
    };
    let base = format!(
        "{exec} --platform HOST --transport MQTT /config/{messaging_file} -c {source}{source_arg} -t {thing}"
    );
    let settled = match launch.and_then(|l| l.settle_seconds) {
        Some(secs) if secs > 0 => format!("/bin/bash -lc 'sleep {secs}; exec {base}'"),
        _ => base,
    };
    match launch.map(|l| l.wait_for.as_slice()).unwrap_or(&[]) {
        [] => settled,
        gates => format!(
            "/usr/local/bin/wait-for-tcp {} -- {settled}",
            gates.join(" ")
        ),
    }
}

fn program_from_launch(
    name: &str,
    command: String,
    launch: Option<&Launch>,
    default_priority: u32,
) -> Program {
    Program {
        name: name.to_string(),
        command,
        user: launch.and_then(|l| l.user.clone()),
        directory: launch.and_then(|l| l.working_dir.clone()),
        environment: launch
            .and_then(|l| l.env.as_ref())
            .map(|env| env.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
        priority: launch.and_then(|l| l.order).unwrap_or(default_priority),
        start_secs: launch.and_then(|l| l.start_secs),
        start_retries: launch.and_then(|l| l.start_retries),
        stop_wait_secs: launch.and_then(|l| l.stop_wait_secs),
    }
}

fn component_exec(comp: &Component) -> String {
    comp.launch
        .as_ref()
        .and_then(|l| l.exec.clone())
        .unwrap_or_else(|| comp.name.clone())
}

fn supervisord_conf(node: &Node) -> String {
    let thing = node.thing_name();
    let mut programs: Vec<Program> = Vec::new();

    if let Some(broker) = &node.local_broker {
        let command = match broker.kind.as_str() {
            "emqx" => "/usr/bin/emqx foreground".to_string(),
            other => other.to_string(),
        };
        let mut program = program_from_launch(&broker.kind, command, broker.launch.as_ref(), 10);
        if program.environment.is_empty() {
            if let Some(env) = &broker.env {
                program.environment = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            }
        }
        programs.push(program);
    }
    for aux in &node.auxiliaries {
        programs.push(program_from_launch(
            &aux.name,
            aux.command.clone(),
            aux.launch.as_ref(),
            20,
        ));
    }
    if let Some(provider) = &node.config_provider {
        let command = launch_command(
            "config-component",
            "config-component-messaging.json",
            provider.config_source,
            thing,
            provider.launch.as_ref(),
        );
        programs.push(program_from_launch(
            "config-component",
            command,
            provider.launch.as_ref(),
            25,
        ));
    }
    for comp in &node.components {
        let command = launch_command(
            &component_exec(comp),
            &comp.messaging_file(),
            comp.config_source,
            thing,
            comp.launch.as_ref(),
        );
        programs.push(program_from_launch(
            &comp.name,
            command,
            comp.launch.as_ref(),
            30,
        ));
    }

    let mut order: Vec<usize> = (0..programs.len()).collect();
    order.sort_by_key(|&i| (programs[i].priority, i));

    let mut out = String::from(
        "; Generated by ec-deploy (Deployment Studio HOST renderer). Do not edit by hand.\n\n\
         [supervisord]\n\
         nodaemon=true\n\
         user=root\n\
         logfile=/dev/null\n\
         logfile_maxbytes=0\n\
         pidfile=/run/supervisord.pid\n\n\
         [unix_http_server]\n\
         file=/run/supervisor.sock\n\
         chmod=0700\n\n\
         [rpcinterface:supervisor]\n\
         supervisor.rpcinterface_factory = supervisor.rpcinterface:make_main_rpcinterface\n\n\
         [supervisorctl]\n\
         serverurl=unix:///run/supervisor.sock\n",
    );
    for &i in &order {
        let p = &programs[i];
        out.push('\n');
        out.push_str(&format!("[program:{}]\n", p.name));
        out.push_str(&format!("command={}\n", p.command));
        if let Some(user) = &p.user {
            out.push_str(&format!("user={user}\n"));
        }
        if let Some(dir) = &p.directory {
            out.push_str(&format!("directory={dir}\n"));
        }
        if !p.environment.is_empty() {
            let joined = p
                .environment
                .iter()
                .map(|(k, v)| format!("{k}=\"{v}\""))
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!("environment={joined}\n"));
        }
        out.push_str(&format!("priority={}\n", p.priority));
        out.push_str("autorestart=true\n");
        if let Some(secs) = p.start_secs {
            out.push_str(&format!("startsecs={secs}\n"));
        }
        if let Some(retries) = p.start_retries {
            out.push_str(&format!("startretries={retries}\n"));
        }
        if let Some(secs) = p.stop_wait_secs {
            out.push_str(&format!("stopwaitsecs={secs}\n"));
        }
        out.push_str("stdout_logfile=/dev/stdout\n");
        out.push_str("stdout_logfile_maxbytes=0\n");
        out.push_str("stderr_logfile=/dev/stderr\n");
        out.push_str("stderr_logfile_maxbytes=0\n");
    }
    out
}
