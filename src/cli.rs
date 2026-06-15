//! Standard CLI contract, shared verbatim across the Java, Python, and Rust
//! libraries:
//!
//! - `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
//! - `-m/--mode <MODE> [path]` — `GREENGRASS (default) | STANDALONE <messaging_config.json>`
//! - `-t/--thing <name>` — IoT Thing name (takes the **full** string value)
//!
//! The variadic `-c`/`-m` options mirror the Java `configArgs[]` array: the first
//! token selects the source/mode and the remaining tokens are source-specific.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Arg, Command};

use crate::error::{GgError, Result};

/// Runtime mode selected by `-m/--mode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Default: Greengrass IPC.
    Greengrass,
    /// Dual-broker MQTT for non-Greengrass environments; requires a messaging config file.
    Standalone { messaging_config_path: PathBuf },
}

/// Configuration source selected by `-c/--config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSourceSpec {
    /// `FILE [path]` — JSON file (default `config.json`).
    File { path: PathBuf },
    /// `ENV [var]` — JSON in an environment variable (default `CONFIG`).
    Env { var: String },
    /// `GG_CONFIG [component] [key]` — Greengrass deployment config (default key `ComponentConfig`).
    Greengrass { component: Option<String>, key: String },
    /// `SHADOW [name]` — IoT named device shadow.
    Shadow { name: Option<String> },
    /// `CONFIG_COMPONENT` — dedicated configuration component (over messaging).
    ConfigComponent,
}

/// Parsed standard arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub mode: RuntimeMode,
    pub config: ConfigSourceSpec,
    pub thing: Option<String>,
}

const DEFAULT_CONFIG_FILE: &str = "config.json";
const DEFAULT_ENV_VAR: &str = "CONFIG";
const DEFAULT_GG_CONFIG_KEY: &str = "ComponentConfig";

/// Build the base `clap` command. Application-specific options can be merged onto
/// this in a later phase (mirrors the Java `appOptions` merge).
pub fn command() -> Command {
    Command::new("ggcommons")
        .disable_help_flag(false)
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .num_args(1..=3)
                .value_parser(clap::value_parser!(String))
                .value_name("SOURCE")
                .help("Config source: FILE|ENV|GG_CONFIG|SHADOW|CONFIG_COMPONENT [args...]"),
        )
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .num_args(1..=2)
                .value_parser(clap::value_parser!(String))
                .value_name("MODE")
                .help("Runtime mode: GREENGRASS | STANDALONE <messaging_config.json>"),
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

/// Parse the standard arguments from an argv-style iterator.
///
/// The iterator is expected to include the program name as its first element
/// (as produced by `std::env::args_os()`).
pub fn parse_from<I, T>(args: I) -> Result<ParsedArgs>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = command()
        .try_get_matches_from(args)
        .map_err(|e| GgError::Cli(e.to_string()))?;

    let config = match matches.get_many::<String>("config") {
        Some(values) => parse_config_source(&values.cloned().collect::<Vec<_>>())?,
        None => ConfigSourceSpec::Greengrass {
            component: None,
            key: DEFAULT_GG_CONFIG_KEY.to_string(),
        },
    };

    let mode = match matches.get_many::<String>("mode") {
        Some(values) => parse_mode(&values.cloned().collect::<Vec<_>>())?,
        None => RuntimeMode::Greengrass,
    };

    let thing = matches.get_one::<String>("thing").cloned();

    Ok(ParsedArgs { mode, config, thing })
}

fn parse_config_source(args: &[String]) -> Result<ConfigSourceSpec> {
    let source = args[0].to_ascii_uppercase();
    let arg = |i: usize| args.get(i).cloned();
    Ok(match source.as_str() {
        "FILE" => ConfigSourceSpec::File {
            path: arg(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE)),
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
        other => return Err(GgError::Cli(format!("unknown config source '{other}'"))),
    })
}

fn parse_mode(args: &[String]) -> Result<RuntimeMode> {
    let mode = args[0].to_ascii_uppercase();
    match mode.as_str() {
        "GREENGRASS" => Ok(RuntimeMode::Greengrass),
        "STANDALONE" => {
            let path = args.get(1).ok_or_else(|| {
                GgError::Cli("STANDALONE mode requires a messaging config file path".to_string())
            })?;
            Ok(RuntimeMode::Standalone { messaging_config_path: PathBuf::from(path) })
        }
        other => Err(GgError::Cli(format!("unknown mode '{other}'"))),
    }
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
    fn defaults_are_greengrass_and_gg_config() {
        let a = parse(&[]).unwrap();
        assert_eq!(a.mode, RuntimeMode::Greengrass);
        assert_eq!(
            a.config,
            ConfigSourceSpec::Greengrass { component: None, key: "ComponentConfig".into() }
        );
        assert_eq!(a.thing, None);
    }

    #[test]
    fn file_source_with_and_without_path() {
        assert_eq!(
            parse(&["-c", "FILE"]).unwrap().config,
            ConfigSourceSpec::File { path: PathBuf::from("config.json") }
        );
        assert_eq!(
            parse(&["-c", "FILE", "custom.json"]).unwrap().config,
            ConfigSourceSpec::File { path: PathBuf::from("custom.json") }
        );
    }

    #[test]
    fn gg_config_component_and_key() {
        assert_eq!(
            parse(&["-c", "GG_CONFIG", "com.other", "MyKey"]).unwrap().config,
            ConfigSourceSpec::Greengrass { component: Some("com.other".into()), key: "MyKey".into() }
        );
    }

    #[test]
    fn standalone_requires_a_path() {
        assert!(parse(&["-m", "STANDALONE"]).is_err());
        assert_eq!(
            parse(&["-m", "STANDALONE", "msg.json"]).unwrap().mode,
            RuntimeMode::Standalone { messaging_config_path: PathBuf::from("msg.json") }
        );
    }

    #[test]
    fn thing_takes_the_full_string() {
        // Guards against the historical bug that truncated -t to one character.
        let a = parse(&["-t", "my-thing-name"]).unwrap();
        assert_eq!(a.thing.as_deref(), Some("my-thing-name"));
    }

    #[test]
    fn unknown_source_is_rejected() {
        assert!(parse(&["-c", "BOGUS"]).is_err());
    }
}
