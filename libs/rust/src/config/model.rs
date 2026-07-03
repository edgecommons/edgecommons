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

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::error::Result;

/// The schema default for `messaging.requestTimeoutSeconds` (seconds) — the
/// framework-owned `request()` deadline (UNS-CANONICAL-DESIGN §5 / D-U5).
pub const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 30;

/// Read a JSON value as `u64`, accepting an integer **or** a (truncated) float.
///
/// Greengrass stores configuration numbers as doubles, so an integer like `5`
/// arrives over IPC as `5.0`; `serde_json`'s `as_u64` rejects floats, so accept
/// both representations to stay robust across config sources.
fn value_as_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_f64().map(|f| f as u64))
}

/// `serde` deserializer for an optional `u64` config field that may be encoded as a
/// JSON float (see [`value_as_u64`]). Absent or `null` yields `None`.
fn de_lenient_opt_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(value_as_u64))
}

/// `serde` deserializer for an optional `f64` config field that accepts any JSON
/// number (integer or float). Absent, `null`, or non-numeric yields `None`.
fn de_lenient_opt_f64<'de, D>(deserializer: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(Value::as_f64))
}

/// `logging` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoggingConfig {
    pub level: Option<String>,
    /// Rust log format using {timestamp}/{level}/{target}/{message} tokens (key `rust_format`,
    /// not camelCased — replaces the former language-agnostic `format`).
    #[serde(rename = "rust_format")]
    pub rust_format: Option<String>,
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
    pub max_file_size: Option<String>,
    #[serde(default, deserialize_with = "de_lenient_opt_u64")]
    pub backup_count: Option<u64>,
}

impl FileLogging {
    /// `maxFileSize` for size-based rotation; default `10MB` (parity with the
    /// Python library's `RotatingFileHandler` default).
    pub fn max_file_size(&self) -> String {
        self.max_file_size
            .clone()
            .unwrap_or_else(|| "10MB".to_string())
    }

    /// Number of rotated backups to keep; default `5` (parity with Python).
    pub fn backup_count(&self) -> u64 {
        self.backup_count.unwrap_or(5)
    }
}

/// `heartbeat` section (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20).
///
/// The heartbeat is a library-owned UNS `state` keepalive published each tick to
/// `ecv1/{device}/{component}/main/state` (body `{"status":"RUNNING","uptimeSecs":n}`,
/// best-effort `{"status":"STOPPED"}` on graceful shutdown), with the enabled system
/// measures emitted as the metric `sys` through the normal metric subsystem. The
/// legacy `targets[]` array (the heartbeat topic-override drift knobs) is removed —
/// hard cut; [`destination`](Self::destination) governs only the state keepalive's
/// transport (`local` vs `iotcore`). Defaults: **on / 5 s / local** (D-U14).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HeartbeatConfig {
    /// Whether the heartbeat (state keepalive + `sys` measures metric) runs.
    /// Default `true`.
    pub enabled: bool,
    #[serde(default, deserialize_with = "de_lenient_opt_u64")]
    pub interval_secs: Option<u64>,
    pub measures: Measures,
    /// Publish destination of the state keepalive only (`"local"` — the default —
    /// or `"iotcore"`). The measures route through the metric subsystem's own target.
    pub destination: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        HeartbeatConfig {
            enabled: true,
            interval_secs: None,
            measures: Measures::default(),
            destination: None,
        }
    }
}

impl HeartbeatConfig {
    /// The schema default for `heartbeat.destination` — the local/IPC transport.
    pub const DEFAULT_DESTINATION: &'static str = "local";

    /// The resolved state-keepalive destination (default
    /// [`Self::DEFAULT_DESTINATION`]).
    pub fn destination(&self) -> &str {
        self.destination.as_deref().unwrap_or(Self::DEFAULT_DESTINATION)
    }
}

/// `heartbeat.measures` toggles. Schema defaults: `cpu`/`memory` on, the rest off.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Measures {
    #[serde(default = "default_true")]
    pub cpu: bool,
    #[serde(default = "default_true")]
    pub memory: bool,
    pub disk: bool,
    pub threads: bool,
    pub files: bool,
    pub fds: bool,
}

impl Default for Measures {
    fn default() -> Self {
        Measures { cpu: true, memory: true, disk: false, threads: false, files: false, fds: false }
    }
}

