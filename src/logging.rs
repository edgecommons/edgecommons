//! Logging initialization via `tracing`.
//!
//! Phase 0 wires the log **level** from config to a `tracing-subscriber` fmt
//! layer. Phase 1/3 add: custom format strings, file logging with rotation
//! (`tracing-appender`), per-logger levels (`loggers`), `globalControl`
//! semantics, and runtime reconfiguration via a `reload::Handle` driven by the
//! config watch channel.

use tracing_subscriber::EnvFilter;

use crate::config::model::Config;

/// Initialize the global tracing subscriber from the config's logging level.
///
/// Idempotent: if a global subscriber is already installed (e.g. by the host
/// application or a previous call), this is a no-op.
pub fn init(config: &Config) {
    let level = config
        .parsed
        .logging
        .level
        .clone()
        .unwrap_or_else(|| "INFO".to_string());

    let filter = EnvFilter::try_new(level.to_ascii_lowercase())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // try_init returns Err if a subscriber is already set; that is fine.
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
