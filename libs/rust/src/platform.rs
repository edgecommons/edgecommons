//! # Platform — the two-axis runtime model (platform × transport)
//!
//! **One-liner purpose**: The pure precedence resolver and platform auto-detector
//! (DESIGN-core §4 / §5), mirroring the canonical Java `com.breissinger.ggcommons.platform`.
//!
//! ## Overview
//! Two orthogonal runtime axes replace the legacy single `-m/--mode` token:
//! - [`Platform`] — the primary selector (`GREENGRASS | HOST | KUBERNETES | auto`). A
//!   platform is a named *profile*: a table of per-subsystem defaults (§3).
//! - [`Transport`] — the secondary axis (`IPC | MQTT`); defaults from the platform and is
//!   independently overridable, but constrained by the IPC lock ([`validate`], §4.1).
//!
//! [`resolve_profile`] maps parse-time inputs (explicit flags, then environment, then the
//! platform-profile defaults) to a single [`ResolvedProfile`] consumed by every subsystem
//! initializer.
//!
//! ## Phases
//! Phase 0 wired [`Platform::Greengrass`] and [`Platform::Host`], both defaulting their config
//! source to `GG_CONFIG` (a faithful re-expression of today's behavior; HOST does **not** flip
//! to `FILE`). Phase 1a wires [`Platform::Kubernetes`]: MQTT transport and the k8s-native
//! `CONFIGMAP` source (a mounted ConfigMap directory). The IPC × KUBERNETES rejection is
//! retained (the IPC lock, [`validate`]). The compile-time capability check (GREENGRASS requires
//! the `greengrass` cargo feature) lives at the transport-injection site
//! (`crate::init_messaging`), where the legacy silent `Ok(None)` used to be.
//!
//! ## Safety & Panics
//! All functions are pure (no I/O beyond the explicitly-injected filesystem probe used for
//! Kubernetes detection), which keeps the resolver and detector unit-testable in isolation.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{GgError, Result};

/// The deployment *platform* — the primary runtime axis (DESIGN-core §2/§3). A platform is
/// a named profile: a table of per-subsystem default providers/targets/sinks selected by
/// [`resolve_profile`]. Orthogonal to [`Transport`]; only messaging-transport is
/// platform-coupled (via the IPC lock, [`validate`]).
///
/// Phase 0 wired [`Self::Greengrass`] and [`Self::Host`] (a behavior-preserving re-expression
/// of today's two modes); Phase 1a wires [`Self::Kubernetes`] (MQTT transport + the `CONFIGMAP`
/// source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// On an AWS IoT Greengrass v2 Nucleus: IPC transport, Nucleus-managed config/identity.
    Greengrass,
    /// A plain host (Docker/bare host without a Nucleus): MQTT transport.
    Host,
    /// Kubernetes (declared for Phase 0; profile populated in Phase 1).
    Kubernetes,
}

impl Platform {
    /// Parse a `--platform` token (case-insensitive). `auto` yields `None` so the resolver
    /// auto-detects. Unknown values are an error.
    ///
    /// # Errors
    /// Returns [`GgError::Cli`] for an unrecognized platform token.
    pub fn parse(raw: &str) -> Result<Option<Platform>> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "AUTO" => Ok(None),
            "GREENGRASS" => Ok(Some(Platform::Greengrass)),
            "HOST" => Ok(Some(Platform::Host)),
            "KUBERNETES" => Ok(Some(Platform::Kubernetes)),
            other => Err(GgError::Cli(format!(
                "unknown platform '{other}'. Valid: GREENGRASS, HOST, KUBERNETES, auto."
            ))),
        }
    }
}

/// The messaging *transport* — the secondary runtime axis (DESIGN-core §2). Defaults from
/// the resolved [`Platform`] (GREENGRASS→IPC, HOST→MQTT) and is independently overridable,
/// but constrained: [`Self::Ipc`] is valid only on [`Platform::Greengrass`] (the Nucleus
/// provides the IPC socket). See [`validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Greengrass Nucleus IPC (domain socket). Requires [`Platform::Greengrass`].
    Ipc,
    /// Dual-MQTT (local broker + AWS IoT Core). The off-Nucleus transport.
    Mqtt,
}

impl Transport {
    /// Parse a `--transport` token (case-insensitive). Unknown values are an error.
    ///
    /// # Errors
    /// Returns [`GgError::Cli`] for an unrecognized transport token.
    pub fn parse(raw: &str) -> Result<Transport> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "IPC" => Ok(Transport::Ipc),
            "MQTT" => Ok(Transport::Mqtt),
            other => Err(GgError::Cli(format!(
                "unknown transport '{other}'. Valid: IPC, MQTT."
            ))),
        }
    }
}

