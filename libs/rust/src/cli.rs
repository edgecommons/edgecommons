//! # CLI
//!
//! **One-liner purpose**: Parse the standard command-line contract shared verbatim
//! across the Java (canonical), Python, and TypeScript libraries, then run the
//! platform/transport resolver (DESIGN-core §4 / §6).
//!
//! ## Overview
//! The contract (post Phase-0; the legacy single `-m/--mode` axis is removed):
//! - `--platform GREENGRASS|HOST|KUBERNETES|auto` — primary axis (default `auto`).
//! - `--transport IPC|MQTT [messaging_config_path]` — secondary axis (default: from
//!   the platform; validated by the IPC lock).
//! - `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT`
//!   (default: from the resolved platform profile).
//! - `-t/--thing <name>` — IoT Thing name (takes the **full** string value).
//!
//! ## Semantics & Architecture
//! - Pure clap parsing, then [`crate::platform::resolve_profile`] (which reads the
//!   process environment for `--platform auto` detection and the identity probe).
//! - Invariants: `-t` is never truncated (guards a historical bug); the removed
//!   `-m/--mode` flag is rejected with guidance; the MQTT messaging-config path is
//!   validated when the provider is actually built, not at parse time.
//! - Error handling: parse/resolve failures surface as [`crate::error::EdgeCommonsError::Cli`].
//!
//! ## Usage Example
//! ```
//! use edgecommons::cli::parse_from;
//! use edgecommons::platform::{Platform, Transport};
//!
//! let args = parse_from([
//!     "prog", "--platform", "HOST", "--transport", "MQTT", "msg.json", "-t", "thing-1",
//! ]).unwrap();
//! assert_eq!(args.platform, Platform::Host);
//! assert_eq!(args.transport, Transport::Mqtt);
//! assert_eq!(args.thing.as_deref(), Some("thing-1"));
//! ```
//!
//! ## Related Modules
//! - [`crate::config::source`] consumes [`ConfigSourceSpec`]; [`crate`] consumes [`ParsedArgs`].

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, Command};

use crate::error::{EdgeCommonsError, Result};
use crate::platform::{self, Platform, ResolverInputs, Transport};

/// Configuration source selected by `-c/--config` (or the platform-profile default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSourceSpec {
    /// `FILE [path]` — JSON file (default `config.json`).
    File { path: PathBuf },
    /// `CONFIGMAP [mountDir] [key]` — Kubernetes-native: a mounted ConfigMap directory
    /// (defaults: dir `/etc/edgecommons`, key `config.json`). The default source on KUBERNETES.
    ConfigMap {
        mount_dir: Option<PathBuf>,
        key: Option<String>,
    },
    /// `ENV [var]` — JSON in an environment variable (default `CONFIG`).
    Env { var: String },
    /// `GG_CONFIG [component] [key]` — Greengrass deployment config (default key `ComponentConfig`).
    Greengrass {
        component: Option<String>,
        key: String,
    },
    /// `SHADOW [name]` — IoT named device shadow.
    Shadow { name: Option<String> },
    /// `CONFIG_COMPONENT` — dedicated configuration component (over messaging).
    ConfigComponent,
}

/// Parsed standard arguments, after the platform/transport resolver has run
/// (DESIGN-core §4). Carries the two resolved runtime axes plus the resolved config
/// source, identity, and the optional MQTT messaging-config path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    /// The resolved deployment platform (the primary runtime axis).
    pub platform: Platform,
    /// The resolved messaging transport (the secondary axis; derived from the platform unless overridden).
    pub transport: Transport,
    /// The resolved config source (explicit `-c`, else the platform-profile default).
    pub config: ConfigSourceSpec,
    /// The explicit `-t/--thing` flag value, verbatim (`None` if not supplied).
    pub thing: Option<String>,
    /// The resolved IoT Thing name / identity (explicit `-t` ▸ env probe ▸ library fallback).
    pub identity: String,
    /// The MQTT messaging-config file path. Either the explicit `--transport MQTT <path>`
    /// payload, or — under CONFIGMAP + MQTT with no explicit path (FR-MSG-1) — the resolved
    /// ConfigMap file (mount dir + key), so a single mounted `config.json` carrying a
    /// `.messaging` section doubles as the messaging config and the component config.
    pub messaging_config_path: Option<PathBuf>,
}

