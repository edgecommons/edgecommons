//! # Configuration source — SHADOW (Phase 2)
//!
//! **One-liner purpose**: Load (and hot-reload) configuration from an AWS IoT
//! device shadow via IPC `GetThingShadow`, with a delta subscription for updates.
//!
//! ## Overview
//! Reads the device's (classic or named) shadow through the shared [`crate::ipc`]
//! runtime and treats the shadow's desired state as the component configuration.
//! Compiled only with the `greengrass` feature.
//!
//! ## Semantics & Architecture
//! - The thing name is taken from `AWS_IOT_THING_NAME` (set by the nucleus).
//! - `load` issues `GetThingShadow` and extracts `state.desired` (falling back to
//!   `state.reported`, then `state`, then the whole document).
//! - `watch` subscribes to the shadow's `.../update/delta` topic over the IoT Core
//!   bridge; on each delta it re-fetches the shadow and forwards the extracted
//!   config. (Reporting accepted config back via `UpdateThingShadow` is available on
//!   the runtime but left to the component, matching the other sources.)
//! - `Send + Sync`; async via `async_trait`. Errors map to
//!   [`crate::error::GgError::Ipc`] / [`crate::error::GgError::Config`].
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
use crate::error::{GgError, Result};
use crate::ipc;
use crate::messaging::{Destination, Qos};

/// Greengrass-IPC-backed device-shadow configuration source.
pub struct ShadowConfigSource {
    /// Named shadow, or `None` for the classic shadow.
    name: Option<String>,
}

impl ShadowConfigSource {
    /// Create a source for the given named shadow (or the classic shadow when `None`).
    pub fn new(name: Option<String>) -> Self {
        Self { name }
    }
}

/// Resolve the device thing name from the nucleus-provided environment.
fn thing_name() -> Result<String> {
    std::env::var("AWS_IOT_THING_NAME").map_err(|_| {
        GgError::Config("SHADOW source requires AWS_IOT_THING_NAME to be set".to_string())
    })
}

/// Extract the configuration document from a shadow payload: prefer `state.desired`,
/// then `state.reported`, then `state`, then the whole document.
fn extract_config(bytes: &[u8]) -> Result<Value> {
    let doc: Value = serde_json::from_slice(bytes)?;
    if let Some(state) = doc.get("state") {
        for key in ["desired", "reported"] {
            if let Some(v) = state.get(key) {
                return Ok(v.clone());
            }
        }
        return Ok(state.clone());
    }
    Ok(doc)
}

/// Build the shadow `update/delta` topic for `thing` / optional named shadow.
fn delta_topic(thing: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("$aws/things/{thing}/shadow/name/{n}/update/delta"),
        None => format!("$aws/things/{thing}/shadow/update/delta"),
    }
}

#[async_trait]
impl ConfigSource for ShadowConfigSource {
    async fn load(&self) -> Result<Value> {
        let rt = ipc::global();
        rt.connect().await?;
        let thing = thing_name()?;
        let bytes = rt.get_shadow(&thing, self.name.clone()).await?;
        extract_config(&bytes)
    }

    fn source_name(&self) -> &str {
        "SHADOW"
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let name = self.name.clone();
        tokio::spawn(async move {
            let rt = ipc::global();
            if let Err(e) = rt.connect().await {
                tracing::warn!(error = %e, "SHADOW watch: connect failed");
                return;
            }
            let thing = match thing_name() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(error = %e, "SHADOW watch: no thing name");
                    return;
                }
            };
            let topic = delta_topic(&thing, name.as_deref());
            let (dtx, mut drx) = mpsc::channel::<(String, Vec<u8>)>(8);
            if let Err(e) = rt
                .subscribe(&topic, Destination::IotCore, Qos::AtLeastOnce, dtx)
                .await
            {
                tracing::warn!(error = %e, "SHADOW watch: delta subscribe failed");
                return;
            }
            // On each delta, re-fetch the shadow and forward the extracted config.
            while drx.recv().await.is_some() {
                match rt.get_shadow(&thing, name.clone()).await {
                    Ok(bytes) => {
                        if let Ok(cfg) = extract_config(&bytes) {
                            if tx.send(cfg).is_err() {
                                break; // consumer gone
                            }
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "SHADOW watch: re-fetch failed"),
                }
            }
        });
        Some(rx)
    }
}