/// A platform profile: the table of per-subsystem *defaults* for a [`Platform`]
/// (DESIGN-core §3). Pure data; the resolver consults it only for settings the caller did
/// not set explicitly.
///
/// Phase 0 carries only the two defaultable settings the resolver actually injects — the
/// default messaging [`transport`](Self::transport) and the default
/// [`config_source`](Self::config_source). Later phases append
/// metrics/logging/credentials/streaming/identity defaults as additional fields (additive;
/// no resolver change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformProfile {
    /// The default messaging transport for this platform.
    pub transport: Transport,
    /// The default `-c/--config` source token (e.g. `"GG_CONFIG"`, `"FILE"`) used when
    /// `-c` is omitted.
    pub config_source: &'static str,
    /// The default logging-format selector for this platform (FR-LOG-1 / FR-LOG-4), applied when
    /// the component config does not set `logging.rust_format`. `Some("json")` selects the
    /// structured stdout-JSON sink (the KUBERNETES default); `None` keeps the library default
    /// (console/text), so GREENGRASS and HOST are unchanged. The precedence is enforced by the
    /// logging configurator (explicit config ▸ this profile default ▸ library default — FR-RT-3).
    pub logging_format: Option<&'static str>,
    /// Whether the HTTP health/readiness endpoint (FR-HB-1) starts by default on this platform,
    /// applied when the component config omits `health.enabled`. `true` on KUBERNETES (probes are
    /// the orchestrator's contract); `false` on GREENGRASS/HOST (opt-in via `health.enabled=true`).
    /// Precedence (FR-RT-3): explicit `health.enabled` ▸ this profile default ▸ `false`.
    pub health_enabled: bool,
    /// The default `metricEmission.target` for this platform (FR-MET-1 / FR-RT-3), applied when the
    /// component config omits `metricEmission.target`. `Some("prometheus")` on KUBERNETES (the
    /// pull-based in-process registry served at `/metrics`); `None` on GREENGRASS/HOST so the
    /// library default (`log`) is unchanged off-Kubernetes. Precedence is enforced by the
    /// metric-target selector (explicit config ▸ this profile default ▸ `log` — FR-RT-3).
    ///
    /// NOTE (Rust feature gating): this is pure profile *data* and is `Some("prometheus")` on
    /// KUBERNETES regardless of cargo features. The *effective* k8s default only resolves to
    /// `prometheus` when the `metrics-prometheus` feature is compiled in; without it the selector
    /// gracefully falls back to `log` (with a warning). See
    /// [`crate::metrics::resolve_effective_target`].
    pub metric_target: Option<&'static str>,
    /// The default credentials-vault KEK custodian (`keyProvider.type`) for this platform
    /// (FR-CRED-6, precedence FR-RT-3), applied when a `credentials` section is present but
    /// `credentials.vault.keyProvider.type` is unspecified. `Some("env")` on KUBERNETES (the
    /// offline-capable software-KEK sourced from a mounted Secret); `None` on GREENGRASS/HOST so
    /// the library default (`file`) is unchanged off-Kubernetes. Precedence is enforced at the
    /// credentials init site (explicit `keyProvider.type` ▸ this profile default ▸ `file`).
    ///
    /// CRITICAL: this is *only* a default provider **type** — it never auto-enables credentials.
    /// Credentials stay opt-in (the vault opens only when a `credentials` config section exists).
    ///
    /// This is pure profile *data*; the field is present unconditionally even though the
    /// credentials subsystem is feature-gated (it is just a string token).
    pub credentials_key_provider: Option<&'static str>,
}

/// The output of [`resolve_profile`]: the fully resolved runtime settings that every
/// subsystem initializer consumes (DESIGN-core §4). Produced once, right after argument
/// parse and before messaging init, from parse-time inputs only (flags → env →
/// messaging-config payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    /// The resolved platform (after auto-detection / explicit flag).
    pub platform: Platform,
    /// The resolved messaging transport (validated against the platform).
    pub transport: Transport,
    /// The resolved `-c/--config` argument vector (explicit, else the profile default as a
    /// single-element vector).
    pub config_source: Vec<String>,
    /// The resolved IoT Thing name (identity), never empty.
    pub identity: String,
}

/// The parse-time inputs to the resolver. Any field may be `None`, meaning "not specified —
/// fall back to detection / the profile default".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolverInputs {
    /// Explicit `--platform` value, or `None` for `auto`.
    pub platform: Option<Platform>,
    /// Explicit `--transport` value, or `None` to derive from the platform.
    pub transport: Option<Transport>,
    /// Explicit `-c/--config` vector, or `None` when `-c` is omitted.
    pub config_args: Option<Vec<String>>,
    /// Explicit `-t/--thing` value, or `None`.
    pub thing: Option<String>,
}

