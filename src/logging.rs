//! # Logging
//!
//! **One-liner purpose**: Initialize the `tracing` subscriber from the config's
//! logging settings.
//!
//! ## Overview
//! Wires the log **level** from config to a `tracing-subscriber` fmt layer.
//! Later sub-steps add: custom format strings, file logging with rotation
//! (`tracing-appender`), per-logger levels (`loggers`), `globalControl` semantics,
//! and runtime reconfiguration via a `reload::Handle` driven by the config watch
//! channel.
//!
//! ## Semantics & Architecture
//! - Idempotent: installing a subscriber when one already exists is a no-op.
//! - Thread-safety: installs a process-global subscriber.
//! - Error handling: infallible — an unparseable level falls back to `info`.
//!
//! ## Usage Example
//! ```
//! use ggcommons::config::model::Config;
//! use ggcommons::logging;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("c", "t", json!({ "logging": { "level": "DEBUG" } })).unwrap();
//! logging::init(&cfg);
//! ```
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`crate::config::model`].

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
