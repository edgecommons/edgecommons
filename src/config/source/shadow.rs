//! # Configuration source — SHADOW (Phase 2)
//!
//! **One-liner purpose**: Load (and hot-reload) configuration from an AWS IoT
//! **named** device shadow via IPC, mirroring the Java/Python `ShadowConfigProvider`
//! contract exactly so the three implementations interoperate on the same shadow.
//!
//! ## Overview
//! The component configuration is carried in the shadow as a **stringified JSON**
//! document under the `ComponentConfig` key:
//!
//! ```json
//! { "state": { "desired":  { "ComponentConfig": "<json-string of the config>" },
//!              "reported": { "ComponentConfig": "<json-string of the config>" } } }
//! ```
//!
//! - `load` reads `state.desired.ComponentConfig` (falling back to
//!   `state.reported.ComponentConfig`), parses the embedded JSON, and **reports the
//!   applied config back** into `state.reported` — this is what makes `desired ==
//!   reported` and clears the shadow delta (matching Java/Python).
//! - `watch` subscribes over **local IPC pub/sub** to the shadow's
//!   `$aws/things/<thing>/shadow/name/<name>/+/+` topics (served by the
//!   `ShadowManager` component). On `update/delta` it applies the new config and
//!   reports it back; on `get/rejected` it bootstraps a default config. Reacting to
//!   the *delta* (not *accepted*) is loop-safe: reporting `reported == desired`
//!   clears the delta, so our own report does not re-trigger.
//! - The shadow name defaults to the component name when `-c SHADOW` is given with
//!   no name (matching the other libraries). Compiled only with the `greengrass`
//!   feature.
//!
//! ## Semantics & Architecture
//! - `Send + Sync`; async via `async_trait`. Errors map to
//!   [`crate::error::GgError::Ipc`] / [`crate::error::GgError::Json`].
//!
//! ## Status
//! Phase 2; validated on a live Greengrass core with the `ShadowManager` component.
//!
//! ## Related Modules
//! - [`super`], [`crate::ipc`].

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::ConfigSource;
use crate::error::Result;
use crate::ipc;
use crate::messaging::{Destination, Qos};

/// Greengrass-IPC-backed device-shadow configuration source (named shadow).
pub struct ShadowConfigSource {
    thing_name: String,
    shadow_name: String,
}

impl ShadowConfigSource {
    /// Create a source for the given named shadow. When `name` is `None`, the shadow
    /// name defaults to the component name (matching Java/Python).
    pub fn new(name: Option<String>, thing_name: &str, component_name: &str) -> Self {
        Self {
            thing_name: thing_name.to_string(),
            shadow_name: name.unwrap_or_else(|| component_name.to_string()),
        }
    }
}

/// The default configuration written when no shadow exists yet (mirrors the
/// Java/Python `getDefaultConfig` / `_DEFAULT_CONFIGURATION`).
fn default_config() -> Value {
    json!({
        "logging": {},
        "tags": {},
        "heartbeat": {},
        "component": { "global": {}, "instances": [] }
    })
}

/// Extract the component config from a full shadow document: prefer
/// `state.desired.ComponentConfig`, fall back to `state.reported.ComponentConfig`.
/// The value is a JSON **string** that is itself parsed into the config object.
fn extract_config(doc: &Value) -> Option<Value> {
    let state = doc.get("state")?;
    for key in ["desired", "reported"] {
        if let Some(s) = state
            .get(key)
            .and_then(|d| d.get("ComponentConfig"))
            .and_then(Value::as_str)
        {
            if let Ok(cfg) = serde_json::from_str::<Value>(s) {
                return Some(cfg);
            }
        }
    }
    None
}

/// Report the applied config back into `state.reported.ComponentConfig` (stringified),
/// acknowledging the desired state and clearing the shadow delta.
async fn report_config(rt: &ipc::IpcRuntime, thing: &str, shadow: &str, config: &Value) {
    let stringified = serde_json::to_string(config).unwrap_or_else(|_| "{}".to_string());
    let doc = json!({ "state": { "reported": { "ComponentConfig": stringified } } });
    match serde_json::to_vec(&doc) {
        Ok(payload) => {
            if let Err(e) = rt
                .update_shadow(thing, Some(shadow.to_string()), payload)
                .await
            {
                tracing::warn!(error = %e, "SHADOW: failed to report config back to shadow");
            }
        }
        Err(e) => tracing::warn!(error = %e, "SHADOW: failed to serialize reported config"),
    }
}

#[async_trait]
impl ConfigSource for ShadowConfigSource {
    async fn load(&self) -> Result<Value> {
        let rt = ipc::global();
        rt.connect().await?;
        let config = match rt.get_shadow(&self.thing_name, Some(self.shadow_name.clone())).await {
            Ok(bytes) if !bytes.is_empty() => {
                let doc: Value = serde_json::from_slice(&bytes)?;
                extract_config(&doc).unwrap_or_else(default_config)
            }
            // Shadow does not exist yet (or is empty): bootstrap a default.
            _ => {
                tracing::info!(shadow = %self.shadow_name, "SHADOW: no shadow document; using default config");
                default_config()
            }
        };
        // Acknowledge by reporting the applied config back (clears the delta).
        report_config(rt, &self.thing_name, &self.shadow_name, &config).await;
        Ok(config)
    }

    fn source_name(&self) -> &str {
        "SHADOW"
    }

    fn watch(&self) -> Option<mpsc::UnboundedReceiver<Value>> {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let thing = self.thing_name.clone();
        let shadow = self.shadow_name.clone();
        tokio::spawn(async move {
            let rt = ipc::global();
            if let Err(e) = rt.connect().await {
                tracing::warn!(error = %e, "SHADOW watch: connect failed");
                return;
            }
            // Local IPC pub/sub on the shadow's event topics (served by ShadowManager).
            let filter = format!("$aws/things/{thing}/shadow/name/{shadow}/+/+");
            let (tx, mut rx) = mpsc::channel::<(String, Vec<u8>)>(16);
            if let Err(e) = rt.subscribe(&filter, Destination::Local, Qos::AtLeastOnce, tx).await {
                tracing::warn!(error = %e, "SHADOW watch: subscribe failed");
                return;
            }
            while let Some((topic, payload)) = rx.recv().await {
                // Topic suffix is `.../<action>/<result>`.
                let mut suffix = topic.rsplit('/');
                let result = suffix.next().unwrap_or("");
                let action = suffix.next().unwrap_or("");
                match (action, result) {
                    ("update", "delta") => {
                        // The delta's `state` carries the changed `ComponentConfig`.
                        if let Ok(doc) = serde_json::from_slice::<Value>(&payload) {
                            if let Some(cfg) = doc
                                .get("state")
                                .and_then(|s| s.get("ComponentConfig"))
                                .and_then(Value::as_str)
                                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            {
                                report_config(rt, &thing, &shadow, &cfg).await;
                                if out_tx.send(cfg).is_err() {
                                    break; // consumer gone
                                }
                            }
                        }
                    }
                    ("get", "rejected") => {
                        tracing::warn!(shadow = %shadow, "SHADOW: shadow missing; reporting default config");
                        report_config(rt, &thing, &shadow, &default_config()).await;
                    }
                    _ => {} // update/accepted, get/accepted, etc. — ignored
                }
            }
        });
        Some(out_rx)
    }
}