/// Nucleus-injected env var pointing at the IPC domain socket (definitive GREENGRASS signal).
pub const ENV_GG_IPC_SOCKET: &str = "AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT";
/// Nucleus-injected component service-UID (definitive GREENGRASS signal).
pub const ENV_GG_SVCUID: &str = "SVCUID";
/// Greengrass-injected IoT Thing name (identity probe).
pub const ENV_THING_NAME: &str = "AWS_IOT_THING_NAME";
/// KUBERNETES Downward-API identity (primary k8s tier): the chart maps the
/// `ggcommons.io/thing-name` pod annotation (or an explicit value) into this env var.
pub const ENV_K8S_THING_NAME: &str = "GGCOMMONS_THING_NAME";
/// KUBERNETES Downward-API identity (secondary k8s tier): the pod name, projected via a
/// Downward-API `fieldRef` on `metadata.name`.
pub const ENV_K8S_POD_NAME: &str = "POD_NAME";
/// KUBERNETES Downward-API logging-correlation field (FR-LOG-3): the pod namespace, projected via
/// a Downward-API `fieldRef` on `metadata.namespace`. Best-effort; absent off-Kubernetes.
pub const ENV_K8S_POD_NAMESPACE: &str = "POD_NAMESPACE";
/// KUBERNETES Downward-API logging-correlation field (FR-LOG-3): the node name, projected via a
/// Downward-API `fieldRef` on `spec.nodeName`. Best-effort; absent off-Kubernetes.
pub const ENV_K8S_NODE_NAME: &str = "NODE_NAME";
/// Confirming (secondary) Kubernetes signal. The token file is the primary, definitive one.
pub const ENV_K8S_SERVICE_HOST: &str = "KUBERNETES_SERVICE_HOST";
/// Projected service-account token path: the primary, definitive Kubernetes signal.
pub const K8S_SA_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/// The library-default identity when no thing name is available (matches today's behavior).
pub const DEFAULT_IDENTITY: &str = "NOT_GREENGRASS";

/// The platform-profile for a platform (DESIGN-core §3). GREENGRASS and HOST deliberately
/// default the config source to `GG_CONFIG` to preserve current behavior; KUBERNETES (Phase 1a)
/// defaults to MQTT transport and the k8s-native `CONFIGMAP` source. Returns `None` only for a
/// platform with no profile (none today).
pub fn profile(platform: Platform) -> Option<PlatformProfile> {
    match platform {
        Platform::Greengrass => Some(PlatformProfile {
            transport: Transport::Ipc,
            config_source: "GG_CONFIG",
            logging_format: None,
            health_enabled: false,
            metric_target: None,
            credentials_key_provider: None,
        }),
        Platform::Host => Some(PlatformProfile {
            transport: Transport::Mqtt,
            config_source: "GG_CONFIG",
            logging_format: None,
            health_enabled: false,
            metric_target: None,
            credentials_key_provider: None,
        }),
        // Phase 1a: KUBERNETES is wired — MQTT transport (no Nucleus IPC) and the k8s-native
        // CONFIGMAP source (a mounted ConfigMap directory) as its default config source.
        // Phase 1c: its default logging format is the structured stdout-JSON sink (FR-LOG-1), the
        // HTTP health/readiness endpoint (FR-HB-1) is on by default, and the default metric target
        // is the pull-based `prometheus` registry served at `/metrics` (FR-MET-1).
        // Phase 1d: the default credentials-vault KEK custodian is `env` — the offline-capable
        // software-KEK read from a mounted Secret (FR-CRED-6), applied only when a credentials
        // section is configured without an explicit keyProvider.type.
        Platform::Kubernetes => Some(PlatformProfile {
            transport: Transport::Mqtt,
            config_source: "CONFIGMAP",
            logging_format: Some("json"),
            health_enabled: true,
            metric_target: Some("prometheus"),
            credentials_key_provider: Some("env"),
        }),
    }
}

/// Whether the HTTP health endpoint (FR-HB-1) is on by default for `platform` — the
/// platform-profile default consulted when the component config omits `health.enabled`. `true` on
/// KUBERNETES, `false` elsewhere (and for any platform without a profile). Mirrors the threading of
/// the logging-format default; the final decision (explicit config ▸ this default ▸ false) is made
/// in [`crate::GgCommonsBuilder::build`].
pub fn profile_health_enabled(platform: Platform) -> bool {
    profile(platform).map(|p| p.health_enabled).unwrap_or(false)
}

/// The platform-profile default `metricEmission.target` for `platform` — consulted when the
/// component config omits `metricEmission.target` (FR-MET-1, precedence FR-RT-3). `Some("prometheus")`
/// on KUBERNETES, `None` elsewhere (and for any platform without a profile). Mirrors the threading of
/// [`profile_health_enabled`] / the logging-format default; the final decision (explicit config ▸ this
/// default ▸ `log`, plus the Rust `metrics-prometheus` feature gate) is made in
/// [`crate::metrics::resolve_effective_target`].
pub fn profile_metric_target(platform: Platform) -> Option<&'static str> {
    profile(platform).and_then(|p| p.metric_target)
}

