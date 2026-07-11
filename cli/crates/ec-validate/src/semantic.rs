//! Layer 2 — semantic rules (DESIGN-cli §6.2).
//!
//! The constraints JSON Schema cannot express. Each has a stable code, so CI can pin behavior
//! to `EC2003` rather than to a sentence that may be reworded.

use ec_deploy::{ConfigSource, Platform};
use ec_diag::{Diagnostic, Report};
use serde_json::Value;

/// Run every semantic rule against a config.
///
/// `platform` is the platform the config is destined for, when known. Some rules
/// (`EC2001`, `EC2009`) are only decidable with it; without one they are skipped rather
/// than guessed at.
#[must_use]
pub fn check(config: &Value, platform: Option<Platform>, source: &str) -> Report {
    let mut r = Report::new();
    r.extend(secret_values(config, source));
    r.extend(config_source_platform(config, platform, source));
    r.extend(transport_platform(config, platform, source));
    r.extend(config_bootstrap_loop(config, source));
    r
}

/// `EC2005` — secret **values** are forbidden anywhere; only `secret://` references.
///
/// A secret that reaches Git is a secret that has leaked, and the deployment model stores
/// references and policies, never values.
fn secret_values(config: &Value, source: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    walk(config, "", &mut |pointer, key, value| {
        let Some(text) = value.as_str() else { return };
        if !looks_secretish(key) || is_known_safe(pointer) {
            return;
        }
        // A `secret://` reference is the sanctioned form; a `$secret` config reference and an
        // empty placeholder are fine too. Anything else at a secret-shaped key is a value.
        if text.starts_with("secret://") || text.starts_with("$secret") || text.is_empty() {
            return;
        }
        out.push(
            Diagnostic::error(
                ec_diag::EC2005_SECRET_VALUE,
                format!("`{key}` looks like a secret value; store a reference, not the value"),
            )
            .with_file(source)
            .with_pointer(pointer)
            .with_help("use a `secret://<provider>/<path>` reference — the deployment model never stores secret values"),
        );
    });
    out
}

fn looks_secretish(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "password",
        "secret",
        "token",
        "apikey",
        "api_key",
        "privatekey",
        "private_key",
        "credential",
    ]
    .iter()
    .any(|needle| k.contains(needle))
}

/// Keys that *look* secret-shaped but are documented, non-secret parts of the config contract.
///
/// `component.token` is the **UNS component token** (a lower-kebab identifier like
/// `opcua-adapter`), not a credential. Flagging it would make the rule cry wolf on every
/// correctly-written config — and a validator that cries wolf gets switched off, taking the
/// real findings with it. The exemption is by *pointer*, not by key name, so an actual
/// `token` under `component.global` is still caught.
fn is_known_safe(pointer: &str) -> bool {
    pointer == "/component/token"
}

/// `EC2009` — a config source must be legal for its platform.
///
/// `CONFIGMAP` exists only on Kubernetes; `GG_CONFIG` only on Greengrass. Everything else is
/// portable.
fn config_source_platform(
    config: &Value,
    platform: Option<Platform>,
    source: &str,
) -> Vec<Diagnostic> {
    let (Some(platform), Some(cs)) = (platform, declared_config_source(config)) else {
        return Vec::new();
    };
    if cs.is_legal_on(platform) {
        return Vec::new();
    }
    vec![
        Diagnostic::error(
            ec_diag::EC2009_CONFIG_SOURCE_PLATFORM,
            format!("config source {cs:?} is not available on platform {platform:?}"),
        )
        .with_file(source)
        .with_help("CONFIGMAP is Kubernetes-only and GG_CONFIG is Greengrass-only; FILE, ENV, SHADOW and CONFIG_COMPONENT are portable"),
    ]
}

