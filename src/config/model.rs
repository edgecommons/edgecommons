//! # Configuration — model
//!
//! **One-liner purpose**: Typed `serde` structs for the config sections plus the
//! runtime [`Config`] snapshot.
//!
//! ## Overview
//! Mirrors the cross-language JSON schema (`logging`, `heartbeat`, `metricEmission`,
//! `tags`, `component`). [`Config`] pairs the typed [`RawConfig`] view with the
//! original JSON document and the resolved component/thing identity.
//!
//! ## Semantics & Architecture
//! - All structs derive `Default` and use `#[serde(default)]`, so absent fields
//!   fall back to defaults rather than failing.
//! - `Config` is immutable and cloneable; no interior mutability.
//! - Error handling: [`Config::from_value`] returns [`crate::error::Result`] on
//!   deserialization failure.
//!
//! ## Usage Example
//! ```
//! use ggcommons::config::model::Config;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("c", "t", json!({ "component": { "instances": [ { "id": "main" } ] } })).unwrap();
//! assert_eq!(cfg.instance_ids(), vec!["main"]);
//! ```
//!
//! ## Design Choices
//! Loose subtrees (`component.global`, instances) stay as `serde_json::Value` so
//! component-specific config needs no library changes.
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`super`], [`super::template`], [`super::validation`].

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

impl MetricConfig {
    /// Selected target (`log` | `messaging` | `cloudwatch` | `cloudwatchcomponent`); default `log`.
    pub fn target(&self) -> &str {
        self.target.as_deref().unwrap_or("log")
    }

    /// CloudWatch namespace; default `ggcommons`.
    pub fn namespace(&self) -> &str {
        self.namespace.as_deref().unwrap_or("ggcommons")
    }

    /// Read a string field from `targetConfig`.
    fn target_config_str(&self, key: &str) -> Option<String> {
        self.target_config
            .as_ref()?
            .get(key)?
            .as_str()
            .map(str::to_string)
    }

    /// `targetConfig.logFileName` template (log target); default Greengrass path.
    pub fn log_file_name(&self) -> String {
        self.target_config_str("logFileName")
            .unwrap_or_else(|| "/greengrass/v2/logs/{ComponentFullName}.metric.log".to_string())
    }

    /// `targetConfig.maxFileSize` (log target); default `10MB`.
    pub fn max_file_size(&self) -> String {
        self.target_config_str("maxFileSize").unwrap_or_else(|| "10MB".to_string())
    }

    /// `targetConfig.topic` template; per-target default if unset.
    pub fn topic(&self) -> String {
        if let Some(topic) = self.target_config_str("topic") {
            return topic;
        }
        match self.target() {
            "cloudwatchcomponent" => "cloudwatch/metric/put".to_string(),
            _ => "{ThingName}/{ComponentName}/metric".to_string(),
        }
    }

    /// `targetConfig.destination` (messaging target): `ipc`/`local` or `iotcore`; default `ipc`.
    pub fn destination(&self) -> String {
        self.target_config_str("destination").unwrap_or_else(|| "ipc".to_string())
    }

    /// `targetConfig.intervalSecs` (cloudwatch batch flush); default 5, minimum 1.
    pub fn interval_secs(&self) -> u64 {
        self.target_config
            .as_ref()
            .and_then(|tc| tc.get("intervalSecs"))
            .and_then(Value::as_u64)
            .filter(|&n| n >= 1)
            .unwrap_or(5)
    }
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
