//! # Configuration - effective layered config
//!
//! Internal coordinator for the Rust core library. Direct providers (`FILE`,
//! `ENV`, `CONFIGMAP`, `GG_CONFIG`, and `SHADOW`) are already single effective
//! documents and pass through unchanged. `CONFIG_COMPONENT` replies and pushes
//! are lineage bundles; their ordered `layers[].config` fragments are validated
//! for lineage ownership and merged into the single effective runtime document.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::cli::ConfigSourceSpec;
use crate::config::identity::short_component_name;
use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};

use super::source::ConfigSource;

const LINEAGE_VERSION: i64 = 1;
const LINEAGE_BUNDLE_INVALID: &str = "LINEAGE_BUNDLE_INVALID";
const LINEAGE_SCOPE_CONFLICT: &str = "LINEAGE_SCOPE_CONFLICT";
const LINEAGE_IDENTITY_CONFLICT: &str = "LINEAGE_IDENTITY_CONFLICT";

/// A config source wrapper that returns the effective runtime config document.
pub struct LayeredConfigSource {
    component: Arc<dyn ConfigSource>,
    spec: ConfigSourceSpec,
    requested_component: String,
}

impl LayeredConfigSource {
    /// Wrap an existing source with effective-config behavior.
    pub fn new(
        component: Arc<dyn ConfigSource>,
        spec: ConfigSourceSpec,
        component_name: &str,
    ) -> Self {
        Self {
            component,
            spec,
            requested_component: sanitize(short_component_name(component_name)),
        }
    }

    fn apply_payload(&self, raw: Value) -> Result<Value> {
        effective_from_source_payload(&self.spec, raw, &self.requested_component)
    }
}

#[async_trait]
impl ConfigSource for LayeredConfigSource {
    async fn load(&self) -> Result<Value> {
        self.apply_payload(self.component.load().await?)
    }

    fn source_name(&self) -> &str {
        self.component.source_name()
    }

    fn watch(&self) -> Option<UnboundedReceiver<Value>> {
        let mut component_rx = self.component.watch()?;
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let source = Self {
            component: self.component.clone(),
            spec: self.spec.clone(),
            requested_component: self.requested_component.clone(),
        };

        tokio::spawn(async move {
            while let Some(raw) = component_rx.recv().await {
                match source.apply_payload(raw) {
                    Ok(effective) => {
                        let _ = out_tx.send(effective);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "configuration reload failed; keeping previous effective snapshot"
                        );
                    }
                }
            }
        });

        Some(out_rx)
    }
}

/// Merge layers using the hierarchical config deep-merge rules.
///
/// Objects merge recursively. Arrays, scalars, and null replace the previous
/// value. Later layers win.
#[must_use]
pub fn deep_merge(layers: &[Value]) -> Value {
    let mut result = Value::Object(Map::new());
    for layer in layers {
        result = merge_value(result, layer.clone(), "$");
    }
    result
}

fn merge_value(left: Value, right: Value, path: &str) -> Value {
    match (left, right) {
        (Value::Object(mut l), Value::Object(r)) => {
            for (key, value) in r {
                let child_path = if path == "$" {
                    format!("$.{key}")
                } else {
                    format!("{path}.{key}")
                };
                match l.remove(&key) {
                    Some(existing) => {
                        l.insert(key, merge_value(existing, value, &child_path));
                    }
                    None => {
                        l.insert(key, value);
                    }
                }
            }
            Value::Object(l)
        }
        (left, right) => {
            if type_conflict_should_warn(&left, &right) {
                tracing::warn!(path, "hierarchical config type conflict; later layer wins");
            }
            right
        }
    }
}

fn type_conflict_should_warn(left: &Value, right: &Value) -> bool {
    !left.is_null()
        && !right.is_null()
        && !left.is_array()
        && !right.is_array()
        && value_kind(left) != value_kind(right)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn effective_from_source_payload(
    spec: &ConfigSourceSpec,
    payload: Value,
    requested_component: &str,
) -> Result<Value> {
    if matches!(spec, ConfigSourceSpec::ConfigComponent) {
        let bundle = parse_lineage_bundle(payload, requested_component)?;
        return Ok(merge_lineage_bundle(bundle));
    }
    Ok(payload)
}

fn parse_lineage_bundle(payload: Value, requested_component: &str) -> Result<LineageBundle> {
    if let Some((code, message)) = structured_error(&payload) {
        return Err(config_error(&code, message));
    }

    let obj = payload.as_object().ok_or_else(|| {
        config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT payload must be a JSON object",
        )
    })?;

    let version = obj
        .get("lineageVersion")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            config_error(
                LINEAGE_BUNDLE_INVALID,
                "CONFIG_COMPONENT lineageVersion must be 1",
            )
        })?;
    if version != LINEAGE_VERSION {
        return Err(config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT lineageVersion must be 1",
        ));
    }

    let catalog_version = require_string(obj.get("catalogVersion"), "catalogVersion")?;
    let component = require_string(obj.get("component"), "component")?;
    if component != requested_component {
        return Err(config_error(
            LINEAGE_BUNDLE_INVALID,
            format!(
                "CONFIG_COMPONENT component '{component}' does not match requested component '{requested_component}'"
            ),
        ));
    }

    let layers_value = obj.get("layers").ok_or_else(|| {
        config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT lineage bundle must include layers",
        )
    })?;
    let layers = layers_value.as_array().ok_or_else(|| {
        config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT layers must be an array",
        )
    })?;
    if layers.is_empty() {
        return Err(config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT layers must not be empty",
        ));
    }

    let mut parsed = Vec::with_capacity(layers.len());
    for (index, layer) in layers.iter().enumerate() {
        parsed.push(parse_layer(layer, index, layers.len(), component)?);
    }

    validate_scope_ownership(&parsed)?;
    validate_identity_ownership(&parsed)?;

    Ok(LineageBundle {
        catalog_version: catalog_version.to_string(),
        component: component.to_string(),
        layers: parsed,
    })
}