/// `EC2001` — `--transport IPC` is valid only on GREENGRASS.
fn transport_platform(config: &Value, platform: Option<Platform>, source: &str) -> Vec<Diagnostic> {
    let Some(platform) = platform else {
        return Vec::new();
    };
    let transport = config
        .pointer("/messaging/transport")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase);
    if transport.as_deref() != Some("IPC") || platform == Platform::Greengrass {
        return Vec::new();
    }
    vec![
        Diagnostic::error(
            ec_diag::EC2001_IPC_REQUIRES_GREENGRASS,
            format!("transport IPC is only valid on GREENGRASS, not {platform:?}"),
        )
        .with_file(source)
        .with_pointer("/messaging/transport")
        .with_help("use MQTT on HOST and KUBERNETES; IPC is the Greengrass Nucleus channel"),
    ]
}

/// `EC2007` — a component bootstrapping from `CONFIG_COMPONENT` cannot depend recursively on
/// `CONFIG_COMPONENT` for its own bootstrap config.
///
/// The loop is real: the component cannot ask the config service where the config service is.
fn config_bootstrap_loop(config: &Value, source: &str) -> Vec<Diagnostic> {
    if declared_config_source(config) != Some(ConfigSource::ConfigComponent) {
        return Vec::new();
    }
    // The bootstrap section tells the component how to *reach* ConfigComponent. If that
    // section itself claims to come from ConfigComponent, the component can never start.
    let bootstrap = config
        .pointer("/component/configComponent/bootstrapSource")
        .or_else(|| config.pointer("/component/bootstrapSource"))
        .and_then(Value::as_str);
    if bootstrap.map(str::to_ascii_uppercase).as_deref() != Some("CONFIG_COMPONENT") {
        return Vec::new();
    }
    vec![
        Diagnostic::error(
            ec_diag::EC2007_CONFIG_BOOTSTRAP_LOOP,
            "a component sourcing config from CONFIG_COMPONENT cannot also bootstrap from CONFIG_COMPONENT"
                .to_string(),
        )
        .with_file(source)
        .with_help("bootstrap from FILE, ENV, or the platform's native source; the component must be able to find the config service before it can ask it anything"),
    ]
}

fn declared_config_source(config: &Value) -> Option<ConfigSource> {
    let raw = config
        .pointer("/component/configSource")
        .or_else(|| config.pointer("/config/source"))
        .and_then(Value::as_str)?;
    match raw.to_ascii_uppercase().as_str() {
        "FILE" => Some(ConfigSource::File),
        "ENV" => Some(ConfigSource::Env),
        "GG_CONFIG" => Some(ConfigSource::GgConfig),
        "SHADOW" => Some(ConfigSource::Shadow),
        "CONFIG_COMPONENT" => Some(ConfigSource::ConfigComponent),
        "CONFIGMAP" => Some(ConfigSource::ConfigMap),
        _ => None,
    }
}

/// Walk every scalar in a JSON document, yielding `(json pointer, key, value)`.
fn walk(value: &Value, pointer: &str, f: &mut impl FnMut(&str, &str, &Value)) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{pointer}/{}", escape(k));
                f(&child, k, v);
                walk(v, &child, f);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let child = format!("{pointer}/{i}");
                walk(v, &child, f);
            }
        }
        _ => {}
    }
}