/// The platform-profile default credentials-vault KEK custodian (`keyProvider.type`) for
/// `platform` — consulted when a `credentials` section is present but `keyProvider.type` is
/// unspecified (FR-CRED-6, precedence FR-RT-3). `Some("env")` on KUBERNETES, `None` elsewhere
/// (and for any platform without a profile). Mirrors the threading of [`profile_metric_target`] /
/// [`profile_health_enabled`]; the final decision (explicit `keyProvider.type` ▸ this default ▸
/// `file`) is made at the credentials init site in [`crate::GgCommonsBuilder::build`] and applied
/// by [`crate::credentials::build_key_provider`].
///
/// CRITICAL: returning `Some("env")` here does **not** enable credentials — it only changes the
/// default provider type *when* credentials is already configured.
pub fn profile_credentials_key_provider(platform: Platform) -> Option<&'static str> {
    profile(platform).and_then(|p| p.credentials_key_provider)
}

/// The platforms that have a profile (GREENGRASS, HOST, KUBERNETES). Mirrors the Java `PROFILES`
/// map key set; used by tests and diagnostics.
pub fn profiled_platforms() -> [Platform; 3] {
    [Platform::Greengrass, Platform::Host, Platform::Kubernetes]
}

/// Resolves the runtime profile from parse-time inputs and the environment (DESIGN-core §4).
///
/// One rule governs every defaultable setting:
/// `resolve(setting) = explicit flag ▸ platform-profile default ▸ library default`.
///
/// # Errors
/// Returns [`GgError::Cli`] if the resolved platform has no profile, or the platform/transport
/// combination is illegal (the IPC lock, §4.1).
pub fn resolve_profile(inputs: ResolverInputs, env: &HashMap<String, String>) -> Result<ResolvedProfile> {
    let auto_detected = inputs.platform.is_none();
    let platform = match inputs.platform {
        Some(p) => p,
        None => detect_platform(env),
    };
    let basis = if auto_detected { "auto-detected" } else { "explicit --platform" };

    let profile = profile(platform).ok_or_else(|| {
        GgError::Cli(format!(
            "Platform {platform:?} is not supported in this build (no profile). Valid \
             platforms: GREENGRASS, HOST, KUBERNETES."
        ))
    })?;

    let transport = inputs.transport.unwrap_or(profile.transport);
    validate(platform, transport)?;

    let config_source = inputs
        .config_args
        .unwrap_or_else(|| vec![profile.config_source.to_string()]);

    let identity = resolve_identity(inputs.thing.as_deref(), platform, env);

    tracing::info!(
        platform = ?platform,
        basis,
        transport = ?transport,
        config_source = %config_source[0],
        identity = %identity,
        "platform/transport resolved"
    );

    Ok(ResolvedProfile {
        platform,
        transport,
        config_source,
        identity,
    })
}

/// Auto-detects the platform from the environment (DESIGN-core §5), using the real
/// filesystem to probe for the Kubernetes service-account token. First match wins; HOST is
/// the fallback.
pub fn detect_platform(env: &HashMap<String, String>) -> Platform {
    detect_platform_with(env, |p| Path::new(p).exists())
}

/// Auto-detection with an injectable filesystem probe (for tests). Signal order is
/// load-bearing: a containerized Nucleus component can set both Greengrass and Kubernetes
/// signals, and GREENGRASS must win (DESIGN-core §5).
pub fn detect_platform_with<F>(env: &HashMap<String, String>, file_exists: F) -> Platform
where
    F: Fn(&str) -> bool,
{
    // 1. GREENGRASS — Nucleus-injected signals exist nowhere else (definitive).
    if is_set(env, ENV_GG_IPC_SOCKET) || is_set(env, ENV_GG_SVCUID) {
        return Platform::Greengrass;
    }
    // 2. KUBERNETES — projected SA token (primary); service host (confirming/secondary).
    if file_exists(K8S_SA_TOKEN_PATH) || is_set(env, ENV_K8S_SERVICE_HOST) {
        return Platform::Kubernetes;
    }
    // 3. HOST — fallback.
    Platform::Host
}

/// Validates the platform/transport combination — the IPC lock (DESIGN-core §4.1). IPC is
/// valid only on a Greengrass Nucleus, which provides the IPC domain socket.
///
/// # Errors
/// Returns [`GgError::Cli`] if `transport == IPC && platform != GREENGRASS`.
pub fn validate(platform: Platform, transport: Transport) -> Result<()> {
    if transport == Transport::Ipc && platform != Platform::Greengrass {
        return Err(GgError::Cli(format!(
            "IPC transport requires --platform GREENGRASS (the Nucleus provides the IPC \
             socket); got platform={platform:?}"
        )));
    }
    Ok(())
}

