//! # Logging
//!
//! **One-liner purpose**: Initialize the `tracing` subscriber from config, with
//! runtime-reloadable log level.
//!
//! ## Overview
//! Installs a `tracing-subscriber` registry with a `fmt` layer and a **reloadable**
//! `EnvFilter`. The level comes from `logging.level`; [`reconfigure`] (driven by a
//! config hot-reload via [`LoggingReconfigurer`]) swaps the filter at runtime.
//!
//! ## Semantics & Architecture
//! - Idempotent install: if a global subscriber already exists, init is a no-op.
//! - The reload handle is stored type-erased in a `OnceLock`; reconfiguration is a
//!   cheap filter swap with no re-init.
//! - Error handling: infallible — an unparseable level falls back to `info`.
//! - Custom format strings, file logging with rotation, and per-logger levels remain
//!   future Phase 3 work; runtime *level* reconfiguration is implemented here.
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
//! - [`crate::config::model`], [`crate::config::ConfigChangeListener`].

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, reload, EnvFilter};

use crate::config::model::Config;
use crate::config::ConfigChangeListener;

/// Type-erased "reload the level filter" callback, installed once by [`init`].
static RECONFIGURE: OnceLock<Box<dyn Fn(EnvFilter) + Send + Sync>> = OnceLock::new();

/// Initialize the global tracing subscriber with a reloadable level filter.
pub fn init(config: &Config) {
    let (layer, handle) = reload::Layer::new(level_filter(config));
    let installed = tracing_subscriber::registry()
        .with(layer)
        .with(fmt::layer())
        .try_init()
        .is_ok();
    if installed {
        let _ = RECONFIGURE.set(Box::new(move |filter: EnvFilter| {
            let _ = handle.reload(filter);
        }));
    }
}

/// Apply the log level from `config` to the running subscriber (no-op if logging
/// was never initialized by this library).
pub fn reconfigure(config: &Config) {
    if let Some(reconfigure) = RECONFIGURE.get() {
        reconfigure(level_filter(config));
    }
}

/// Build an `EnvFilter` from the config's `logging.level` (default `info`).
fn level_filter(config: &Config) -> EnvFilter {
    let level = config
        .parsed
        .logging
        .level
        .clone()
        .unwrap_or_else(|| "INFO".to_string());
    EnvFilter::try_new(level.to_ascii_lowercase()).unwrap_or_else(|_| EnvFilter::new("info"))
}

/// A [`ConfigChangeListener`] that re-applies the log level on config hot-reload.
pub struct LoggingReconfigurer;

#[async_trait]
impl ConfigChangeListener for LoggingReconfigurer {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        reconfigure(&config);
        true
    }
}
