//! Layer 1 — schema validation (DESIGN-cli §6.1).
//!
//! Two schemas, because one of them did not exist and its absence was a live hole:
//!
//! **(a) The library envelope** — the canonical `schema/edgecommons-config-schema.json`,
//! embedded at compile time so validation is offline by construction.
//!
//! **(b) The component's own config** — which, until now, was validated by **nothing**. The
//! canonical schema is strict at the top level (`additionalProperties: false`), but
//! `component.global` is `additionalProperties: true` with **zero declared properties**, and
//! no component repo shipped a schema. A typo in a `telemetry-processor` pipeline was caught
//! by no tool, at any stage. Every template now ships a `config.schema.json`, and that one
//! artifact is consumed here, by `deployment validate` against the *pinned version's* schema
//! (D-CLI-16), and by the runtime itself (RM-014).

use ec_diag::{Diagnostic, Report};
use serde_json::Value;

/// The canonical config schema, compiled into the binary.
///
/// Embedding is what keeps `component validate` offline (P2). A CI gate asserts this copy
/// matches `schema/edgecommons-config-schema.json`, mirroring the existing `sync-schema.sh
/// --check` drift gate the four libraries already use.
pub const CANONICAL_SCHEMA: &str = include_str!("../../../../schema/edgecommons-config-schema.json");

/// The name a component gives its own config schema.
pub const COMPONENT_SCHEMA_NAME: &str = "config.schema.json";

/// Validate a component config against the canonical library envelope.
#[must_use]
pub fn validate_envelope(config: &Value, source: &str) -> Report {
    let mut report = Report::new();
    let schema: Value = serde_json::from_str(CANONICAL_SCHEMA).expect("embedded canonical schema must parse");

    let validator = match jsonschema::validator_for(&schema) {
        Ok(v) => v,
        Err(e) => {
            report.push(Diagnostic::error(
                ec_diag::EC1001_SCHEMA,
                format!("the embedded canonical schema is not a valid JSON Schema: {e}"),
            ));
            return report;
        }
    };

    for err in validator.iter_errors(config) {
        report.push(
            Diagnostic::error(ec_diag::EC1001_SCHEMA, err.to_string())
                .with_file(source)
                .with_pointer(err.instance_path.to_string())
                .with_help("this key is rejected by the canonical edgecommons config schema"),
        );
    }
    report
}

/// Validate the **component's own** config section against the schema it publishes.
///
/// `component_schema` is the contents of the component's `config.schema.json`. When a
/// component publishes none, the caller gets [`no_component_schema`] instead — a **warning**,
/// not an error, saying so out loud rather than implying coverage that does not exist.
#[must_use]
pub fn validate_component_section(config: &Value, component_schema: &Value, source: &str) -> Report {
    let mut report = Report::new();

    let validator = match jsonschema::validator_for(component_schema) {
        Ok(v) => v,
        Err(e) => {
            report.push(
                Diagnostic::error(
                    ec_diag::EC1002_COMPONENT_SCHEMA,
                    format!("{COMPONENT_SCHEMA_NAME} is not a valid JSON Schema: {e}"),
                )
                .with_file(COMPONENT_SCHEMA_NAME),
            );
            return report;
        }
    };

    // The component's schema describes what lives under `component.global`. Anything else is
    // the library's envelope and is not this schema's business.
    let Some(global) = config.pointer("/component/global") else {
        return report; // nothing to check
    };

    for err in validator.iter_errors(global) {
        report.push(
            Diagnostic::error(ec_diag::EC1002_COMPONENT_SCHEMA, err.to_string())
                .with_file(source)
                .with_pointer(format!("/component/global{}", err.instance_path))
                .with_help(format!(
                    "this key is not accepted by the component's own {COMPONENT_SCHEMA_NAME}"
                )),
        );
    }
    report
}

