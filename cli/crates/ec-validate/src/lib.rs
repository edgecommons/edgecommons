//! The validation engine (DESIGN-cli §6).
//!
//! Three layers, one diagnostic stream:
//!
//! 1. [`schema`] — the canonical library envelope, **plus** the component's own
//!    `config.schema.json`, which is the part nothing validated before.
//! 2. [`semantic`] — the rules JSON Schema cannot express.
//! 3. [`artifact`] — recipe and `gdk-config.json`, **parsed** rather than regexed.
//!
//! Built once and shared by `component validate` and `deployment validate` (D-CLI-6). Two
//! implementations would drift, and the Studio needs exactly the effective-config validation a
//! component author already wants.

pub mod artifact;
pub mod schema;
pub mod semantic;

use std::path::Path;

use ec_deploy::Platform;
use ec_diag::Report;
use serde_json::Value;

/// Validate one component config: envelope, the component's own schema, and semantics.
///
/// `component_schema` is the component's `config.schema.json` when it publishes one. When it
/// does not, the caller should add [`schema::no_component_schema`] — a warning, not a failure
/// (RM-013's degradation rule).
#[must_use]
pub fn validate_config(
    config: &Value,
    component_schema: Option<&Value>,
    platform: Option<Platform>,
    source: &str,
) -> Report {
    let mut r = Report::new();
    r.extend(schema::validate_envelope(config, source).diagnostics);
    if let Some(cs) = component_schema {
        r.extend(schema::validate_component_section(config, cs, source).diagnostics);
    }
    r.extend(semantic::check(config, platform, source).diagnostics);
    r
}

/// Validate a whole component project: every config it ships, plus its artifacts.
///
/// `platform` is what the config is destined for. Some rules (`EC2001` transport/platform,
/// `EC2009` config-source/platform) are only *decidable* with it — without one they are skipped
/// rather than guessed at, so supplying it is what makes them reachable at all.
#[must_use]
pub fn validate_project(root: &Path, only: Option<&Path>, platform: Option<Platform>) -> Report {
    let mut r = Report::new();

    // The component's own schema, if it publishes one.
    let schema_path = root.join(schema::COMPONENT_SCHEMA_NAME);
    let component_schema: Option<Value> = std::fs::read_to_string(&schema_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());

    if component_schema.is_none() {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        r.push(schema::no_component_schema(&name));
    }

    let configs: Vec<std::path::PathBuf> = match only {
        Some(p) => vec![p.to_path_buf()],
        None => discover_configs(root),
    };

    for cfg_path in configs {
        let Ok(text) = std::fs::read_to_string(&cfg_path) else {
            continue;
        };
        let source = cfg_path.display().to_string();
        match serde_json::from_str::<Value>(&text) {
            Ok(cfg) => {
                r.extend(
                    validate_config(&cfg, component_schema.as_ref(), platform, &source).diagnostics,
                );
            }
            Err(e) => r.push(
                ec_diag::Diagnostic::error(
                    ec_diag::EC1001_SCHEMA,
                    format!("config is not valid JSON: {e}"),
                )
                .with_file(&cfg_path),
            ),
        }
    }

    r.extend(artifact::lint_recipe(&root.join("recipe.yaml")).diagnostics);
    r.extend(artifact::lint_gdk_config(&root.join("gdk-config.json")).diagnostics);
    r.extend(artifact::lint_k8s(&root.join("k8s")).diagnostics);

    r
}

/// The configs a component project ships: everything in `test-configs/`, minus the messaging
/// configs, which are a transport document rather than a component config.
fn discover_configs(root: &Path) -> Vec<std::path::PathBuf> {
    let dir = root.join("test-configs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("messaging"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_project_with_no_component_schema_warns_but_does_not_fail() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Thing");
        std::fs::create_dir_all(root.join("test-configs")).unwrap();
        std::fs::write(
            root.join("test-configs/config.json"),
            serde_json::to_string(&json!({ "component": { "token": "thing" } })).unwrap(),
        )
        .unwrap();

        let r = validate_project(&root, None, None);
        assert_eq!(r.error_count(), 0, "{}", r.render_human());
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.diagnostics[0].code, ec_diag::EC1003_NO_COMPONENT_SCHEMA);
    }

    #[test]
    fn a_typo_in_the_components_own_config_is_finally_caught() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Thing");
        std::fs::create_dir_all(root.join("test-configs")).unwrap();
        std::fs::write(
            root.join("config.schema.json"),
            serde_json::to_string(&json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "publish_interval": { "type": "integer" } }
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("test-configs/config.json"),
            serde_json::to_string(&json!({
                "component": { "token": "thing", "global": { "publish_intervall": 5 } }
            }))
            .unwrap(),
        )
        .unwrap();

        let r = validate_project(&root, None, None);
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC1002_COMPONENT_SCHEMA);
    }

    #[test]
    fn a_valid_project_is_clean() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Thing");
        std::fs::create_dir_all(root.join("test-configs")).unwrap();
        std::fs::write(
            root.join("config.schema.json"),
            serde_json::to_string(&json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "publish_interval": { "type": "integer" } }
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("test-configs/config.json"),
            serde_json::to_string(&json!({
                "component": { "token": "thing", "global": { "publish_interval": 5 } }
            }))
            .unwrap(),
        )
        .unwrap();

        let r = validate_project(&root, None, None);
        assert_eq!(r.error_count(), 0, "{}", r.render_human());
        assert_eq!(r.warning_count(), 0, "{}", r.render_human());
    }

    #[test]
    fn messaging_configs_are_not_treated_as_component_configs() {
        // standalone-messaging.json is a transport document; running it through the component
        // config schema would produce nonsense errors.
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Thing");
        std::fs::create_dir_all(root.join("test-configs")).unwrap();
        std::fs::write(
            root.join("test-configs/standalone-messaging.json"),
            r#"{"endpoint":"x"}"#,
        )
        .unwrap();
        let found = discover_configs(&root);
        assert!(found.is_empty(), "{found:?}");
    }
}