const DEFAULT_CONFIG_FILE: &str = "config.json";
const DEFAULT_ENV_VAR: &str = "CONFIG";
const DEFAULT_GG_CONFIG_KEY: &str = "ComponentConfig";

/// Build the base `clap` command. Application-specific options can be merged onto
/// this in a later phase (mirrors the Java `appOptions` merge).
pub fn command() -> Command {
    Command::new("edgecommons")
        .disable_help_flag(false)
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .num_args(1..=3)
                .value_parser(clap::value_parser!(String))
                .value_name("SOURCE")
                .help(
                    "Config source: FILE|CONFIGMAP|ENV|GG_CONFIG|SHADOW|CONFIG_COMPONENT [args...] \
                       (default: from the resolved platform profile)",
                ),
        )
        .arg(
            Arg::new("platform")
                .long("platform")
                .num_args(1)
                .value_parser(clap::value_parser!(String))
                .value_name("PLATFORM")
                .help("Deployment platform: GREENGRASS | HOST | KUBERNETES | auto (default auto)"),
        )
        .arg(
            Arg::new("transport")
                .long("transport")
                .num_args(1..=2)
                .value_parser(clap::value_parser!(String))
                .value_name("TRANSPORT")
                .help(
                    "Messaging transport: IPC | MQTT <messaging_config_path> \
                       (default: derived from the platform)",
                ),
        )
        .arg(
            Arg::new("thing")
                .short('t')
                .long("thing")
                .num_args(1)
                .value_parser(clap::value_parser!(String))
                .value_name("NAME")
                .help("IoT Thing name"),
        )
}

/// Parse the standard arguments from an argv-style iterator and resolve the runtime
/// profile (DESIGN-core §4).
///
/// # Purpose
/// Turn raw process arguments into a typed [`ParsedArgs`] — the two resolved runtime
/// axes (platform/transport), the config source, the identity, and the optional MQTT
/// messaging-config path — enforcing the cross-language CLI contract.
///
/// # Pre-conditions
/// - The first element is the program name (it is skipped, per `clap` convention).
///
/// # Post-conditions
/// - `platform`/`transport`/`config`/`identity` are fully resolved (profile defaults
///   applied, IPC lock validated); `thing` reflects `-t` verbatim.
///
/// # Errors
/// | Error Variant | Condition | Recovery |
/// |---------------|-----------|----------|
/// | `EdgeCommonsError::Cli` | Unknown flag, the removed `-m`/`--mode`, an unknown source/platform/transport, or an illegal platform/transport combo | Fix the arguments |
///
/// # Examples
/// ```
/// # use edgecommons::cli::parse_from;
/// let a = parse_from(["prog", "--platform", "HOST", "-c", "FILE", "config.json"]).unwrap();
/// assert!(a.thing.is_none());
/// ```
pub fn parse_from<I, T>(args: I) -> Result<ParsedArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // The legacy single-axis -m/--mode token is removed (DESIGN-core §6.1 / FR-RT-1). Reject
    // it explicitly with guidance rather than letting clap report an opaque "unexpected
    // argument" error.
    reject_legacy_mode_flag(&argv)?;

    let matches = command()
        .try_get_matches_from(&argv)
        .map_err(|e| EdgeCommonsError::Cli(e.to_string()))?;

    // Explicit -c/--config args, or None (the resolver fills the platform-profile default).
    let config_args: Option<Vec<String>> = matches
        .get_many::<String>("config")
        .map(|v| v.cloned().collect());

    let platform_flag: Option<Platform> = match matches.get_one::<String>("platform") {
        Some(raw) => Platform::parse(raw)?,
        None => None,
    };

    // --transport [IPC|MQTT] <optional messaging-config path>. The optional second value (the
    // MQTT messaging-config path) is stashed for the provider builder.
    let (transport_flag, messaging_config_path): (Option<Transport>, Option<PathBuf>) =
        match matches.get_many::<String>("transport") {
            Some(values) => {
                let vals: Vec<String> = values.cloned().collect();
                let t = Transport::parse(&vals[0])?;
                let path = vals.get(1).map(PathBuf::from);
                (Some(t), path)
            }
            None => (None, None),
        };

    let thing_flag = matches.get_one::<String>("thing").cloned();
    let inputs = ResolverInputs {
        platform: platform_flag,
        transport: transport_flag,
        config_args,
        thing: thing_flag.clone(),
    };

    // Resolve the two runtime axes + the default config provider + identity from parse-time
    // inputs only (DESIGN-core §4 / §4.2). Validation failures (e.g. the IPC lock) propagate.
    let env: HashMap<String, String> = std::env::vars().collect();
    let resolved = platform::resolve_profile(inputs, &env)?;

    let config = parse_config_source(&resolved.config_source)?;

    // FR-MSG-1: under CONFIGMAP + MQTT with no explicit `--transport MQTT <path>`, the single
    // mounted ConfigMap file doubles as the messaging config (it carries a `.messaging` section)
    // and the component config. Default the messaging-config path to the resolved ConfigMap file
    // (mount dir + key; default `/etc/edgecommons/config.json`) so messaging init gets `Some(path)`
    // without a positional path. Computed from parse-time inputs only (the resolved transport +
    // config source) — never by reading the ConfigMap via the config source first (that runs after
    // messaging init). The explicit-path behavior is unchanged; HOST is unaffected (it defaults to
    // FILE, not CONFIGMAP, so no default is synthesized and MQTT still requires a path).
    let messaging_config_path =
        default_messaging_config_path(messaging_config_path, resolved.transport, &config);

    Ok(ParsedArgs {
        platform: resolved.platform,
        transport: resolved.transport,
        config,
        thing: thing_flag,
        identity: resolved.identity,
        messaging_config_path,
    })
}