/// Resolves the IoT Thing name / identity (DESIGN-core §6.2, FR-RT-7 / FR-CFG-6). Order:
/// 1. explicit `-t/--thing` (highest);
/// 2. **only when `platform == KUBERNETES`** the Downward-API env tier, in order:
///    [`ENV_K8S_THING_NAME`] (`GGCOMMONS_THING_NAME`) then [`ENV_K8S_POD_NAME`] (`POD_NAME`);
/// 3. the generic `AWS_IOT_THING_NAME` probe (GREENGRASS / platform-supplied);
/// 4. the library fallback [`DEFAULT_IDENTITY`].
///
/// The KUBERNETES tier (2) takes precedence over the generic probe (3) **only** on the
/// KUBERNETES platform; on every other platform behavior is unchanged (the `platform`
/// argument is now load-bearing). Empty env values are ignored at every tier. The resolved
/// value is not mangled here — it is sanitized later by template substitution
/// ([`crate::config::template`]) wherever it is interpolated into a path/topic.
pub fn resolve_identity(thing: Option<&str>, platform: Platform, env: &HashMap<String, String>) -> String {
    if let Some(t) = thing {
        return t.to_string();
    }
    // KUBERNETES Downward-API identity tier — precedes the generic probe only on k8s.
    if platform == Platform::Kubernetes {
        for key in [ENV_K8S_THING_NAME, ENV_K8S_POD_NAME] {
            if let Some(v) = env.get(key) {
                if !v.is_empty() {
                    return v.clone();
                }
            }
        }
    }
    if let Some(v) = env.get(ENV_THING_NAME) {
        if !v.is_empty() {
            return v.clone();
        }
    }
    DEFAULT_IDENTITY.to_string()
}