/// RFC 6901 escaping.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_secret_value_is_rejected_but_a_reference_is_not() {
        let bad = json!({ "component": { "global": { "apiToken": "hunter2" } } });
        let r = check(&bad, None, "config.json");
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC2005_SECRET_VALUE);
        assert_eq!(
            r.diagnostics[0].locus.as_ref().unwrap().to_string(),
            "/component/global/apiToken"
        );

        let good = json!({ "component": { "global": { "apiToken": "secret://prod/northbound/api-token" } } });
        assert_eq!(check(&good, None, "config.json").error_count(), 0);
    }

    #[test]
    fn the_uns_component_token_is_not_mistaken_for_a_secret() {
        // `component.token` is the UNS component token (e.g. `opcua-adapter`), not a
        // credential. Flagging it would fire on every correctly-written config.
        let cfg = json!({ "component": { "token": "opcua-adapter" } });
        assert_eq!(check(&cfg, None, "c.json").error_count(), 0);

        // ...but a real token elsewhere is still caught. The exemption is by pointer, not by
        // key name, so it cannot be used as a hiding place.
        let sneaky = json!({ "component": { "global": { "token": "hunter2" } } });
        assert_eq!(check(&sneaky, None, "c.json").error_count(), 1);
    }

    #[test]
    fn secret_detection_looks_at_the_key_not_the_value() {
        // A non-secret key holding an innocuous string must not trip the rule...
        let ok = json!({ "component": { "global": { "endpoint": "hunter2" } } });
        assert_eq!(check(&ok, None, "c.json").error_count(), 0);
        // ...while several secret-shaped key spellings must.
        for key in ["password", "API_KEY", "privateKey", "dbCredential"] {
            let v = json!({ "component": { "global": { key: "literal" } } });
            assert_eq!(
                check(&v, None, "c.json").error_count(),
                1,
                "key `{key}` should be caught"
            );
        }
    }

    #[test]
    fn configmap_is_kubernetes_only() {
        let cfg = json!({ "component": { "configSource": "CONFIGMAP" } });
        assert_eq!(
            check(&cfg, Some(Platform::Kubernetes), "c.json").error_count(),
            0
        );

        let r = check(&cfg, Some(Platform::Host), "c.json");
        assert_eq!(r.error_count(), 1);
        assert_eq!(
            r.diagnostics[0].code,
            ec_diag::EC2009_CONFIG_SOURCE_PLATFORM
        );
    }

    #[test]
    fn gg_config_is_greengrass_only() {
        let cfg = json!({ "component": { "configSource": "GG_CONFIG" } });
        assert_eq!(
            check(&cfg, Some(Platform::Greengrass), "c.json").error_count(),
            0
        );
        assert_eq!(
            check(&cfg, Some(Platform::Kubernetes), "c.json").error_count(),
            1
        );
    }

    #[test]
    fn portable_sources_are_legal_everywhere() {
        for src in ["FILE", "ENV", "SHADOW", "CONFIG_COMPONENT"] {
            let cfg = json!({ "component": { "configSource": src } });
            for p in [Platform::Host, Platform::Kubernetes, Platform::Greengrass] {
                assert_eq!(
                    check(&cfg, Some(p), "c.json").error_count(),
                    0,
                    "{src} on {p:?}"
                );
            }
        }
    }

    #[test]
    fn ipc_outside_greengrass_is_rejected() {
        let cfg = json!({ "messaging": { "transport": "IPC" } });
        assert_eq!(
            check(&cfg, Some(Platform::Greengrass), "c.json").error_count(),
            0
        );

        let r = check(&cfg, Some(Platform::Host), "c.json");
        assert_eq!(r.error_count(), 1);
        assert_eq!(
            r.diagnostics[0].code,
            ec_diag::EC2001_IPC_REQUIRES_GREENGRASS
        );
    }

    #[test]
    fn the_config_component_bootstrap_loop_is_caught() {
        let looped = json!({
            "component": { "configSource": "CONFIG_COMPONENT", "bootstrapSource": "CONFIG_COMPONENT" }
        });
        let r = check(&looped, None, "c.json");
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC2007_CONFIG_BOOTSTRAP_LOOP);

        // Bootstrapping from FILE and then sourcing from ConfigComponent is the correct shape.
        let ok = json!({
            "component": { "configSource": "CONFIG_COMPONENT", "bootstrapSource": "FILE" }
        });
        assert_eq!(check(&ok, None, "c.json").error_count(), 0);
    }

    #[test]
    fn rules_needing_a_platform_are_skipped_rather_than_guessed() {
        // With no platform, EC2001/EC2009 cannot be decided. They must stay silent, not
        // invent a verdict.
        let cfg = json!({
            "messaging": { "transport": "IPC" },
            "component": { "configSource": "CONFIGMAP" }
        });
        assert_eq!(check(&cfg, None, "c.json").error_count(), 0);
    }

    #[test]
    fn pointers_are_rfc6901_escaped() {
        let cfg = json!({ "component": { "global": { "a/b": { "password": "x" } } } });
        let r = check(&cfg, None, "c.json");
        assert_eq!(r.error_count(), 1);
        assert!(
            r.diagnostics[0]
                .locus
                .as_ref()
                .unwrap()
                .to_string()
                .contains("a~1b"),
            "{:?}",
            r.diagnostics[0].locus
        );
    }
}