/// `serde` default helper: `true`.
fn default_true() -> bool {
    true
}

/// `health` section (FR-HB-1) — the Kubernetes HTTP health/readiness endpoint.
///
/// Mirrors the canonical schema `health` object. [`enabled`](Self::enabled) is an `Option`: `None`
/// means "unset", so the platform profile decides (on by default on KUBERNETES, off elsewhere —
/// resolved in [`crate::GgCommonsBuilder::build`] via the FR-RT-3 precedence). The path/port
/// accessors apply the schema defaults when a field is absent.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HealthConfig {
    /// Explicit enable toggle. `None` defers to the platform-profile default.
    pub enabled: Option<bool>,
    /// TCP port (schema default 8081); accepts integer-valued floats from Greengrass.
    #[serde(default, deserialize_with = "de_lenient_opt_u64")]
    pub port: Option<u64>,
    /// Liveness route (schema default `/livez`).
    pub liveness_path: Option<String>,
    /// Readiness route (schema default `/readyz`).
    pub readiness_path: Option<String>,
    /// Startup route (schema default `/startupz`); reuses readiness semantics.
    pub startup_path: Option<String>,
}

impl HealthConfig {
    /// The schema's default health port.
    pub const DEFAULT_PORT: u16 = 8081;

    /// Resolved listen port; default [`Self::DEFAULT_PORT`] (8081) when absent or out of range.
    pub fn port(&self) -> u16 {
        self.port
            .and_then(|p| u16::try_from(p).ok())
            .filter(|&p| p != 0)
            .unwrap_or(Self::DEFAULT_PORT)
    }

    /// Resolved liveness path; default `/livez`.
    pub fn liveness_path(&self) -> &str {
        self.liveness_path.as_deref().unwrap_or("/livez")
    }

    /// Resolved readiness path; default `/readyz`.
    pub fn readiness_path(&self) -> &str {
        self.readiness_path.as_deref().unwrap_or("/readyz")
    }

    /// Resolved startup path; default `/startupz`.
    pub fn startup_path(&self) -> &str {
        self.startup_path.as_deref().unwrap_or("/startupz")
    }
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
    /// The explicitly-configured target (`log` | `messaging` | `cloudwatch` |
    /// `cloudwatchcomponent` | `prometheus`), or `log` when unset. NOTE: this is the *literal*
    /// config value; the *effective* target also factors in the platform-profile default (e.g.
    /// `prometheus` on KUBERNETES) — see `crate::metrics::resolve_effective_target`.
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

    /// The explicit `targetConfig.logFileName` exactly as configured, or `None` when absent. Lets the
    /// metric `log` target distinguish an explicit path (which must win) from an unset one (which
    /// falls through to the platform-profile default, then the library default) — the HOST-aware
    /// metric-log-path precedence.
    pub fn explicit_log_file_name(&self) -> Option<String> {
        self.target_config_str("logFileName")
    }

    /// `targetConfig.maxFileSize` (log target); default `10MB`.
    pub fn max_file_size(&self) -> String {
        self.target_config_str("maxFileSize").unwrap_or_else(|| "10MB".to_string())
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
            .and_then(value_as_u64)
            .filter(|&n| n >= 1)
            .unwrap_or(5)
    }

    /// `targetConfig.buffer` object for the cloudwatch target (`None` if absent).
    fn buffer(&self) -> Option<&Value> {
        self.target_config.as_ref()?.get("buffer")
    }