fn is_set(env: &HashMap<String, String>, key: &str) -> bool {
    env.get(key).is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // ---------- detect_platform ----------

    #[test]
    fn detect_greengrass_from_ipc_socket_env() {
        let e = env(&[(ENV_GG_IPC_SOCKET, "/run/gg.sock")]);
        assert_eq!(Platform::Greengrass, detect_platform_with(&e, |_| false));
    }

    #[test]
    fn detect_greengrass_from_svcuid_env() {
        let e = env(&[(ENV_GG_SVCUID, "abc123")]);
        assert_eq!(Platform::Greengrass, detect_platform_with(&e, |_| false));
    }

    #[test]
    fn detect_kubernetes_from_token_file() {
        let e = env(&[]);
        assert_eq!(
            Platform::Kubernetes,
            detect_platform_with(&e, |p| p == K8S_SA_TOKEN_PATH)
        );
    }

    #[test]
    fn detect_kubernetes_from_service_host_env() {
        let e = env(&[(ENV_K8S_SERVICE_HOST, "10.0.0.1")]);
        assert_eq!(Platform::Kubernetes, detect_platform_with(&e, |_| false));
    }

    #[test]
    fn detect_host_when_no_signals() {
        assert_eq!(Platform::Host, detect_platform_with(&env(&[]), |_| false));
    }

    #[test]
    fn greengrass_wins_over_kubernetes_when_both_signals_present() {
        // A containerized Nucleus component can set both; GREENGRASS must win.
        let e = env(&[(ENV_GG_SVCUID, "uid"), (ENV_K8S_SERVICE_HOST, "10.0.0.1")]);
        assert_eq!(Platform::Greengrass, detect_platform_with(&e, |_| true));
    }

    #[test]
    fn empty_env_value_is_not_a_signal() {
        let e = env(&[(ENV_GG_SVCUID, "")]);
        assert_eq!(Platform::Host, detect_platform_with(&e, |_| false));
    }

    // ---------- resolve_profile: profile defaults ----------

    #[test]
    fn resolve_greengrass_explicit_gives_ipc_and_gg_config() {
        let inputs = ResolverInputs {
            platform: Some(Platform::Greengrass),
            ..Default::default()
        };
        let r = resolve_profile(inputs, &env(&[])).unwrap();
        assert_eq!(Platform::Greengrass, r.platform);
        assert_eq!(Transport::Ipc, r.transport);
        assert_eq!(vec!["GG_CONFIG".to_string()], r.config_source);
        assert_eq!(DEFAULT_IDENTITY, r.identity);
    }

    #[test]
    fn resolve_host_explicit_gives_mqtt_and_gg_config_in_phase0() {
        // Phase 0 deliberately keeps HOST's default config source at GG_CONFIG (not FILE).
        let inputs = ResolverInputs {
            platform: Some(Platform::Host),
            ..Default::default()
        };
        let r = resolve_profile(inputs, &env(&[])).unwrap();
        assert_eq!(Platform::Host, r.platform);
        assert_eq!(Transport::Mqtt, r.transport);
        assert_eq!(vec!["GG_CONFIG".to_string()], r.config_source);
    }

    #[test]
    fn resolve_auto_with_no_signals_detects_host() {
        let r = resolve_profile(ResolverInputs::default(), &env(&[])).unwrap();
        assert_eq!(Platform::Host, r.platform);
        assert_eq!(Transport::Mqtt, r.transport);
    }

    #[test]
    fn resolve_auto_with_greengrass_env_detects_greengrass() {
        let r = resolve_profile(
            ResolverInputs::default(),
            &env(&[(ENV_GG_IPC_SOCKET, "/run/gg.sock")]),
        )
        .unwrap();
        assert_eq!(Platform::Greengrass, r.platform);
        assert_eq!(Transport::Ipc, r.transport);
    }

    // ---------- resolve_profile: explicit overrides ----------

    #[test]
    fn explicit_config_args_override_profile_default() {
        let inputs = ResolverInputs {
            platform: Some(Platform::Greengrass),
            config_args: Some(vec!["FILE".to_string(), "/etc/cfg.json".to_string()]),
            ..Default::default()
        };
        let r = resolve_profile(inputs, &env(&[])).unwrap();
        assert_eq!(vec!["FILE".to_string(), "/etc/cfg.json".to_string()], r.config_source);
    }

    #[test]
    fn explicit_transport_overrides_profile_default() {
        let inputs = ResolverInputs {
            platform: Some(Platform::Host),
            transport: Some(Transport::Mqtt),
            ..Default::default()
        };
        let r = resolve_profile(inputs, &env(&[])).unwrap();
        assert_eq!(Transport::Mqtt, r.transport);
    }

    #[test]
    fn explicit_thing_overrides_env_probe() {
        let inputs = ResolverInputs {
            platform: Some(Platform::Host),
            thing: Some("my-thing".to_string()),
            ..Default::default()
        };
        let r = resolve_profile(inputs, &env(&[(ENV_THING_NAME, "env-thing")])).unwrap();
        assert_eq!("my-thing", r.identity);
    }

    #[test]
    fn resolve_profile_uses_k8s_downward_api_identity_on_kubernetes() {
        // End-to-end through the resolver: KUBERNETES + POD_NAME yields the pod identity, even
        // with AWS_IOT_THING_NAME also set (the k8s tier precedes the generic probe).
        let inputs = ResolverInputs {
            platform: Some(Platform::Kubernetes),
            ..Default::default()
        };
        let r = resolve_profile(
            inputs,
            &env(&[(ENV_K8S_POD_NAME, "pod-7"), (ENV_THING_NAME, "iot-thing")]),
        )
        .unwrap();
        assert_eq!("pod-7", r.identity);
    }

    // ---------- resolve_profile: failures ----------

    #[test]
    fn resolve_kubernetes_gives_mqtt_and_configmap() {
        // Phase 1a: KUBERNETES resolves cleanly to MQTT transport + the CONFIGMAP default source.
        let inputs = ResolverInputs {
            platform: Some(Platform::Kubernetes),
            ..Default::default()
        };
        let r = resolve_profile(inputs, &env(&[])).unwrap();
        assert_eq!(Platform::Kubernetes, r.platform);
        assert_eq!(Transport::Mqtt, r.transport);
        assert_eq!(vec!["CONFIGMAP".to_string()], r.config_source);
    }

    #[test]
    fn resolve_auto_with_k8s_token_detects_kubernetes() {
        // A SA-token pod auto-detects to KUBERNETES and resolves to MQTT + CONFIGMAP.
        let r = resolve_profile(
            ResolverInputs::default(),
            &env(&[(ENV_K8S_SERVICE_HOST, "10.0.0.1")]),
        )
        .unwrap();
        assert_eq!(Platform::Kubernetes, r.platform);
        assert_eq!(Transport::Mqtt, r.transport);
        assert_eq!(vec!["CONFIGMAP".to_string()], r.config_source);
    }

    #[test]
    fn resolve_ipc_on_kubernetes_fails_the_ipc_lock() {
        let inputs = ResolverInputs {
            platform: Some(Platform::Kubernetes),
            transport: Some(Transport::Ipc),
            ..Default::default()
        };
        let err = resolve_profile(inputs, &env(&[])).unwrap_err();
        assert!(err.to_string().contains("IPC transport requires --platform GREENGRASS"));
    }

    #[test]
    fn resolve_ipc_on_host_fails_the_ipc_lock() {
        let inputs = ResolverInputs {
            platform: Some(Platform::Host),
            transport: Some(Transport::Ipc),
            ..Default::default()
        };
        let err = resolve_profile(inputs, &env(&[])).unwrap_err();
        assert!(err.to_string().contains("IPC transport requires --platform GREENGRASS"));
    }

    // ---------- validate ----------

    #[test]
    fn validate_rejects_ipc_on_non_greengrass() {
        assert!(validate(Platform::Host, Transport::Ipc).is_err());
        assert!(validate(Platform::Kubernetes, Transport::Ipc).is_err());
    }

    #[test]
    fn validate_accepts_legal_combos() {
        assert!(validate(Platform::Greengrass, Transport::Ipc).is_ok());
        assert!(validate(Platform::Host, Transport::Mqtt).is_ok());
        assert!(validate(Platform::Kubernetes, Transport::Mqtt).is_ok());
    }

    // ---------- resolve_identity ----------

    #[test]
    fn resolve_identity_prefers_explicit_thing() {
        assert_eq!("t1", resolve_identity(Some("t1"), Platform::Greengrass, &env(&[])));
    }

    #[test]
    fn resolve_identity_falls_back_to_env() {
        assert_eq!(
            "env-thing",
            resolve_identity(None, Platform::Host, &env(&[(ENV_THING_NAME, "env-thing")]))
        );
    }

    #[test]
    fn resolve_identity_defaults_when_nothing_available() {
        assert_eq!(DEFAULT_IDENTITY, resolve_identity(None, Platform::Host, &env(&[])));
    }

    #[test]
    fn resolve_identity_ignores_empty_env() {
        assert_eq!(
            DEFAULT_IDENTITY,
            resolve_identity(None, Platform::Host, &env(&[(ENV_THING_NAME, "")]))
        );
    }

    // ---------- resolve_identity: KUBERNETES Downward-API tier (FR-RT-7 / FR-CFG-6) ----------

    #[test]
    fn k8s_identity_from_ggcommons_thing_name() {
        let r = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[(ENV_K8S_THING_NAME, "annotated-thing")]),
        );
        assert_eq!("annotated-thing", r);
    }

    #[test]
    fn k8s_identity_from_pod_name() {
        let r = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[(ENV_K8S_POD_NAME, "my-pod-abc123")]),
        );
        assert_eq!("my-pod-abc123", r);
    }

    #[test]
    fn k8s_ggcommons_thing_name_wins_over_pod_name() {
        // The annotation/explicit value (GGCOMMONS_THING_NAME) precedes POD_NAME.
        let r = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[(ENV_K8S_THING_NAME, "annotated"), (ENV_K8S_POD_NAME, "pod-xyz")]),
        );
        assert_eq!("annotated", r);
    }

    #[test]
    fn k8s_identity_tier_precedes_aws_iot_thing_name() {
        // On KUBERNETES, the Downward-API tier takes precedence over the generic probe.
        let r = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[(ENV_K8S_POD_NAME, "pod-1"), (ENV_THING_NAME, "iot-thing")]),
        );
        assert_eq!("pod-1", r);
    }

    #[test]
    fn k8s_falls_through_to_aws_iot_thing_name_when_no_downward_api() {
        // No GGCOMMONS_THING_NAME / POD_NAME → fall through to the generic probe even on k8s.
        let r = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[(ENV_THING_NAME, "iot-thing")]),
        );
        assert_eq!("iot-thing", r);
    }

    #[test]
    fn k8s_ignores_empty_downward_api_values() {
        // Empty Downward-API values are not signals; fall through to the generic probe.
        let r = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[
                (ENV_K8S_THING_NAME, ""),
                (ENV_K8S_POD_NAME, ""),
                (ENV_THING_NAME, "iot-thing"),
            ]),
        );
        assert_eq!("iot-thing", r);
    }

    #[test]
    fn k8s_identity_defaults_when_nothing_available() {
        assert_eq!(
            DEFAULT_IDENTITY,
            resolve_identity(None, Platform::Kubernetes, &env(&[]))
        );
    }

    #[test]
    fn explicit_thing_wins_over_k8s_downward_api() {
        // -t/--thing is highest precedence, even on KUBERNETES with Downward-API vars set.
        let r = resolve_identity(
            Some("cli-thing"),
            Platform::Kubernetes,
            &env(&[(ENV_K8S_THING_NAME, "annotated"), (ENV_K8S_POD_NAME, "pod-1")]),
        );
        assert_eq!("cli-thing", r);
    }

    #[test]
    fn non_k8s_platforms_ignore_the_k8s_identity_tier() {
        // The Downward-API tier is gated on platform==KUBERNETES; HOST/GREENGRASS ignore it and
        // use only the generic AWS_IOT_THING_NAME probe.
        let e = env(&[(ENV_K8S_THING_NAME, "annotated"), (ENV_K8S_POD_NAME, "pod-1")]);
        assert_eq!(DEFAULT_IDENTITY, resolve_identity(None, Platform::Host, &e));
        assert_eq!(DEFAULT_IDENTITY, resolve_identity(None, Platform::Greengrass, &e));
        // ... and the generic probe still wins for them.
        let e2 = env(&[(ENV_K8S_POD_NAME, "pod-1"), (ENV_THING_NAME, "iot-thing")]);
        assert_eq!("iot-thing", resolve_identity(None, Platform::Host, &e2));
    }

    #[test]
    fn resolved_k8s_identity_is_sanitized_when_interpolated() {
        // FR-RT-7: the resolved identity is not mangled by the resolver, but a hostile pod name
        // MUST still pass the existing template-variable sanitization wherever it is interpolated
        // into a path/topic (no path traversal, no MQTT wildcards).
        use crate::config::model::Config;
        use crate::config::template::resolve;

        let identity = resolve_identity(
            None,
            Platform::Kubernetes,
            &env(&[(ENV_K8S_POD_NAME, "../../etc/passwd")]),
        );
        assert_eq!("../../etc/passwd", identity, "resolver returns the raw value");

        let cfg = Config::from_value("com.example.C", &identity, serde_json::json!({})).unwrap();
        assert_eq!(resolve(&cfg, "/logs/{ThingName}.log"), "/logs/____etc_passwd.log");
    }

    // ---------- profiles + enums ----------

    #[test]
    fn profiles_contain_greengrass_host_and_kubernetes() {
        assert_eq!(3, profiled_platforms().len());
        assert!(profile(Platform::Greengrass).is_some());
        assert!(profile(Platform::Host).is_some());
        assert!(profile(Platform::Kubernetes).is_some());
    }

    #[test]
    fn kubernetes_profile_exposes_mqtt_and_configmap() {
        let p = profile(Platform::Kubernetes).unwrap();
        assert_eq!(Transport::Mqtt, p.transport);
        assert_eq!("CONFIGMAP", p.config_source);
    }

    #[test]
    fn kubernetes_profile_defaults_logging_to_json() {
        // FR-LOG-1: the KUBERNETES profile's default logging format is the stdout-JSON sink.
        assert_eq!(Some("json"), profile(Platform::Kubernetes).unwrap().logging_format);
    }

    #[test]
    fn greengrass_and_host_profiles_keep_library_default_logging() {
        // GREENGRASS/HOST carry no profile logging-format default (None) so the library default
        // (console/text) is unchanged off-Kubernetes.
        assert_eq!(None, profile(Platform::Greengrass).unwrap().logging_format);
        assert_eq!(None, profile(Platform::Host).unwrap().logging_format);
    }

    #[test]
    fn kubernetes_profile_enables_health_by_default() {
        // FR-HB-1: the health endpoint is on by default on KUBERNETES and off on GREENGRASS/HOST.
        assert!(profile(Platform::Kubernetes).unwrap().health_enabled);
        assert!(!profile(Platform::Greengrass).unwrap().health_enabled);
        assert!(!profile(Platform::Host).unwrap().health_enabled);
    }

    #[test]
    fn profile_health_enabled_helper_matches_profiles() {
        assert!(profile_health_enabled(Platform::Kubernetes));
        assert!(!profile_health_enabled(Platform::Greengrass));
        assert!(!profile_health_enabled(Platform::Host));
    }

    #[test]
    fn kubernetes_profile_defaults_metric_target_to_prometheus() {
        // FR-MET-1: the KUBERNETES profile's default metric target is the pull-based prometheus
        // registry; GREENGRASS/HOST carry no profile default (None) so the library default (log)
        // is unchanged off-Kubernetes. (This is pure profile data; the Rust feature gate is applied
        // by the metric-target selector, not here.)
        assert_eq!(Some("prometheus"), profile(Platform::Kubernetes).unwrap().metric_target);
        assert_eq!(None, profile(Platform::Greengrass).unwrap().metric_target);
        assert_eq!(None, profile(Platform::Host).unwrap().metric_target);
    }

    #[test]
    fn profile_metric_target_helper_matches_profiles() {
        assert_eq!(Some("prometheus"), profile_metric_target(Platform::Kubernetes));
        assert_eq!(None, profile_metric_target(Platform::Greengrass));
        assert_eq!(None, profile_metric_target(Platform::Host));
    }

    #[test]
    fn kubernetes_profile_defaults_credentials_key_provider_to_env() {
        // FR-CRED-6: the KUBERNETES profile's default vault KEK custodian is `env` (the offline
        // software-KEK from a mounted Secret); GREENGRASS/HOST carry no profile default (None) so
        // the library default (`file`) is unchanged off-Kubernetes. Pure profile data — it does not
        // enable credentials, only changes the default provider type when credentials is configured.
        assert_eq!(Some("env"), profile(Platform::Kubernetes).unwrap().credentials_key_provider);
        assert_eq!(None, profile(Platform::Greengrass).unwrap().credentials_key_provider);
        assert_eq!(None, profile(Platform::Host).unwrap().credentials_key_provider);
    }

    #[test]
    fn profile_credentials_key_provider_helper_matches_profiles() {
        assert_eq!(Some("env"), profile_credentials_key_provider(Platform::Kubernetes));
        assert_eq!(None, profile_credentials_key_provider(Platform::Greengrass));
        assert_eq!(None, profile_credentials_key_provider(Platform::Host));
    }

    #[test]
    fn profile_exposes_its_fields() {
        let p = profile(Platform::Greengrass).unwrap();
        assert_eq!(Transport::Ipc, p.transport);
        assert_eq!("GG_CONFIG", p.config_source);
    }

    // ---------- token parsing ----------

    #[test]
    fn platform_parse_handles_auto_and_known_and_unknown() {
        assert_eq!(None, Platform::parse("auto").unwrap());
        assert_eq!(Some(Platform::Greengrass), Platform::parse("greengrass").unwrap());
        assert_eq!(Some(Platform::Kubernetes), Platform::parse("KUBERNETES").unwrap());
        assert!(Platform::parse("bogus").is_err());
    }

    #[test]
    fn transport_parse_handles_known_and_unknown() {
        assert_eq!(Transport::Ipc, Transport::parse("ipc").unwrap());
        assert_eq!(Transport::Mqtt, Transport::parse("MQTT").unwrap());
        assert!(Transport::parse("bogus").is_err());
    }
}
