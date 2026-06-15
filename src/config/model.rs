//! Typed configuration model (mirrors the cross-language JSON schema) plus the
//! runtime [`Config`] snapshot.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::Result;

/// `logging` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoggingConfig {
    pub level: Option<String>,
    pub format: Option<String>,
    pub file_logging: Option<FileLogging>,
    pub loggers: BTreeMap<String, String>,
    pub global_control: bool,
}

/// `logging.fileLogging` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileLogging {
    pub enabled: bool,
    pub file_path: Option<String>,
}

/// `heartbeat` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HeartbeatConfig {
    pub interval_secs: Option<u64>,
    pub measures: Measures,
    pub targets: Vec<HeartbeatTarget>,
}

/// `heartbeat.measures` toggles.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Measures {
    pub cpu: bool,
    pub memory: bool,
    pub disk: bool,
    pub threads: bool,
    pub files: bool,
    pub fds: bool,
}

/// One entry of `heartbeat.targets`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HeartbeatTarget {
    #[serde(rename = "type")]
    pub target_type: String,
    pub config: Option<Value>,
}

/// `metricEmission` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MetricConfig {
    pub target: Option<String>,
    pub namespace: Option<String>,
    pub large_fleet_workaround: bool,
    pub target_config: Option<Value>,
}

/// `component` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ComponentConfig {
    pub global: Value,
    pub instances: Vec<Value>,
}

/// The full typed configuration, deserialized from the raw JSON document.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawConfig {
    pub logging: LoggingConfig,
    pub heartbeat: HeartbeatConfig,
    pub metric_emission: MetricConfig,
    pub tags: BTreeMap<String, Value>,
    pub component: ComponentConfig,
}

/// An immutable configuration snapshot. Construct via [`Config::from_value`] and
/// publish through `ArcSwap`; never mutate in place.
#[derive(Debug, Clone)]
pub struct Config {
    pub component_name: String,
    pub thing_name: String,
    /// Strongly-typed view of the known sections.
    pub parsed: RawConfig,
    /// The original JSON document, retained for template substitution over
    /// arbitrary user keys and for access to instance subtrees.
    pub raw: Value,
}

impl Config {
    /// Deserialize a raw JSON document into a snapshot.
    pub fn from_value(
        component_name: impl Into<String>,
        thing_name: impl Into<String>,
        raw: Value,
    ) -> Result<Self> {
        let parsed: RawConfig = serde_json::from_value(raw.clone())?;
        Ok(Self {
            component_name: component_name.into(),
            thing_name: thing_name.into(),
            parsed,
            raw,
        })
    }

    /// Global component config subtree (`component.global`).
    pub fn global(&self) -> &Value {
        &self.parsed.component.global
    }

    /// Instance ids declared under `component.instances[].id`.
    pub fn instance_ids(&self) -> Vec<String> {
        self.parsed
            .component
            .instances
            .iter()
            .filter_map(|inst| inst.get("id").and_then(Value::as_str).map(str::to_string))
            .collect()
    }

    /// The instance subtree whose `id` matches `id`, if any.
    pub fn instance(&self, id: &str) -> Option<&Value> {
        self.parsed
            .component
            .instances
            .iter()
            .find(|inst| inst.get("id").and_then(Value::as_str) == Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_known_sections_and_instances() {
        let raw = json!({
            "logging": { "level": "DEBUG" },
            "heartbeat": { "intervalSecs": 10, "measures": { "cpu": true } },
            "metricEmission": { "target": "log", "namespace": "demo" },
            "tags": { "site": "factory-1" },
            "component": {
                "global": { "url": "https://x" },
                "instances": [ { "id": "main" }, { "id": "second" } ]
            }
        });
        let cfg = Config::from_value("com.example.C", "thing-1", raw).unwrap();
        assert_eq!(cfg.parsed.logging.level.as_deref(), Some("DEBUG"));
        assert_eq!(cfg.parsed.heartbeat.interval_secs, Some(10));
        assert!(cfg.parsed.heartbeat.measures.cpu);
        assert_eq!(cfg.parsed.metric_emission.namespace.as_deref(), Some("demo"));
        assert_eq!(cfg.instance_ids(), vec!["main", "second"]);
        assert!(cfg.instance("main").is_some());
    }

    #[test]
    fn empty_document_uses_defaults() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        assert_eq!(cfg.parsed.logging.level, None);
        assert!(cfg.instance_ids().is_empty());
    }
}
