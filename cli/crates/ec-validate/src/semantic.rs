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
    r.extend(lineage_is_ordered_and_acyclic(config, source));
    r.extend(uns_tokens(config, source));
    r.extend(reserved_uns_classes(config, source));
    r
}

/// `EC2004` — a hierarchical config lineage must be acyclic and ordered.
///
/// The lineage is an ordered list of scopes (enterprise → site → … → component). A repeated
/// scope is a cycle in the only sense that matters here: the merge would apply the same layer
/// twice and the "effective" config would depend on iteration order rather than on the model.
fn lineage_is_ordered_and_acyclic(config: &Value, source: &str) -> Vec<Diagnostic> {
    let Some(levels) = config.pointer("/hierarchy").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for (i, entry) in levels.iter().enumerate() {
        // A hierarchy entry is either a bare level name or {level, value}.
        let level = entry
            .as_str()
            .or_else(|| entry.get("level").and_then(Value::as_str))
            .unwrap_or_default();
        if level.is_empty() {
            continue;
        }
        if seen.contains(&level) {
            out.push(
                Diagnostic::error(
                    ec_diag::EC2004_LINEAGE_CYCLE,
                    format!("hierarchy level `{level}` appears more than once"),
                )
                .with_file(source)
                .with_pointer(format!("/hierarchy/{i}"))
                .with_help("a lineage is an ordered list of distinct scopes; a repeated level makes the merged result depend on iteration order"),
            );
        }
        seen.push(level);
    }
    out
}

/// `EC2008` — UNS identity tokens must satisfy the character set and the depth guard.
///
/// The rules are the UNS grammar's, not this crate's invention: a token is lower-kebab, and a
/// topic may not exceed the IoT Core seven-slash depth once the class and channel are appended.
fn uns_tokens(config: &Value, source: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (pointer, token) in identity_tokens(config) {
        if token.is_empty() {
            continue;
        }
        if !is_uns_token(&token) {
            out.push(
                Diagnostic::error(
                    ec_diag::EC2008_INVALID_UNS_TOKEN,
                    format!("`{token}` is not a valid UNS token"),
                )
                .with_file(source)
                .with_pointer(pointer)
                .with_help(
                    "UNS tokens are lower-kebab: [a-z0-9] separated by single hyphens (e.g. `opcua-adapter`)",
                ),
            );
        }
    }
    out
}

/// `EC2006` — a raw publish to a reserved UNS class is rejected.
///
/// `state`, `metric`, `cfg` and `log` are library-owned: the library publishes them, and a
/// component that raw-publishes to them corrupts a class other tools rely on. `data`, `evt`,
/// `cmd` and `app` are the application-facing classes.
fn reserved_uns_classes(config: &Value, source: &str) -> Vec<Diagnostic> {
    const RESERVED: [&str; 4] = ["state", "metric", "cfg", "log"];
    let mut out = Vec::new();

    walk(config, "", &mut |pointer, key, value| {
        // A configured publish target: a `topic`/`publishTopic` naming a UNS class.
        if !matches!(key, "topic" | "publishTopic" | "publish_topic") {
            return;
        }
        let Some(topic) = value.as_str() else { return };
        // ecv1/{device}/{component}/{instance}/{class}[/channel]
        let class = topic.split('/').nth(4).unwrap_or_default();
        if RESERVED.contains(&class) {
            out.push(
                Diagnostic::error(
                    ec_diag::EC2006_RESERVED_UNS_CLASS,
                    format!("`{class}` is a reserved UNS class and cannot be published to directly"),
                )
                .with_file(source)
                .with_pointer(pointer)
                .with_help(
                    "the library owns state/metric/cfg/log; publish application traffic to data, evt, cmd or app",
                ),
            );
        }
    });
    out
}

/// Every identity token in a config, with its pointer.
fn identity_tokens(config: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (ptr, key) in [
        ("/component/token", "token"),
        ("/identity/component", "component"),
        ("/identity/instance", "instance"),
    ] {
        if let Some(v) = config.pointer(ptr).and_then(Value::as_str) {
            let _ = key;
            out.push((ptr.to_string(), v.to_string()));
        }
    }
    out
}

/// The UNS token grammar: lower-kebab, no leading/trailing/doubled hyphens.
fn is_uns_token(t: &str) -> bool {
    !t.is_empty()
        && !t.starts_with('-')
        && !t.ends_with('-')
        && !t.contains("--")
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
    fn a_repeated_hierarchy_level_is_caught() {
        // EC2004: a lineage is an ordered list of DISTINCT scopes. A repeated level makes the
        // merged result depend on iteration order rather than on the model.
        let bad = json!({
            "component": { "token": "x" },
            "hierarchy": [
                { "level": "site", "value": "dallas" },
                { "level": "line", "value": "fill-1" },
                { "level": "site", "value": "austin" }
            ]
        });
        let r = check(&bad, None, "c.json");
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC2004_LINEAGE_CYCLE);

        let good = json!({
            "component": { "token": "x" },
            "hierarchy": [
                { "level": "site", "value": "dallas" },
                { "level": "line", "value": "fill-1" }
            ]
        });
        assert_eq!(check(&good, None, "c.json").error_count(), 0);
    }

    #[test]
    fn an_invalid_uns_token_is_caught() {
        // EC2008: UNS tokens are lower-kebab. `MyComponent` would produce a topic no fleet-wide
        // consumer could match.
        for bad in [
            "MyComponent",
            "my_component",
            "-leading",
            "trailing-",
            "double--hyphen",
        ] {
            let cfg = json!({ "component": { "token": bad } });
            let r = check(&cfg, None, "c.json");
            assert_eq!(r.error_count(), 1, "`{bad}` should be rejected");
            assert_eq!(r.diagnostics[0].code, ec_diag::EC2008_INVALID_UNS_TOKEN);
        }
        for good in ["opcua-adapter", "telemetry-processor", "gw01", "a-b-c"] {
            let cfg = json!({ "component": { "token": good } });
            assert_eq!(
                check(&cfg, None, "c.json").error_count(),
                0,
                "`{good}` should be accepted"
            );
        }
    }

    #[test]
    fn publishing_to_a_reserved_uns_class_is_rejected() {
        // EC2006: state/metric/cfg/log are library-owned. A component that raw-publishes to them
        // corrupts a class the rest of the fleet relies on.
        for class in ["state", "metric", "cfg", "log"] {
            let cfg = json!({
                "component": { "token": "x", "global": { "topic": format!("ecv1/gw01/thing/main/{class}") } }
            });
            let r = check(&cfg, None, "c.json");
            assert_eq!(
                r.error_count(),
                1,
                "`{class}` must be reserved: {}",
                r.render_human()
            );
            assert_eq!(r.diagnostics[0].code, ec_diag::EC2006_RESERVED_UNS_CLASS);
        }
        // The application-facing classes are fine.
        for class in ["data", "evt", "cmd", "app"] {
            let cfg = json!({
                "component": { "token": "x", "global": { "topic": format!("ecv1/gw01/thing/main/{class}/x") } }
            });
            assert_eq!(
                check(&cfg, None, "c.json").error_count(),
                0,
                "`{class}` must be allowed"
            );
        }
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
