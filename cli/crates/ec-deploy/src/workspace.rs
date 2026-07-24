//! The loaded workspace, as pure data — the kernel's no-I/O rule made concrete.
//!
//! The kernel never touches the filesystem: an adapter (`ec-adapters`) loads the definition
//! text and every referenced file into a [`Workspace`], and everything here derives from that
//! content. Placement is the single source of lineage (schema rules S-4/S-7): scope chains,
//! per-node level lists, and identity all derive from `nodes[].scope`.

use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

use crate::model::{AuthoredDefinition, DefinitionDoc, Node, Scope};

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("parsing definition: {0}")]
    Definition(String),
    #[error("missing workspace file: {0} (referenced by {1})")]
    MissingFile(String, String),
    #[error("layer {0} is not valid JSON: {1}")]
    BadLayer(String, String),
    #[error("layer {0} must be a JSON object")]
    LayerNotObject(String),
    #[error("unknown scope '{0}'")]
    UnknownScope(String),
    #[error("scope chain too deep at '{0}' (cycle?)")]
    ChainTooDeep(String),
    #[error("unknown environment '{0}'")]
    UnknownEnvironment(String),
    #[error("unresolved token ${{{0}:{1}}}")]
    UnresolvedToken(String, String),
}

/// The definition plus every referenced file's bytes, keyed by the workspace-relative path
/// exactly as the definition spells it.
pub struct Workspace {
    pub definition: DefinitionDoc,
    pub files: BTreeMap<String, String>,
}

/// Parse an authored definition (`topology` + `profiles`) from YAML text.
pub fn parse_authored(text: &str) -> Result<AuthoredDefinition, WorkspaceError> {
    serde_yaml::from_str(text).map_err(|e| WorkspaceError::Definition(e.to_string()))
}

