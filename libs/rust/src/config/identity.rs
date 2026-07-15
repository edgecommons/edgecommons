//! # Configuration — UNS component-identity resolution
//!
//! **One-liner purpose**: Resolve the component's UNS [`MessageIdentity`] from its
//! OWN config (top-level `hierarchy` + `identity` blocks) — once per configuration
//! snapshot, fail-fast (UNS-CANONICAL-DESIGN §1.5, D-U1/D-U2/D-U10).
//!
//! ## Resolution algorithm (identical across all four languages)
//! 1. `levels` = top-level `hierarchy.levels` when present, else the zero-config
//!    default `["device"]` — the UNS works out of the box as
//!    `ecv1/{thing}/{comp}/{class}` (D-U28: the component identity is component scope,
//!    so its topics carry no instance slot).
//! 2. Level **names** must match `^[A-Za-z0-9_-]+$`, be unique and non-empty (they
//!    become Parquet columns in a later phase — keep them strict).
//! 3. Every level **except the last** takes its value from the top-level `identity`
//!    config object — a missing value is a startup error naming the level(s). The
//!    **last level's value = the resolved thing name** (the existing platform
//!    identity chain — D-U1). An `identity` key equal to the last level name, or not
//!    among the declared non-device levels, is a startup error (typo protection the
//!    schema cannot express).
//! 4. Every **value** passes through the template sanitizer
//!    ([`crate::config::template::sanitize`]); a changed value is WARN-logged and the
//!    sanitized value is used.
//! 5. `component` = `component.token` when configured, otherwise the sanitized SHORT
//!    component name (the segment after the last `.` — the existing `{ComponentName}`
//!    fallback semantics, D-U18).
//!
//! NO shared config: each component reads its own `hierarchy`/`identity` blocks.
//!
//! ## Related Modules
//! - [`super::model`] — calls [`resolve`] from `Config::from_value`.
//! - [`crate::messaging::message`] — the resolved [`MessageIdentity`] type.

use serde_json::Value;

use crate::config::template::sanitize;
use crate::error::{EdgeCommonsError, Result};
use crate::messaging::message::{HierEntry, MessageIdentity};