    /// `targetConfig.buffer.type` (cloudwatch target); default `durable` per the design.
    /// Returns the lowercased string (`durable` | `memory`).
    pub fn buffer_type(&self) -> String {
        self.buffer()
            .and_then(|b| b.get("type"))
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "durable".to_string())
    }

    /// `targetConfig.buffer.path` (durable cloudwatch buffer directory; supports templates).
    /// Default mirrors the design doc's per-component path.
    pub fn buffer_path(&self) -> String {
        self.buffer()
            .and_then(|b| b.get("path"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                "/var/lib/ggcommons/metrics/{ComponentName}/cw".to_string()
            })
    }

    /// `targetConfig.buffer.maxDiskBytes`; default ~128 MiB.
    pub fn buffer_max_disk_bytes(&self) -> u64 {
        self.buffer()
            .and_then(|b| b.get("maxDiskBytes"))
            .and_then(value_as_u64)
            .unwrap_or(134_217_728)
    }

    /// `targetConfig.buffer.onFull`; default `dropOldest` (lowercased string).
    pub fn buffer_on_full(&self) -> String {
        self.buffer()
            .and_then(|b| b.get("onFull"))
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "dropoldest".to_string())
    }

    /// `targetConfig.buffer.fsync`; default `perBatch` (lowercased string).
    pub fn buffer_fsync(&self) -> String {
        self.buffer()
            .and_then(|b| b.get("fsync"))
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "perbatch".to_string())
    }

    /// `targetConfig.port` (prometheus target HTTP port); default `9090`. Out-of-range
    /// (`0` or `> 65535`) or non-numeric values fall back to the default.
    pub fn prometheus_port(&self) -> u16 {
        self.target_config
            .as_ref()
            .and_then(|tc| tc.get("port"))
            .and_then(value_as_u64)
            .and_then(|p| u16::try_from(p).ok())
            .filter(|&p| p != 0)
            .unwrap_or(9090)
    }

    /// `targetConfig.path` (prometheus target OpenMetrics exposition path); default `/metrics`.
    pub fn prometheus_path(&self) -> String {
        self.target_config_str("path").unwrap_or_else(|| "/metrics".to_string())
    }
}

/// `component` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ComponentConfig {
    pub global: Value,
    pub instances: Vec<Value>,
}

/// Top-level `topic` section — UNS topic-building options (UNS-CANONICAL-DESIGN
/// §2.2 rule 6 / D-U11).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TopicConfig {
    /// Insert the first hierarchy value (the site) after the `ecv1` root in built
    /// topics (multi-site broker deployments). Default `false` (rootless grammar).
    /// Effective only with a multi-level hierarchy (D-U25).
    pub include_root: bool,
}

/// The component config's `messaging` section knobs the library reads directly
/// (the broker connection shape itself is consumed by
/// [`crate::messaging::config::MessagingConfig`]).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MessagingSectionConfig {
    /// `messaging.requestTimeoutSeconds` (UNS-CANONICAL-DESIGN §5 / D-U5): the
    /// default `request()` deadline in seconds (fractions allowed); `0` disables.
    /// Absent = the schema default 30.
    #[serde(default, deserialize_with = "de_lenient_opt_f64")]
    pub request_timeout_seconds: Option<f64>,
    /// The raw `messaging.lwt` section, retained only for presence checks (the MQTT
    /// provider parses the typed shape from the messaging-config document; the IPC
    /// transport DEBUG-logs and no-ops it — §6).
    pub lwt: Option<Value>,
}

/// The full typed configuration, deserialized from the raw JSON document.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawConfig {
    pub logging: LoggingConfig,
    pub heartbeat: HeartbeatConfig,
    pub health: HealthConfig,
    pub metric_emission: MetricConfig,
    pub tags: BTreeMap<String, Value>,
    pub component: ComponentConfig,
    pub topic: TopicConfig,
    pub messaging: MessagingSectionConfig,
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
    /// The component's resolved UNS identity (instance
    /// [`MessageIdentity::DEFAULT_INSTANCE`]), resolved fail-fast at snapshot
    /// construction from the component's OWN config (top-level `hierarchy` +
    /// `identity` — no shared config, UNS-CANONICAL-DESIGN §1.5).
    identity: crate::messaging::message::MessageIdentity,
}

impl Config {
    /// Deserialize a raw JSON document into a snapshot, resolving the component's
    /// UNS identity fail-fast (UNS-CANONICAL-DESIGN §1.5 — a malformed
    /// `hierarchy`/`identity` block, a missing level value, or an unavailable thing
    /// name is an error naming the precise inconsistency).
    pub fn from_value(
        component_name: impl Into<String>,
        thing_name: impl Into<String>,
        raw: Value,
    ) -> Result<Self> {
        let component_name = component_name.into();
        let thing_name = thing_name.into();
        let parsed: RawConfig = serde_json::from_value(raw.clone())?;
        let identity = super::identity::resolve(&raw, &thing_name, &component_name)?;
        // D-U25: includeRoot needs a level ABOVE the device to prepend — with a
        // single-level hierarchy (the zero-config ["device"] default) hier[0] IS the
        // device, so the setting is a no-op in Uns (prepending would duplicate it).
        if parsed.topic.include_root && identity.hier().len() == 1 {
            tracing::warn!(
                "topic.includeRoot=true has no effect with a single-level hierarchy \
                 (hierarchy.levels resolves to one level - the device): the site position \
                 requires a level above the device, so UNS topics stay rootless (D-U25). \
                 Declare a multi-level hierarchy.levels or remove topic.includeRoot."
            );
        }
        Ok(Self { component_name, thing_name, parsed, raw, identity })
    }

