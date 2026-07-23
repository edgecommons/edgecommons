//! Semantic rules S-1..S-9 for the deployment definition (DESIGN-cli §8.1 stage two of
//! `deployment validate`; the rule catalog lives in the Deployment Studio repo's
//! `schema/DEFINITION.md`). Definition-schema validation (stage one) and effective-config
//! validation (stage three) run in the command layer via `ec-validate`; this module owns
//! the rules JSON Schema cannot express.

use std::collections::HashSet;

use serde_json::Value;

use crate::ConfigSource;
use crate::workspace::{Workspace, collect_tokens, lookup};

#[derive(Debug, Default)]
pub struct Findings {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Findings {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn validate(ws: &Workspace, environment: Option<&str>) -> Findings {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let def = &ws.definition;
    let levels = &def.hierarchy.levels;

    // S-1: level list ends with 'device'.
    if levels.last().map(String::as_str) != Some("device") {
        errors.push("S-1: hierarchy.levels must end with 'device'".into());
    }

    // S-2 / S-3: scope ids and parent chains.
    let scope_ids: HashSet<&str> = def.hierarchy.scopes.iter().map(|s| s.id.as_str()).collect();
    let level_index = |lv: &str| levels.iter().position(|l| l == lv);
    for scope in &def.hierarchy.scopes {
        let level = scope.level();
        match level_index(level) {
            None => errors.push(format!(
                "S-2: scope {}: level '{level}' not declared",
                scope.id
            )),
            Some(i) if i + 1 == levels.len() => {
                errors.push(format!(
                    "S-2: scope {}: 'device' is never a scope level",
                    scope.id
                ));
            }
            Some(i) => match &scope.parent {
                None if i != 0 => errors.push(format!(
                    "S-3: root scope {} must sit at first level '{}'",
                    scope.id, levels[0]
                )),
                Some(parent) => {
                    if !scope_ids.contains(parent.as_str()) {
                        errors.push(format!("S-3: scope {}: unknown parent {parent}", scope.id));
                    } else {
                        let p_level = parent.split('/').next().unwrap_or("");
                        if level_index(p_level).is_none_or(|pi| pi >= i) {
                            errors.push(format!(
                                "S-3: scope {}: parent {parent} does not step down the level order",
                                scope.id
                            ));
                        }
                    }
                }
                None => {}
            },
        }
        if let Err(e) = ws.chain(&scope.id) {
            errors.push(format!("S-3: {e}"));
        }
    }

    // S-4 + token collection over every referenced layer.
    let mut binding_tokens: Vec<String> = Vec::new();
    let mut check_layer =
        |rel: &str, owner: &str, errors: &mut Vec<String>| match ws.layer(rel, owner) {
            Err(e) => errors.push(format!("layer: {owner}: {e}")),
            Ok(map) => {
                for forbidden in ["hierarchy", "identity"] {
                    if map.contains_key(forbidden) {
                        errors.push(format!(
                            "S-4: {rel}: derived key '{forbidden}' is forbidden in authored layers"
                        ));
                    }
                }
                collect_tokens(&Value::Object(map), "binding", &mut binding_tokens);
            }
        };

    for scope in &def.hierarchy.scopes {
        if let Some(layer) = &scope.layer {
            check_layer(layer, &format!("scope {}", scope.id), &mut errors);
        }
    }

    // Nodes: S-6..S-9, plus per-platform config-source legality (EC2009's kernel-side twin).
    let target = crate::Platform::from_family(&def.target_standard.family);
    let mut keys = HashSet::new();
    for node in &def.nodes {
        if !keys.insert(node.key.as_str()) {
            errors.push(format!("S-8: duplicate node key {}", node.key));
        }
        if !scope_ids.contains(node.scope.as_str()) {
            errors.push(format!(
                "S-8: node {}: unknown scope {}",
                node.key, node.scope
            ));
        }
        if node.thing_name() != node.key {
            warnings.push(format!(
                "node {}: thingName '{}' diverges from the key - runtime-identity consequence must be surfaced",
                node.key,
                node.thing_name()
            ));
        }
        let uses_cc = node
            .components
            .iter()
            .any(|c| c.config_source == ConfigSource::ConfigComponent);
        match &node.config_provider {
            Some(cp) => {
                if cp.config_source == ConfigSource::ConfigComponent {
                    errors.push(format!(
                        "S-9: node {}: configProvider bootstrap must not be CONFIG_COMPONENT",
                        node.key
                    ));
                }
                check_layer(
                    &cp.layer,
                    &format!("{}/configProvider", node.key),
                    &mut errors,
                );
            }
            None if uses_cc => errors.push(format!(
                "node {}: components use CONFIG_COMPONENT but the node has no configProvider",
                node.key
            )),
            None => {}
        }
        for comp in &node.components {
            match &comp.layer {
                Some(layer) => {
                    check_layer(layer, &format!("{}/{}", node.key, comp.name), &mut errors);
                }
                None if comp.config_source == ConfigSource::ConfigComponent => {
                    errors.push(format!(
                        "node {}/{}: CONFIG_COMPONENT requires a layer (catalog leaf)",
                        node.key, comp.name
                    ));
                }
                None => {}
            }
            if let Some(platform) = target {
                if !comp.config_source.is_legal_on(platform) {
                    errors.push(format!(
                        "node {}/{}: config source {:?} is not legal on {:?}",
                        node.key, comp.name, comp.config_source, platform
                    ));
                }
            }
            // What runs: an artifact (version/source, for HOST/Greengrass) or a container image
            // (Kubernetes). A component must name one of them.
            let has_artifact = comp
                .artifact
                .as_ref()
                .map(|a| a.version.is_some() || a.source.is_some())
                .unwrap_or(false)
                || comp.image.is_some();
            if !has_artifact {
                errors.push(format!(
                    "S-6: node {}/{}: needs an artifact (version/source) or an image",
                    node.key, comp.name
                ));
            }
        }
    }

    // S-5: binding tokens resolve per environment.
    binding_tokens.sort();
    binding_tokens.dedup();
    for env in &def.environments {
        if environment.is_some_and(|only| only != env.name) {
            continue;
        }
        match ws.bindings(&env.name) {
            Err(e) => errors.push(format!("S-5: environment {}: {e}", env.name)),
            Ok(bindings) => {
                for token in &binding_tokens {
                    if lookup(&bindings, token).is_none() {
                        errors.push(format!(
                            "S-5: environment {}: unresolved binding '{token}'",
                            env.name
                        ));
                    }
                }
            }
        }
    }

    Findings { errors, warnings }
}