/// Strict UNS hierarchy level-name rule: `^[A-Za-z0-9_-]+$` (future Parquet columns).
fn is_valid_level_name(level: &str) -> bool {
    !level.is_empty()
        && level
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Builds the uniform fail-fast identity-resolution startup error.
fn identity_error(detail: impl std::fmt::Display) -> EdgeCommonsError {
    EdgeCommonsError::Config(format!("Component identity resolution failed: {detail}"))
}

/// Sanitizes an identity value via the template sanitizer, WARN-logging when it changed.
fn sanitized_identity_value(what: &str, raw_value: &str) -> String {
    let sanitized = sanitize(raw_value);
    if sanitized != raw_value {
        tracing::warn!(
            "Identity value for '{what}' contained reserved characters and was sanitized: \
             '{raw_value}' -> '{sanitized}'"
        );
    }
    sanitized
}

/// Reduces a component name to its short form (the segment after the last `.`) —
/// the existing `{ComponentName}` semantics (D-U18).
pub(crate) fn short_component_name(component_name: &str) -> &str {
    component_name.rsplit('.').next().unwrap_or(component_name)
}

fn configured_component_token(raw: &Value) -> Result<Option<&str>> {
    let Some(component_el) = raw.get("component") else {
        return Ok(None);
    };
    let Some(component) = component_el.as_object() else {
        return Err(identity_error(
            "'component' must be an object when configuring 'component.token'",
        ));
    };
    let Some(token_el) = component.get("token") else {
        return Ok(None);
    };
    let Some(token) = token_el.as_str().filter(|v| !v.is_empty()) else {
        return Err(identity_error(
            "'component.token' must be a non-empty string",
        ));
    };
    Ok(Some(token))
}

/// Resolves the component's UNS identity (component scope — no instance, D-U28) from
/// the raw config document + the resolved thing name + the (full or short) component
/// name. See the [module docs](self) for the algorithm.
///
/// # Errors
/// [`EdgeCommonsError::Config`] naming the precise inconsistency (fail-fast at construction).
pub(crate) fn resolve(
    raw: &Value,
    thing_name: &str,
    component_name: &str,
) -> Result<MessageIdentity> {
    // 1. levels = hierarchy.levels if present, else the zero-config default ["device"].
    let mut levels: Vec<String> = Vec::new();
    if let Some(hierarchy) = raw.get("hierarchy") {
        let Some(levels_el) = hierarchy.as_object().and_then(|h| h.get("levels")) else {
            return Err(identity_error(
                "'hierarchy' must be an object with a 'levels' array",
            ));
        };
        let Some(levels_arr) = levels_el.as_array().filter(|a| !a.is_empty()) else {
            return Err(identity_error(
                "'hierarchy.levels' must be a non-empty array of level names",
            ));
        };
        for level_el in levels_arr {
            let Some(level) = level_el.as_str() else {
                return Err(identity_error("'hierarchy.levels' entries must be strings"));
            };
            levels.push(level.to_string());
        }
    } else {
        levels.push("device".to_string());
    }

    // 2. Level names: strict charset, unique, non-empty.
    let mut seen: Vec<&str> = Vec::with_capacity(levels.len());
    for level in &levels {
        if !is_valid_level_name(level) {
            return Err(identity_error(format!(
                "invalid hierarchy level name '{level}' (must match ^[A-Za-z0-9_-]+$)"
            )));
        }
        if seen.contains(&level.as_str()) {
            return Err(identity_error(format!(
                "duplicate hierarchy level name '{level}'"
            )));
        }
        seen.push(level);
    }
    let device_level = levels[levels.len() - 1].clone();
    let value_levels = &levels[..levels.len() - 1];

    // 3/4. The `identity` config object supplies every level's value except the last;
    //      keys must be exactly (a subset of) the non-device levels.
    let empty = serde_json::Map::new();
    let identity_config = match raw.get("identity") {
        None => &empty,
        Some(identity_el) => identity_el
            .as_object()
            .ok_or_else(|| identity_error("'identity' must be an object of level-name -> value"))?,
    };
    for key in identity_config.keys() {
        if *key == device_level {
            return Err(identity_error(format!(
                "'identity.{key}' must not be set: '{device_level}' is the last hierarchy level \
                 (the device) and its value is always the resolved thing name"
            )));
        }
        if !value_levels.contains(key) {
            return Err(identity_error(format!(
                "'identity.{key}' is not a declared hierarchy level; expected keys: {value_levels:?}"
            )));
        }
    }

    let mut hier: Vec<HierEntry> = Vec::with_capacity(levels.len());
    let mut missing: Vec<&str> = Vec::new();
    for level in value_levels {
        match identity_config
            .get(level)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            Some(value) => hier.push(HierEntry {
                level: level.clone(),
                value: sanitized_identity_value(level, value),
            }),
            None => missing.push(level),
        }
    }
    if !missing.is_empty() {
        return Err(identity_error(format!(
            "the top-level 'identity' config object is missing value(s) for hierarchy level(s) \
             {missing:?} (hierarchy.levels = {levels:?}; the last level '{device_level}' is the \
             resolved thing name and must not be configured)"
        )));
    }

    // The device (last level) value is the resolved thing name (platform-resolver chain).
    if thing_name.is_empty() {
        return Err(identity_error(format!(
            "the device level '{device_level}' value (the resolved thing name) is not available"
        )));
    }
    hier.push(HierEntry {
        level: device_level,
        value: sanitized_identity_value("device", thing_name),
    });

    // 5. component = explicit token when configured, else sanitized short name.
    if component_name.is_empty() {
        return Err(identity_error("the component name is not available"));
    }
    let raw_component_token =
        configured_component_token(raw)?.unwrap_or_else(|| short_component_name(component_name));
    let component_token = sanitized_identity_value("component", raw_component_token);
    MessageIdentity::new(hier, component_token, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zero_config_default_is_single_device_level() {
        let id = resolve(&json!({}), "gw-01", "com.example.OpcuaAdapter").unwrap();
        assert_eq!(id.hier().len(), 1);
        assert_eq!(id.hier()[0].level, "device");
        assert_eq!(id.device(), "gw-01");
        assert_eq!(id.path(), "gw-01");
        assert_eq!(
            id.component(),
            "OpcuaAdapter",
            "short name (segment after last '.')"
        );
        // D-U28: the resolved component identity is component scope (no instance).
        assert_eq!(id.instance(), None);
    }

    #[test]
    fn configured_component_token_overrides_pascal_component_name() {
        let raw = json!({ "component": { "token": "opcua-adapter" } });
        let id = resolve(&raw, "gw-01", "com.mbreissi.edgecommons.OpcUaAdapter").unwrap();
        assert_eq!(id.component(), "opcua-adapter");
    }

    #[test]
    fn malformed_component_token_fails_fast() {
        for raw in [
            json!({ "component": "nope" }),
            json!({ "component": { "token": "" } }),
            json!({ "component": { "token": 42 } }),
        ] {
            assert!(resolve(&raw, "t", "c").is_err(), "should fail: {raw}");
        }
    }

    #[test]
    fn multi_level_hierarchy_resolves_values_and_device() {
        let raw = json!({
            "hierarchy": { "levels": ["site", "factory", "zone", "device"] },
            "identity": { "site": "dallas", "factory": "finishing", "zone": "zone-3" }
        });
        let id = resolve(&raw, "gw-01", "opcua-adapter").unwrap();
        assert_eq!(
            id.hier()
                .iter()
                .map(|e| e.value.as_str())
                .collect::<Vec<_>>(),
            vec!["dallas", "finishing", "zone-3", "gw-01"]
        );
        assert_eq!(id.path(), "dallas/finishing/zone-3/gw-01");
        assert_eq!(id.device(), "gw-01");
    }

    #[test]
    fn values_pass_through_the_sanitizer() {
        let raw = json!({
            "hierarchy": { "levels": ["site", "device"] },
            "identity": { "site": "dal/las" }
        });
        let id = resolve(&raw, "gw+01", "opcua#adapter").unwrap();
        assert_eq!(id.hier()[0].value, "dal_las");
        assert_eq!(id.device(), "gw_01");
        assert_eq!(id.component(), "opcua_adapter");
    }

    #[test]
    fn malformed_hierarchy_shapes_fail_fast() {
        for raw in [
            json!({ "hierarchy": "nope" }),
            json!({ "hierarchy": {} }),
            json!({ "hierarchy": { "levels": [] } }),
            json!({ "hierarchy": { "levels": "device" } }),
            json!({ "hierarchy": { "levels": [42] } }),
        ] {
            assert!(resolve(&raw, "t", "c").is_err(), "should fail: {raw}");
        }
    }

    #[test]
    fn level_names_are_strict_and_unique() {
        let bad_name = json!({ "hierarchy": { "levels": ["si te", "device"] } });
        let err = resolve(&bad_name, "t", "c").unwrap_err().to_string();
        assert!(err.contains("invalid hierarchy level name"), "{err}");

        let dup = json!({ "hierarchy": { "levels": ["device", "device"] } });
        let err = resolve(&dup, "t", "c").unwrap_err().to_string();
        assert!(err.contains("duplicate hierarchy level name"), "{err}");
    }

    #[test]
    fn identity_key_for_device_level_is_a_startup_error() {
        let raw = json!({
            "hierarchy": { "levels": ["site", "device"] },
            "identity": { "site": "dallas", "device": "forged" }
        });
        let err = resolve(&raw, "t", "c").unwrap_err().to_string();
        assert!(err.contains("'identity.device' must not be set"), "{err}");
    }

    #[test]
    fn undeclared_identity_key_is_a_startup_error() {
        let raw = json!({
            "hierarchy": { "levels": ["site", "device"] },
            "identity": { "site": "dallas", "zone": "typo" }
        });
        let err = resolve(&raw, "t", "c").unwrap_err().to_string();
        assert!(
            err.contains("'identity.zone' is not a declared hierarchy level"),
            "{err}"
        );
    }

    #[test]
    fn missing_identity_values_are_named() {
        let raw = json!({
            "hierarchy": { "levels": ["site", "zone", "device"] },
            "identity": { "site": "dallas" }
        });
        let err = resolve(&raw, "t", "c").unwrap_err().to_string();
        assert!(
            err.contains("missing value(s)") && err.contains("zone"),
            "{err}"
        );
    }

    #[test]
    fn missing_thing_or_component_fails() {
        assert!(resolve(&json!({}), "", "c").is_err(), "no thing name");
        assert!(resolve(&json!({}), "t", "").is_err(), "no component name");
    }

    #[test]
    fn non_string_identity_value_counts_as_missing() {
        let raw = json!({
            "hierarchy": { "levels": ["site", "device"] },
            "identity": { "site": 42 }
        });
        assert!(resolve(&raw, "t", "c").is_err());
    }

    #[test]
    fn short_component_name_semantics() {
        assert_eq!(short_component_name("com.example.MyComp"), "MyComp");
        assert_eq!(short_component_name("Simple"), "Simple");
    }
}