/// Apply the FR-MSG-1 default: when no explicit MQTT messaging-config path was given, the
/// transport is MQTT, and the resolved config source is CONFIGMAP, default the path to the
/// resolved ConfigMap file (the same mount dir + key the CONFIGMAP source resolves from).
/// Otherwise the explicit value (or `None`) passes through unchanged.
fn default_messaging_config_path(
    explicit: Option<PathBuf>,
    transport: Transport,
    config: &ConfigSourceSpec,
) -> Option<PathBuf> {
    use crate::config::source::configmap::{DEFAULT_KEY, DEFAULT_MOUNT_DIR};
    match (explicit, transport, config) {
        (None, Transport::Mqtt, ConfigSourceSpec::ConfigMap { mount_dir, key }) => {
            let dir = mount_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_MOUNT_DIR));
            let file = key.clone().unwrap_or_else(|| DEFAULT_KEY.to_string());
            Some(dir.join(file))
        }
        (explicit, _, _) => explicit,
    }
}

/// Rejects the removed `-m`/`--mode` flag with guidance to the new axes (DESIGN-core §6.1).
///
/// Mirrors Java's `rejectLegacyModeFlag`: the thrown error is the Rust analog of Java's
/// `IllegalArgumentException`, surfaced by `build()` as a [`EdgeCommonsError::Cli`].
fn reject_legacy_mode_flag(argv: &[OsString]) -> Result<()> {
    for arg in argv {
        if arg == "-m" || arg == "--mode" {
            return Err(EdgeCommonsError::Cli(
                "The -m/--mode flag has been removed. Use --platform GREENGRASS|HOST|KUBERNETES \
                 and --transport IPC|MQTT instead (e.g. '-m STANDALONE <path>' becomes \
                 '--platform HOST --transport MQTT <path>')."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn parse_config_source(args: &[String]) -> Result<ConfigSourceSpec> {
    let source = args[0].to_ascii_uppercase();
    let arg = |i: usize| args.get(i).cloned();
    Ok(match source.as_str() {
        "FILE" => ConfigSourceSpec::File {
            path: arg(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE)),
        },
        // -c CONFIGMAP [mountDir] [key]; defaults applied inside the source (/etc/edgecommons,
        // config.json). The k8s-native source; the default on the KUBERNETES platform.
        "CONFIGMAP" => ConfigSourceSpec::ConfigMap {
            mount_dir: arg(1).map(PathBuf::from),
            key: arg(2),
        },
        "ENV" => ConfigSourceSpec::Env {
            var: arg(1).unwrap_or_else(|| DEFAULT_ENV_VAR.to_string()),
        },
        "GG_CONFIG" => ConfigSourceSpec::Greengrass {
            component: arg(1),
            key: arg(2).unwrap_or_else(|| DEFAULT_GG_CONFIG_KEY.to_string()),
        },
        "SHADOW" => ConfigSourceSpec::Shadow { name: arg(1) },
        "CONFIG_COMPONENT" => ConfigSourceSpec::ConfigComponent,
        other => {
            return Err(EdgeCommonsError::Cli(format!(
                "unknown config source '{other}'"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[&str]) -> Result<ParsedArgs> {
        let mut argv = vec!["prog"];
        argv.extend_from_slice(extra);
        parse_from(argv)
    }

    #[test]
    fn explicit_greengrass_gives_ipc_and_gg_config_default() {
        let a = parse(&["--platform", "GREENGRASS"]).unwrap();
        assert_eq!(a.platform, Platform::Greengrass);
        assert_eq!(a.transport, Transport::Ipc);
        assert_eq!(
            a.config,
            ConfigSourceSpec::Greengrass {
                component: None,
                key: "ComponentConfig".into()
            }
        );
        assert_eq!(a.thing, None);
    }

    #[test]
    fn explicit_host_derives_mqtt() {
        let a = parse(&["--platform", "HOST"]).unwrap();
        assert_eq!(a.platform, Platform::Host);
        assert_eq!(a.transport, Transport::Mqtt);
    }

    #[test]
    fn file_source_with_and_without_path() {
        assert_eq!(
            parse(&["--platform", "HOST", "-c", "FILE"]).unwrap().config,
            ConfigSourceSpec::File {
                path: PathBuf::from("config.json")
            }
        );
        assert_eq!(
            parse(&["--platform", "HOST", "-c", "FILE", "custom.json"])
                .unwrap()
                .config,
            ConfigSourceSpec::File {
                path: PathBuf::from("custom.json")
            }
        );
    }

    #[test]
    fn gg_config_component_and_key() {
        assert_eq!(
            parse(&[
                "--platform",
                "GREENGRASS",
                "-c",
                "GG_CONFIG",
                "com.other",
                "MyKey"
            ])
            .unwrap()
            .config,
            ConfigSourceSpec::Greengrass {
                component: Some("com.other".into()),
                key: "MyKey".into()
            }
        );
    }

    #[test]
    fn transport_mqtt_carries_messaging_config_path() {
        let a = parse(&["--platform", "HOST", "--transport", "MQTT", "msg.json"]).unwrap();
        assert_eq!(a.transport, Transport::Mqtt);
        assert_eq!(a.messaging_config_path, Some(PathBuf::from("msg.json")));
    }

    // ---------- FR-MSG-1: default messaging-config path from CONFIGMAP ----------

    #[test]
    fn configmap_mqtt_defaults_messaging_path_to_configmap_file() {
        // KUBERNETES defaults to CONFIGMAP + MQTT; with no positional `--transport MQTT <path>`
        // the messaging-config path defaults to the resolved ConfigMap file (/etc/edgecommons/config.json).
        let a = parse(&["--platform", "KUBERNETES"]).unwrap();
        assert_eq!(a.transport, Transport::Mqtt);
        assert_eq!(
            a.config,
            ConfigSourceSpec::ConfigMap {
                mount_dir: None,
                key: None
            }
        );
        assert_eq!(
            a.messaging_config_path,
            Some(PathBuf::from("/etc/edgecommons").join("config.json"))
        );
    }

    #[test]
    fn configmap_mqtt_default_path_uses_custom_dir_and_key() {
        // The default tracks the SAME (dir, key) the CONFIGMAP source resolved from `-c CONFIGMAP`.
        let a = parse(&[
            "--platform",
            "KUBERNETES",
            "-c",
            "CONFIGMAP",
            "/mnt/cfg",
            "app.json",
        ])
        .unwrap();
        assert_eq!(
            a.messaging_config_path,
            Some(PathBuf::from("/mnt/cfg").join("app.json"))
        );
    }

    #[test]
    fn configmap_mqtt_no_explicit_path_needed() {
        // The point of FR-MSG-1: parsing succeeds under CONFIGMAP+MQTT with NO positional path,
        // and a path is nonetheless available for messaging init.
        let a = parse(&["--platform", "KUBERNETES"]).unwrap();
        assert!(a.messaging_config_path.is_some());
    }

    #[test]
    fn configmap_mqtt_explicit_path_still_wins() {
        // The old explicit-path behavior is unchanged: an explicit `--transport MQTT <path>`
        // overrides the CONFIGMAP default.
        let a = parse(&[
            "--platform",
            "KUBERNETES",
            "--transport",
            "MQTT",
            "explicit.json",
        ])
        .unwrap();
        assert_eq!(
            a.messaging_config_path,
            Some(PathBuf::from("explicit.json"))
        );
    }

    #[test]
    fn host_mqtt_does_not_synthesize_a_messaging_path() {
        // Only CONFIGMAP synthesizes a default messaging path; HOST defaults to FILE (not CONFIGMAP),
        // so HOST+MQTT still requires an explicit path (enforced later, at messaging init).
        let a = parse(&["--platform", "HOST"]).unwrap();
        assert_eq!(a.transport, Transport::Mqtt);
        assert_eq!(
            a.config,
            ConfigSourceSpec::File {
                path: PathBuf::from("config.json")
            }
        );
        assert_eq!(a.messaging_config_path, None);
    }

    #[test]
    fn file_source_mqtt_does_not_synthesize_a_messaging_path() {
        // Only CONFIGMAP triggers the default; an explicit FILE source under MQTT does not.
        let a = parse(&[
            "--platform",
            "HOST",
            "--transport",
            "MQTT",
            "-c",
            "FILE",
            "config.json",
        ]);
        // Note: order — the positional path is consumed by --transport; here we give none, so
        // FILE has its own path and messaging stays None.
        let a = a.unwrap();
        assert_eq!(
            a.config,
            ConfigSourceSpec::File {
                path: PathBuf::from("config.json")
            }
        );
        assert_eq!(a.messaging_config_path, None);
    }

    #[test]
    fn explicit_transport_ipc_on_host_violates_the_ipc_lock() {
        assert!(parse(&["--platform", "HOST", "--transport", "IPC"]).is_err());
    }

    #[test]
    fn kubernetes_resolves_to_mqtt_and_configmap() {
        // Phase 1a: KUBERNETES resolves cleanly — MQTT transport + the CONFIGMAP default source.
        let a = parse(&["--platform", "KUBERNETES"]).unwrap();
        assert_eq!(a.platform, Platform::Kubernetes);
        assert_eq!(a.transport, Transport::Mqtt);
        assert_eq!(
            a.config,
            ConfigSourceSpec::ConfigMap {
                mount_dir: None,
                key: None
            }
        );
    }

    #[test]
    fn kubernetes_rejects_ipc_transport() {
        // The IPC × KUBERNETES rejection is retained (only a Nucleus provides the IPC socket).
        assert!(parse(&["--platform", "KUBERNETES", "--transport", "IPC"]).is_err());
    }

    #[test]
    fn configmap_source_with_and_without_args() {
        assert_eq!(
            parse(&["--platform", "KUBERNETES", "-c", "CONFIGMAP"])
                .unwrap()
                .config,
            ConfigSourceSpec::ConfigMap {
                mount_dir: None,
                key: None
            }
        );
        assert_eq!(
            parse(&[
                "--platform",
                "KUBERNETES",
                "-c",
                "CONFIGMAP",
                "/mnt/cfg",
                "app.json"
            ])
            .unwrap()
            .config,
            ConfigSourceSpec::ConfigMap {
                mount_dir: Some(PathBuf::from("/mnt/cfg")),
                key: Some("app.json".into())
            }
        );
    }

    #[test]
    fn thing_takes_the_full_string() {
        // Guards against the historical bug that truncated -t to one character.
        let a = parse(&["--platform", "HOST", "-t", "my-thing-name"]).unwrap();
        assert_eq!(a.thing.as_deref(), Some("my-thing-name"));
        assert_eq!(a.identity, "my-thing-name");
    }

    #[test]
    fn unknown_source_is_rejected() {
        assert!(parse(&["--platform", "HOST", "-c", "BOGUS"]).is_err());
    }

    #[test]
    fn unknown_platform_is_rejected() {
        assert!(parse(&["--platform", "BOGUS"]).is_err());
    }

    #[test]
    fn unknown_transport_is_rejected() {
        assert!(parse(&["--platform", "HOST", "--transport", "BOGUS"]).is_err());
    }

    #[test]
    fn shared_config_disable_flag_is_removed() {
        assert!(parse(&["--platform", "HOST", "--no-shared-config"]).is_err());
    }

    #[test]
    fn legacy_mode_flag_is_rejected() {
        let err = parse(&["-m", "STANDALONE", "msg.json"]).unwrap_err();
        assert!(err.to_string().contains("-m/--mode flag has been removed"));
        assert!(parse(&["--mode", "GREENGRASS"]).is_err());
    }
}
