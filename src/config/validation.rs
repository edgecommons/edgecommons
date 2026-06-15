//! JSON-schema validation of the configuration document.
//!
//! The schema is **embedded** with `include_str!`, so it can never be "missing
//! from the classpath" — closing the fail-open hole in the Java validator. A
//! document that does not satisfy the schema is a hard error by default.

use serde_json::Value;

use crate::error::{GgError, Result};

const SCHEMA: &str = include_str!("../../resources/ggcommons-config-schema.json");

/// Validate `instance` against the embedded config schema.
pub fn validate(instance: &Value) -> Result<()> {
    let schema: Value = serde_json::from_str(SCHEMA)
        .map_err(|e| GgError::Validation(format!("embedded schema is not valid JSON: {e}")))?;

    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| GgError::Validation(format!("embedded schema is invalid: {e}")))?;

    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{e} (at {})", e.instance_path))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(GgError::Validation(errors.join("; ")))
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
            "heartbeat": { "intervalSecs": 5, "targets": [ { "type": "metric" } ] },
            "component": { "global": {}, "instances": [] }
        });
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn rejects_a_bad_metric_target() {
        let cfg = json!({ "metricEmission": { "target": "not-a-target" } });
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_wrong_type_for_interval() {
        let cfg = json!({ "heartbeat": { "intervalSecs": "five" } });
        assert!(validate(&cfg).is_err());
    }
}