fn parse_layer(
    layer: &Value,
    index: usize,
    layer_count: usize,
    bundle_component: &str,
) -> Result<ResolvedConfigLayer> {
    let obj = layer.as_object().ok_or_else(|| {
        config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT layer must be a JSON object",
        )
    })?;
    let id = require_string(obj.get("id"), "layer id")?;
    let kind = require_string(obj.get("kind"), "layer kind")?;
    if kind != "scope" && kind != "component" {
        return Err(config_error(
            LINEAGE_BUNDLE_INVALID,
            format!("CONFIG_COMPONENT layer '{id}' kind must be 'scope' or 'component'"),
        ));
    }
    if kind == "component" {
        if index != layer_count - 1 {
            return Err(config_error(
                LINEAGE_BUNDLE_INVALID,
                "CONFIG_COMPONENT component layer must be final",
            ));
        }
        let component = require_string(obj.get("component"), "layer component")?;
        if component != bundle_component {
            return Err(config_error(
                LINEAGE_BUNDLE_INVALID,
                format!(
                    "CONFIG_COMPONENT component layer '{id}' does not match bundle component '{bundle_component}'"
                ),
            ));
        }
    } else if index == layer_count - 1 {
        return Err(config_error(
            LINEAGE_BUNDLE_INVALID,
            "CONFIG_COMPONENT final layer must be kind 'component'",
        ));
    }
    let config = obj
        .get("config")
        .cloned()
        .ok_or_else(|| config_error(LINEAGE_BUNDLE_INVALID, "layer config is required"))?;
    if !config.is_object() {
        return Err(config_error(
            LINEAGE_BUNDLE_INVALID,
            "layer config must be a JSON object",
        ));
    }
    let scope = match obj.get("scope") {
        Some(value) => Some(parse_string_map(value, "layer scope")?),
        None if kind == "scope" => {
            return Err(config_error(
                LINEAGE_BUNDLE_INVALID,
                format!("CONFIG_COMPONENT scope layer '{id}' must contain object scope"),
            ));
        }
        None => None,
    };

    Ok(ResolvedConfigLayer {
        id: id.to_string(),
        scope,
        config,
    })
}

fn require_string<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            config_error(
                LINEAGE_BUNDLE_INVALID,
                format!("CONFIG_COMPONENT {label} must be a non-empty string"),
            )
        })
}

fn parse_string_map(value: &Value, label: &str) -> Result<BTreeMap<String, String>> {
    let obj = value.as_object().ok_or_else(|| {
        config_error(
            LINEAGE_BUNDLE_INVALID,
            format!("CONFIG_COMPONENT {label} must be a JSON object"),
        )
    })?;
    let mut out = BTreeMap::new();
    for (key, value) in obj {
        let Some(value) = value.as_str() else {
            return Err(config_error(
                LINEAGE_BUNDLE_INVALID,
                format!("CONFIG_COMPONENT {label}.{key} must be a string"),
            ));
        };
        out.insert(key.clone(), value.to_string());
    }
    Ok(out)
}