    /// The component's resolved UNS identity (instance
    /// [`MessageIdentity::DEFAULT_INSTANCE`]) — see
    /// [`crate::config::identity`] for the resolution algorithm.
    pub fn identity(&self) -> &crate::messaging::message::MessageIdentity {
        &self.identity
    }

    /// The raw top-level `topic.includeRoot` setting (default `false`). Note that
    /// topic building applies it only with a multi-level hierarchy (D-U25) — use
    /// [`Self::effective_include_root`] for the binding value.
    pub fn topic_include_root(&self) -> bool {
        self.parsed.topic.include_root
    }

    /// The EFFECTIVE root mode (D-U27): `topic.includeRoot` AND a multi-level
    /// hierarchy (≥ 2 `hier` entries). This is the value bound into both the topic
    /// builder ([`crate::uns::Uns`]) and the reserved-class publish guard, so the
    /// guard's position-5 check always agrees with topic building.
    pub fn effective_include_root(&self) -> bool {
        self.parsed.topic.include_root && self.identity.hier().len() >= 2
    }

    /// The default `request()` deadline from `messaging.requestTimeoutSeconds`
    /// (UNS-CANONICAL-DESIGN §5 / D-U5): absent (or a schema-invalid negative) =
    /// the default 30 s; `0` = `None` (deadline disabled); an explicit per-call
    /// timeout always wins over this default.
    pub fn messaging_request_timeout(&self) -> Option<std::time::Duration> {
        match self.parsed.messaging.request_timeout_seconds {
            None => Some(std::time::Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECONDS)),
            Some(0.0) => None,
            Some(secs) if secs < 0.0 => {
                Some(std::time::Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECONDS))
            }
            Some(secs) => Some(std::time::Duration::from_secs_f64(secs)),
        }
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

    #[test]
    fn metric_config_defaults() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        let m = &cfg.parsed.metric_emission;
        assert_eq!(m.target(), "log");
        assert_eq!(m.namespace(), "ggcommons");
        assert!(m.log_file_name().contains("{ComponentFullName}"));
        assert_eq!(m.max_file_size(), "10MB");
        assert_eq!(m.destination(), "ipc");
        assert_eq!(m.interval_secs(), 5);
        assert!(!m.large_fleet_workaround);
    }

    #[test]
    fn explicit_log_file_name_absent_and_present() {
        let none = Config::from_value("c", "t", json!({})).unwrap();
        assert_eq!(none.parsed.metric_emission.explicit_log_file_name(), None);

        let set = Config::from_value(
            "c",
            "t",
            json!({ "metricEmission": { "target": "log", "targetConfig": { "logFileName": "/x.log" } } }),
        )
        .unwrap();
        assert_eq!(
            set.parsed.metric_emission.explicit_log_file_name(),
            Some("/x.log".to_string())
        );
    }

    #[test]
    fn metric_config_reads_target_config_values() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "metricEmission": {
                "target": "messaging",
                "namespace": "ns",
                "largeFleetWorkaround": true,
                "targetConfig": {
                    "logFileName": "/x.log",
                    "maxFileSize": "5MB",
                    "destination": "iotcore",
                    "intervalSecs": 10
                }
            } }),
        )
        .unwrap();
        let m = &cfg.parsed.metric_emission;
        assert_eq!(m.target(), "messaging");
        assert_eq!(m.namespace(), "ns");
        assert!(m.large_fleet_workaround);
        assert_eq!(m.log_file_name(), "/x.log");
        assert_eq!(m.max_file_size(), "5MB");
        assert_eq!(m.destination(), "iotcore");
        assert_eq!(m.interval_secs(), 10);
    }

    // ---------- heartbeat reshape (D-U14/D-U20) ----------

    #[test]
    fn heartbeat_defaults_are_on_5s_local_with_cpu_memory() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        let hb = &cfg.parsed.heartbeat;
        assert!(hb.enabled, "heartbeat defaults ON (D-U14)");
        assert_eq!(hb.interval_secs, None, "interval default applied by the runtime (5s)");
        assert_eq!(hb.destination(), "local");
        assert!(hb.measures.cpu && hb.measures.memory, "cpu/memory default on");
        assert!(!hb.measures.disk && !hb.measures.threads && !hb.measures.files && !hb.measures.fds);
    }

    #[test]
    fn heartbeat_reads_explicit_values() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "heartbeat": {
                "enabled": false,
                "intervalSecs": 9,
                "measures": { "cpu": false, "disk": true },
                "destination": "iotcore"
            } }),
        )
        .unwrap();
        let hb = &cfg.parsed.heartbeat;
        assert!(!hb.enabled);
        assert_eq!(hb.interval_secs, Some(9));
        assert_eq!(hb.destination(), "iotcore");
        assert!(!hb.measures.cpu, "explicit false wins");
        assert!(hb.measures.memory, "absent measure keeps its default (true)");
        assert!(hb.measures.disk);
    }

    // ---------- UNS identity + topic + messaging knobs ----------

    #[test]
    fn identity_resolves_from_own_config() {
        let cfg = Config::from_value(
            "com.example.OpcuaAdapter",
            "gw-01",
            json!({
                "hierarchy": { "levels": ["site", "device"] },
                "identity": { "site": "dallas" }
            }),
        )
        .unwrap();
        assert_eq!(cfg.identity().path(), "dallas/gw-01");
        assert_eq!(cfg.identity().component(), "OpcuaAdapter");
        assert_eq!(cfg.identity().instance(), "main");
    }

    #[test]
    fn identity_resolution_is_fail_fast() {
        let err = Config::from_value(
            "c",
            "t",
            json!({ "hierarchy": { "levels": ["site", "device"] } }),
        );
        assert!(err.is_err(), "missing identity.site must fail construction");
    }

    #[test]
    fn include_root_is_effective_only_with_multi_level_hierarchy() {
        // Single-level + includeRoot: raw true, effective false (D-U25/D-U27).
        let single =
            Config::from_value("c", "t", json!({ "topic": { "includeRoot": true } })).unwrap();
        assert!(single.topic_include_root());
        assert!(!single.effective_include_root());

        let multi = Config::from_value(
            "c",
            "gw-01",
            json!({
                "topic": { "includeRoot": true },
                "hierarchy": { "levels": ["site", "device"] },
                "identity": { "site": "dallas" }
            }),
        )
        .unwrap();
        assert!(multi.effective_include_root());

        let off = Config::from_value("c", "t", json!({})).unwrap();
        assert!(!off.topic_include_root() && !off.effective_include_root());
    }

    #[test]
    fn messaging_request_timeout_default_zero_and_fractional() {
        use std::time::Duration;
        let default = Config::from_value("c", "t", json!({})).unwrap();
        assert_eq!(default.messaging_request_timeout(), Some(Duration::from_secs(30)));

        let disabled =
            Config::from_value("c", "t", json!({ "messaging": { "requestTimeoutSeconds": 0 } }))
                .unwrap();
        assert_eq!(disabled.messaging_request_timeout(), None, "0 disables the default deadline");

        let fractional = Config::from_value(
            "c",
            "t",
            json!({ "messaging": { "requestTimeoutSeconds": 2.5 } }),
        )
        .unwrap();
        assert_eq!(fractional.messaging_request_timeout(), Some(Duration::from_secs_f64(2.5)));

        // A (schema-invalid) negative value falls back to the default.
        let negative = Config::from_value(
            "c",
            "t",
            json!({ "messaging": { "requestTimeoutSeconds": -1 } }),
        )
        .unwrap();
        assert_eq!(negative.messaging_request_timeout(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn messaging_section_reads_lwt_presence() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "messaging": { "lwt": { "topic": "ecv1/t/c/main/state" } } }),
        )
        .unwrap();
        assert!(cfg.parsed.messaging.lwt.is_some());
        let none = Config::from_value("c", "t", json!({})).unwrap();
        assert!(none.parsed.messaging.lwt.is_none());
    }

    #[test]
    fn cloudwatch_buffer_defaults_to_durable_when_absent() {
        let cfg = Config::from_value("c", "t", json!({ "metricEmission": { "target": "cloudwatch" } }))
            .unwrap();
        let m = &cfg.parsed.metric_emission;
        assert_eq!(m.buffer_type(), "durable", "default buffer is durable");
        assert_eq!(m.buffer_max_disk_bytes(), 134_217_728);
        assert_eq!(m.buffer_on_full(), "dropoldest");
        assert_eq!(m.buffer_fsync(), "perbatch");
        assert_eq!(m.buffer_path(), "/var/lib/ggcommons/metrics/{ComponentName}/cw");
    }

    #[test]
    fn cloudwatch_buffer_reads_explicit_values() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "metricEmission": { "target": "cloudwatch", "targetConfig": { "buffer": {
                "type": "memory", "path": "/data/cw", "maxDiskBytes": 65536.0,
                "onFull": "block", "fsync": "always"
            } } } }),
        )
        .unwrap();
        let m = &cfg.parsed.metric_emission;
        assert_eq!(m.buffer_type(), "memory");
        assert_eq!(m.buffer_path(), "/data/cw");
        assert_eq!(m.buffer_max_disk_bytes(), 65536); // float-from-Greengrass accepted
        assert_eq!(m.buffer_on_full(), "block");
        assert_eq!(m.buffer_fsync(), "always");
    }

    #[test]
    fn numeric_config_accepts_floats_from_greengrass() {
        // Greengrass returns config numbers as doubles (e.g. 10.0, not 10).
        let cfg = Config::from_value(
            "c",
            "t",
            json!({
                "heartbeat": { "intervalSecs": 10.0 },
                "metricEmission": { "targetConfig": { "intervalSecs": 7.0 } }
            }),
        )
        .unwrap();
        assert_eq!(cfg.parsed.heartbeat.interval_secs, Some(10));
        assert_eq!(cfg.parsed.metric_emission.interval_secs(), 7);
    }

    #[test]
    fn interval_secs_below_minimum_falls_back_to_default() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "metricEmission": { "targetConfig": { "intervalSecs": 0 } } }),
        )
        .unwrap();
        assert_eq!(cfg.parsed.metric_emission.interval_secs(), 5);
    }

    #[test]
    fn health_config_defaults() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        let h = &cfg.parsed.health;
        assert_eq!(h.enabled, None, "enabled is unset by default (profile decides)");
        assert_eq!(h.port(), 8081);
        assert_eq!(h.liveness_path(), "/livez");
        assert_eq!(h.readiness_path(), "/readyz");
        assert_eq!(h.startup_path(), "/startupz");
    }

    #[test]
    fn health_config_reads_explicit_values() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "health": {
                "enabled": true,
                "port": 9000,
                "livenessPath": "/alive",
                "readinessPath": "/ready",
                "startupPath": "/started"
            } }),
        )
        .unwrap();
        let h = &cfg.parsed.health;
        assert_eq!(h.enabled, Some(true));
        assert_eq!(h.port(), 9000);
        assert_eq!(h.liveness_path(), "/alive");
        assert_eq!(h.readiness_path(), "/ready");
        assert_eq!(h.startup_path(), "/started");
    }

    #[test]
    fn health_port_accepts_float_from_greengrass() {
        // Greengrass delivers config numbers as doubles (e.g. 8082.0).
        let cfg =
            Config::from_value("c", "t", json!({ "health": { "port": 8082.0 } })).unwrap();
        assert_eq!(cfg.parsed.health.port(), 8082);
    }

    #[test]
    fn instance_lookup_returns_none_for_missing_id() {
        let cfg = Config::from_value(
            "c",
            "t",
            json!({ "component": { "instances": [ { "id": "a" } ] } }),
        )
        .unwrap();
        assert!(cfg.instance("a").is_some());
        assert!(cfg.instance("missing").is_none());
        assert!(cfg.global().is_null() || cfg.global().is_object());
    }
}