/// Every workspace-relative path an authored definition references, across the topology and **all**
/// profiles — the loader reads this union so any profile's effective definition can be built without
/// re-reading. Scope + component layers come from the shared topology; bindings and provider layers
/// come from each profile.
#[must_use]
pub fn referenced_paths_authored(def: &AuthoredDefinition) -> Vec<String> {
    let mut out = Vec::new();
    for scope in &def.hierarchy.scopes {
        if let Some(layer) = &scope.layer {
            out.push(layer.clone());
        }
    }
    for node in &def.topology.nodes {
        for comp in &node.components {
            if let Some(layer) = &comp.layer {
                out.push(layer.clone());
            }
        }
    }
    for profile in def.profiles.values() {
        for env in &profile.environments {
            out.push(env.bindings.clone());
        }
        for pnode in profile.nodes.values() {
            if let Some(cp) = &pnode.config_provider {
                out.push(cp.layer.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Parse a flat (already-effective) definition document from YAML text — used internally on the
/// output of [`AuthoredDefinition::effective`].
pub fn parse_definition(text: &str) -> Result<DefinitionDoc, WorkspaceError> {
    serde_yaml::from_str(text).map_err(|e| WorkspaceError::Definition(e.to_string()))
}

/// Every workspace-relative path a definition references: scope layers, the provider layers,
/// component leaves, and each environment's bindings file. The loader reads exactly this set.
#[must_use]
pub fn referenced_paths(def: &DefinitionDoc) -> Vec<String> {
    let mut out = Vec::new();
    for scope in &def.hierarchy.scopes {
        if let Some(layer) = &scope.layer {
            out.push(layer.clone());
        }
    }
    for env in &def.environments {
        out.push(env.bindings.clone());
    }
    for node in &def.nodes {
        if let Some(cp) = &node.config_provider {
            out.push(cp.layer.clone());
        }
        for comp in &node.components {
            if let Some(layer) = &comp.layer {
                out.push(layer.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

impl Workspace {
    pub fn scope(&self, id: &str) -> Result<&Scope, WorkspaceError> {
        self.definition
            .hierarchy
            .scopes
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| WorkspaceError::UnknownScope(id.into()))
    }

    /// Root-first chain of scopes ending at `scope_id`.
    pub fn chain(&self, scope_id: &str) -> Result<Vec<&Scope>, WorkspaceError> {
        let mut chain = Vec::new();
        let mut cur = Some(scope_id.to_string());
        while let Some(id) = cur {
            let scope = self.scope(&id)?;
            chain.push(scope);
            cur = scope.parent.clone();
            if chain.len() > 64 {
                return Err(WorkspaceError::ChainTooDeep(scope_id.into()));
            }
        }
        chain.reverse();
        Ok(chain)
    }

    /// The node's derived level list: its chain's levels plus the terminal `device` level.
    pub fn levels_for(&self, node: &Node) -> Result<Vec<String>, WorkspaceError> {
        let mut levels: Vec<String> = self
            .chain(&node.scope)?
            .iter()
            .map(|s| s.level().to_string())
            .collect();
        levels.push("device".into());
        Ok(levels)
    }

    fn file(&self, rel: &str, owner: &str) -> Result<&str, WorkspaceError> {
        self.files
            .get(rel)
            .map(String::as_str)
            .ok_or_else(|| WorkspaceError::MissingFile(rel.into(), owner.into()))
    }

    /// A referenced layer file as a JSON object.
    pub fn layer(
        &self,
        rel: &str,
        owner: &str,
    ) -> Result<serde_json::Map<String, Value>, WorkspaceError> {
        let text = self.file(rel, owner)?;
        let value: Value = serde_json::from_str(text)
            .map_err(|e| WorkspaceError::BadLayer(rel.into(), e.to_string()))?;
        match value {
            Value::Object(map) => Ok(map),
            _ => Err(WorkspaceError::LayerNotObject(rel.into())),
        }
    }

    /// An environment's bindings document.
    pub fn bindings(&self, environment: &str) -> Result<Value, WorkspaceError> {
        let env = self
            .definition
            .environments
            .iter()
            .find(|e| e.name == environment)
            .ok_or_else(|| WorkspaceError::UnknownEnvironment(environment.into()))?;
        let text = self.file(&env.bindings, "environments")?;
        serde_json::from_str(text)
            .map_err(|e| WorkspaceError::BadLayer(env.bindings.clone(), e.to_string()))
    }
}

/// Look up a dotted path inside a JSON value.
#[must_use]
pub fn lookup<'v>(root: &'v Value, dotted: &str) -> Option<&'v Value> {
    let mut cur = root;
    for part in dotted.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

/// Find `${namespace:path}` tokens in a string. Returns `(start, end, path)` triples.
fn find_tokens(s: &str, namespace: &str) -> Vec<(usize, usize, String)> {
    let prefix = format!("${{{namespace}:");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = s[from..].find(&prefix) {
        let start = from + rel;
        let path_start = start + prefix.len();
        match s[path_start..].find('}') {
            Some(rel_end) => {
                let end = path_start + rel_end + 1;
                out.push((start, end, s[path_start..path_start + rel_end].to_string()));
                from = end;
            }
            None => break,
        }
    }
    out
}

/// Collect every `${namespace:path}` token path appearing anywhere in a JSON tree.
pub fn collect_tokens(value: &Value, namespace: &str, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            for (_, _, path) in find_tokens(s, namespace) {
                out.push(path);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_tokens(v, namespace, out)),
        Value::Object(map) => map.values().for_each(|v| collect_tokens(v, namespace, out)),
        _ => {}
    }
}

/// Resolve `${namespace:path}` tokens in a JSON tree against a source document.
///
/// A string that consists of exactly one token is replaced by the bound value with its type
/// preserved (numbers stay numbers — the Dallas packaging Modbus port relies on this). Tokens
/// embedded in longer strings substitute textually.
pub fn resolve_tokens(
    value: &mut Value,
    namespace: &str,
    source: &Value,
) -> Result<(), WorkspaceError> {
    match value {
        Value::String(s) => {
            let tokens = find_tokens(s, namespace);
            if tokens.is_empty() {
                return Ok(());
            }
            let whole = tokens.len() == 1 && tokens[0].0 == 0 && tokens[0].1 == s.len();
            if whole {
                let bound = lookup(source, &tokens[0].2).ok_or_else(|| {
                    WorkspaceError::UnresolvedToken(namespace.into(), tokens[0].2.clone())
                })?;
                *value = bound.clone();
            } else {
                let mut rebuilt = String::new();
                let mut cursor = 0;
                for (start, end, path) in &tokens {
                    let bound = lookup(source, path).ok_or_else(|| {
                        WorkspaceError::UnresolvedToken(namespace.into(), path.clone())
                    })?;
                    rebuilt.push_str(&s[cursor..*start]);
                    match bound {
                        Value::String(b) => rebuilt.push_str(b),
                        other => rebuilt.push_str(&other.to_string()),
                    }
                    cursor = *end;
                }
                rebuilt.push_str(&s[cursor..]);
                *value = Value::String(rebuilt);
            }
        }
        Value::Array(items) => {
            for v in items {
                resolve_tokens(v, namespace, source)?;
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                resolve_tokens(v, namespace, source)?;
            }
        }
        _ => {}
    }
    Ok(())
}
