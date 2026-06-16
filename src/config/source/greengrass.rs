//! # Configuration source — GG_CONFIG (Phase 2)
//!
//! **One-liner purpose**: Load (and hot-reload) configuration from the Greengrass
//! deployment via IPC `GetConfiguration` + `SubscribeToConfigurationUpdate`.
//!
//! ## Overview
//! Reads the component's deployed configuration at a key path (default key
//! `ComponentConfig`) through the shared [`crate::ipc`] runtime, and watches it for
//! deployment-driven changes. Compiled only with the `greengrass` feature.
//!
//! ## Semantics & Architecture
//! - `load` issues `GetConfiguration` and returns the subtree as a JSON document.
//! - `watch` registers a `SubscribeToConfigurationUpdate`; because that operation
//!   delivers only the changed key path, the runtime re-fetches the value and
//!   forwards the fresh document on the channel.
//! - `Send + Sync`; async via `async_trait`. Errors map to
//!   [`crate::error::GgError::Ipc`].
//!
//! ## Status
//! Phase 2, **compile-only** — not yet validated against a live Greengrass core.
//!
//! ## Related Modules
//! - [`super`], [`crate::ipc`].

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use super::ConfigSource;
use crate::error::Result;
use crate::ipc;

/// Greengrass-IPC-backed configuration source (`GetConfiguration`).
pub struct GreengrassConfigSource {
    /// Other component to read config from, or `None` for this component.
    component: Option<String>,
    /// Top-level configuration key to read (e.g. `ComponentConfig`).
    key: String,
}

impl GreengrassConfigSource {
    /// Create a source reading `key` (default `ComponentConfig`) from `component`
    /// (or this component when `None`).
    pub fn new(component: Option<String>, key: String) -> Self {
        Self { component, key }
    }

    /// The IPC key path: empty for the whole config, else the single configured key.
    fn key_path(&self) -> Vec<String> {
        if self.key.is_empty() {
            Vec::new()
        } else {
            vec![self.key.clone()]
        }
    }
}

#[async_trait]
impl ConfigSource for GreengrassConfigSource {
    async fn load(&self) -> Result<Value> {
        let rt = ipc::global();
        rt.connect().await?;
        rt.get_config(self.key_path(), self.component.clone()).await
    }

    fn source_name(&self) -> &str {
        "GG_CONFIG"
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let component = self.component.clone();
        let key_path = self.key_path();
        // Registration is async; spawn it. `build()` runs in an async context.
        tokio::spawn(async move {
            let rt = ipc::global();
            if let Err(e) = rt.connect().await {
                tracing::warn!(error = %e, "GG_CONFIG watch: connect failed");
                return;
            }
            if let Err(e) = rt.watch_config(component, key_path, tx).await {
                tracing::warn!(error = %e, "GG_CONFIG watch: subscribe failed");
            }
        });
        Some(rx)
    }
}