fn validate_scope_ownership(layers: &[ResolvedConfigLayer]) -> Result<()> {
    let mut owned = BTreeMap::<String, String>::new();
    for layer in layers {
        let Some(scope) = &layer.scope else {
            continue;
        };
        for (key, value) in scope {
            match owned.get(key) {
                Some(previous) if previous != value => {
                    return Err(config_error(
                        LINEAGE_SCOPE_CONFLICT,
                        format!(
                            "scope key '{key}' changed from '{previous}' to '{value}' in layer '{}'",
                            layer.id
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    owned.insert(key.clone(), value.clone());
                }
            }
        }
    }
    Ok(())
}

fn validate_identity_ownership(layers: &[ResolvedConfigLayer]) -> Result<()> {
    let mut owned = BTreeMap::<String, Value>::new();
    for layer in layers {
        let Some(identity) = layer.config.get("identity") else {
            continue;
        };
        let Some(identity) = identity.as_object() else {
            continue;
        };
        for (key, value) in identity {
            match owned.get(key) {
                Some(previous) if previous != value => {
                    return Err(config_error(
                        LINEAGE_IDENTITY_CONFLICT,
                        format!(
                            "identity key '{key}' changed from '{previous}' to '{value}' in layer '{}'",
                            layer.id
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    owned.insert(key.clone(), value.clone());
                }
            }
        }
    }
    Ok(())
}

fn merge_lineage_bundle(bundle: LineageBundle) -> Value {
    let _catalog_version = bundle.catalog_version;
    let _component = bundle.component;
    let configs: Vec<Value> = bundle
        .layers
        .into_iter()
        .map(|layer| layer.config)
        .collect();
    deep_merge(&configs)
}

fn structured_error(payload: &Value) -> Option<(String, String)> {
    let obj = payload.as_object()?;
    if obj.get("ok").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let err = obj.get("error")?.as_object()?;
    let code = err.get("code")?.as_str()?.to_string();
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((code, message))
}

fn config_error(code: &str, message: impl Into<String>) -> EdgeCommonsError {
    EdgeCommonsError::Config(format!("{code}: {}", message.into()))
}

#[derive(Debug)]
struct LineageBundle {
    catalog_version: String,
    component: String,
    layers: Vec<ResolvedConfigLayer>,
}

#[derive(Debug)]
struct ResolvedConfigLayer {
    id: String,
    scope: Option<BTreeMap<String, String>>,
    config: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vectors(name: &str) -> Value {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../hierarchical-config-test-vectors")
            .join(name);
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn body_or_push(case: &Value) -> Value {
        case["input"]
            .get("body")
            .or_else(|| case["input"].get("push"))
            .unwrap()
            .clone()
    }

    fn requested_component(case: &Value) -> &str {
        case["input"]
            .get("requestComponent")
            .and_then(Value::as_str)
            .unwrap_or("opcua-adapter")
    }

    fn error_code(err: EdgeCommonsError) -> String {
        err.to_string()
            .split_once(": ")
            .map(|(_, rest)| rest)
            .unwrap_or("")
            .split_once(':')
            .map(|(code, _)| code)
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn consumes_hierarchical_merge_vectors() {
        let doc = vectors("merge.json");
        for case in doc["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let layers: Vec<Value> = case["input"]["layers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|layer| layer["config"].clone())
                .collect();
            assert_eq!(deep_merge(&layers), case["expected"]["effective"], "{name}");
        }
    }

    #[test]
    fn consumes_lineage_bundle_vectors() {
        let doc = vectors("lineage-bundles.json");
        for case in doc["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let payload = body_or_push(case);
            let result = effective_from_source_payload(
                &ConfigSourceSpec::ConfigComponent,
                payload,
                requested_component(case),
            );

            if let Some(expected_error) = case["expected"].get("error").and_then(Value::as_str) {
                assert_eq!(error_code(result.unwrap_err()), expected_error, "{name}");
            } else {
                assert_eq!(result.unwrap(), case["expected"]["effective"], "{name}");
            }
        }
    }

    #[test]
    fn direct_sources_are_single_effective_documents() {
        let payload = json!({
            "unknownTopLevel": { "retained": true },
            "component": { "token": "direct" }
        });
        let effective = effective_from_source_payload(
            &ConfigSourceSpec::File {
                path: "config.json".into(),
            },
            payload.clone(),
            "direct",
        )
        .unwrap();
        assert_eq!(effective, payload);
    }

    #[test]
    fn invalid_effective_config_vector_fails_only_after_merge() {
        let doc = vectors("errors.json");
        let case = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "invalid-effective-config-keeps-previous-effective")
            .unwrap();
        let effective = effective_from_source_payload(
            &ConfigSourceSpec::ConfigComponent,
            body_or_push(case),
            "opcua-adapter",
        )
        .unwrap();
        let err = crate::config::validation::validate(&effective).unwrap_err();
        assert!(
            err.to_string().contains("CONFIG_VALIDATION_FAILED")
                || err.to_string().contains("validation")
                || err.to_string().contains("required")
        );
    }

    #[test]
    fn valid_push_vector_replaces_previous_effective_candidate() {
        let doc = vectors("errors.json");
        let case = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "valid-push-replaces-previous-effective")
            .unwrap();
        let effective = effective_from_source_payload(
            &ConfigSourceSpec::ConfigComponent,
            body_or_push(case),
            "opcua-adapter",
        )
        .unwrap();
        assert_eq!(effective, case["expected"]["effective"]);
    }
}