/// The warning emitted when a component publishes no schema of its own.
///
/// Deliberately a warning: until components actually publish (RM-013), refusing to validate
/// would block every existing component. But it must be *said*, not silently skipped — the
/// tooling never implies coverage it does not have.
#[must_use]
pub fn no_component_schema(component: &str) -> Diagnostic {
    Diagnostic::warning(
        ec_diag::EC1003_NO_COMPONENT_SCHEMA,
        format!("`{component}` publishes no {COMPONENT_SCHEMA_NAME}, so its own config is not validated"),
    )
    .with_help(format!(
        "add a {COMPONENT_SCHEMA_NAME} describing what goes under `component.global` — \
         `edgecommons component new` scaffolds one"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_embedded_canonical_schema_is_a_valid_json_schema() {
        let schema: Value = serde_json::from_str(CANONICAL_SCHEMA).unwrap();
        assert!(jsonschema::validator_for(&schema).is_ok());
    }

    #[test]
    fn a_valid_config_passes_the_envelope() {
        let cfg = json!({ "component": { "token": "MyComponent" } });
        let r = validate_envelope(&cfg, "config.json");
        assert_eq!(r.error_count(), 0, "{}", r.render_human());
    }

    #[test]
    fn an_unknown_top_level_key_is_rejected() {
        // The top level is strict (additionalProperties: false).
        let cfg = json!({ "component": { "token": "X" }, "nonsense": true });
        let r = validate_envelope(&cfg, "config.json");
        assert!(r.error_count() > 0);
        assert_eq!(r.diagnostics[0].code, ec_diag::EC1001_SCHEMA);
    }

    #[test]
    fn a_missing_component_section_is_rejected() {
        let cfg = json!({ "logging": {} });
        let r = validate_envelope(&cfg, "config.json");
        assert!(r.error_count() > 0, "`component` is required");
    }

    #[test]
    fn the_canonical_schema_does_not_check_the_components_own_config() {
        // This is the hole. `component.global` is additionalProperties:true with no declared
        // properties, so the canonical schema happily accepts a typo'd pipeline key. If this
        // test ever starts failing, the canonical schema grew component-specific knowledge and
        // the two-schema split needs revisiting.
        let cfg = json!({
            "component": { "token": "X", "global": { "totally": "made up", "pipelnie": [] } }
        });
        let r = validate_envelope(&cfg, "config.json");
        assert_eq!(r.error_count(), 0, "the envelope schema is blind to component config — by design");
    }

    #[test]
    fn the_components_own_schema_catches_what_the_envelope_cannot() {
        let component_schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": { "pipeline": { "type": "array" } }
        });
        let cfg = json!({
            "component": { "token": "X", "global": { "pipelnie": [] } }  // typo
        });
        let r = validate_component_section(&cfg, &component_schema, "config.json");
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC1002_COMPONENT_SCHEMA);
        // The diagnostic must point at the offending key, not at the document.
        assert!(
            r.diagnostics[0].locus.as_ref().unwrap().to_string().starts_with("/component/global"),
            "{:?}",
            r.diagnostics[0].locus
        );
    }

    #[test]
    fn a_correct_component_config_passes_its_own_schema() {
        let component_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "pipeline": { "type": "array" } }
        });
        let cfg = json!({ "component": { "token": "X", "global": { "pipeline": [] } } });
        let r = validate_component_section(&cfg, &component_schema, "config.json");
        assert_eq!(r.error_count(), 0, "{}", r.render_human());
    }

    #[test]
    fn a_config_with_no_component_global_is_simply_not_checked() {
        let component_schema = json!({ "type": "object", "additionalProperties": false });
        let cfg = json!({ "component": { "token": "X" } });
        let r = validate_component_section(&cfg, &component_schema, "config.json");
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn a_missing_component_schema_warns_rather_than_blocking() {
        // RM-013's degradation rule: until components publish, say so — do not fail.
        let d = no_component_schema("telemetry-processor");
        assert_eq!(d.severity, ec_diag::Severity::Warning);
        assert_eq!(d.code, ec_diag::EC1003_NO_COMPONENT_SCHEMA);
        assert!(d.message.contains("not validated"));
    }
}
