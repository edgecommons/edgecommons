//! The schema drift gate.
//!
//! `ec-validate` embeds `schema/edgecommons-config-schema.json` at compile time so that
//! validation is offline by construction. That embedding is a *copy*, and a copy can drift —
//! which is exactly the failure the four libraries already guard against with
//! `schema/sync-schema.sh --check`. This is the CLI's equivalent gate.
//!
//! Because the embed is an `include_str!` of the canonical file itself (not a vendored
//! duplicate), drift is structurally impossible *unless someone vendors a second copy*. This
//! test exists to fail loudly if that ever happens.

use std::path::PathBuf;

#[test]
fn the_embedded_schema_is_the_canonical_schema() {
    let canonical = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("ec-validate must live at <root>/cli/crates/ec-validate")
        .join("schema")
        .join("edgecommons-config-schema.json");

    let on_disk = std::fs::read_to_string(&canonical)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", canonical.display()));

    assert_eq!(
        ec_validate::schema::CANONICAL_SCHEMA,
        on_disk,
        "the embedded config schema has drifted from schema/edgecommons-config-schema.json — \
         the schema is single-source; edit it there and rebuild"
    );
}

#[test]
fn the_canonical_schema_still_leaves_component_config_unvalidated() {
    // The load-bearing premise of the two-schema design (DESIGN-cli §6.1): the canonical
    // schema validates the *envelope*, and is deliberately blind to what a component puts
    // under `component.global`. If this ever changes, the per-component schema story needs
    // revisiting — so assert the premise rather than trusting a memory of it.
    let schema: serde_json::Value =
        serde_json::from_str(ec_validate::schema::CANONICAL_SCHEMA).unwrap();

    let top_strict = schema
        .get("additionalProperties")
        .and_then(serde_json::Value::as_bool);
    assert_eq!(top_strict, Some(false), "the top level must remain strict");

    let global = schema
        .pointer("/properties/component/properties/global")
        .expect("component.global must exist");
    assert_eq!(
        global
            .get("additionalProperties")
            .and_then(serde_json::Value::as_bool),
        Some(true),
        "component.global is open by design — the component's own config.schema.json is what closes it"
    );
    assert!(
        global.get("properties").is_none(),
        "component.global declares no properties — this is the hole config.schema.json fills"
    );
}
