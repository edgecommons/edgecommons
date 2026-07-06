//! # Configuration — validation
//!
//! **One-liner purpose**: Validate the configuration document against the embedded
//! JSON schema.
//!
//! ## Overview
//! The schema is **embedded** with `include_str!`, so it can never be "missing
//! from the classpath" — closing the fail-open hole in the Java validator. A
//! document that does not satisfy the schema is a hard error by default.
//!
//! ## Semantics & Architecture
//! - Synchronous; compiles the schema per call (config loading is infrequent).
//! - Fail-closed: any schema violation returns [`crate::error::EdgeCommonsError::Validation`]
//!   listing every error.
//!
//! ## Usage Example
//! ```
//! use edgecommons::config::validation::validate;
//! use serde_json::json;
//!
//! // A valid document must include the required top-level `component` object.
//! assert!(validate(&json!({ "component": {}, "logging": { "level": "INFO" } })).is_ok());
//! assert!(validate(&json!({ "component": {}, "metricEmission": { "target": "nope" } })).is_err());
//! ```
//!
//! ## Design Choices
//! Embedding (vs. loading from disk) guarantees validation can't be silently
//! skipped due to packaging mistakes.
//!
//! ## Safety & Panics
//! None; an invalid embedded schema is reported as an error, not a panic.
//!
//! ## Related Modules
//! - [`super::model`], [`super`].

use serde_json::Value;

use crate::error::{EdgeCommonsError, Result};

const SCHEMA: &str = include_str!("../../resources/edgecommons-config-schema.json");

/// Validate `instance` against the embedded config schema.
pub fn validate(instance: &Value) -> Result<()> {
    let schema: Value = serde_json::from_str(SCHEMA).map_err(|e| {
        EdgeCommonsError::Validation(format!("embedded schema is not valid JSON: {e}"))
    })?;

    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| EdgeCommonsError::Validation(format!("embedded schema is invalid: {e}")))?;

    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{e} (at {})", e.instance_path()))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(EdgeCommonsError::Validation(errors.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_a_well_formed_config() {
        let cfg = json!({
            "logging": { "level": "INFO" },
            "metricEmission": { "target": "cloudwatch", "namespace": "ns" },
            "heartbeat": { "enabled": true, "intervalSecs": 5,
                           "measures": { "cpu": true }, "destination": "local" },
            "hierarchy": { "levels": ["site", "device"] },
            "identity": { "site": "dallas" },
            "topic": { "includeRoot": true },
            "messaging": { "requestTimeoutSeconds": 30 },
            "component": { "global": {}, "instances": [] }
        });
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn rejects_generic_messaging_lwt() {
        let cfg = json!({
            "component": { "global": {} },
            "messaging": { "lwt": { "topic": "ecv1/d/c/main/state", "qos": 1 } }
        });
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn accepts_broker_qos_and_rejects_top_level_qos() {
        let valid = json!({
            "component": { "global": {} },
            "messaging": {
                "local": {
                    "host": "localhost",
                    "port": 1883,
                    "clientId": "local",
                    "qos": { "publish": 1, "subscribe": 1 }
                },
                "northbound": {
                    "host": "broker.example.com",
                    "port": 8883,
                    "clientId": "northbound",
                    "qos": { "publish": 2, "subscribe": 1 }
                }
            }
        });
        assert!(validate(&valid).is_ok());

        let stale = json!({
            "component": { "global": {} },
            "messaging": {
                "local": { "host": "localhost", "port": 1883, "clientId": "local" },
                "qos": { "local": { "publish": 1 } }
            }
        });
        assert!(validate(&stale).is_err());
    }

    #[test]
    fn rejects_a_bad_metric_target() {
        let cfg = json!({ "metricEmission": { "target": "not-a-target" } });
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_the_removed_heartbeat_targets_drift_knob() {
        // UNS hard cut (D-U20): heartbeat.targets[] is gone from the schema — a
        // stale config must fail with a precise error, not silently drift.
        let cfg = json!({
            "heartbeat": { "targets": [ { "type": "metric" } ] },
            "component": {}
        });
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_the_removed_metric_topic_override() {
        // UNS hard cut (D-U9): metricEmission.targetConfig.topic is gone.
        let cfg = json!({
            "metricEmission": { "target": "messaging", "targetConfig": { "topic": "x/y" } },
            "component": {}
        });
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_wrong_type_for_interval() {
        let cfg = json!({ "heartbeat": { "intervalSecs": "five" } });
        assert!(validate(&cfg).is_err());
    }
}
