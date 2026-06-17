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

/// Extract the component config from a full shadow document as the **verbatim JSON
/// string** stored under `ComponentConfig`: prefer `state.desired.ComponentConfig`,
/// fall back to `state.reported.ComponentConfig`.
///
/// The raw string is returned (not a re-parsed value) so it can be reported back
/// **byte-for-byte**. The shadow service treats `ComponentConfig` as an opaque
/// string, so the delta only clears when `reported` equals `desired` exactly;
/// re-serializing a parsed value would reorder keys and the delta would never clear.
fn extract_config_str(doc: &Value) -> Option<String> {
    let state = doc.get("state")?;
    for key in ["desired", "reported"] {
        if let Some(s) = state
            .get(key)
            .and_then(|d| d.get("ComponentConfig"))
            .and_then(Value::as_str)
        {
            return Some(s.to_string());
        }
    }
    None
}

/// Report the applied config back into `state.reported.ComponentConfig`,
/// acknowledging the desired state and clearing the shadow delta.
///
/// `component_config` is the **stringified** config JSON, reported verbatim. To clear
/// the delta this string MUST byte-match `state.desired.ComponentConfig`, so callers
/// pass the exact string they received from the shadow (never a re-serialized value);
/// mirrors the Java/Python `reportUpdatedConfiguration(String)`.
async fn report_config(rt: &ipc::IpcRuntime, thing: &str, shadow: &str, component_config: &str) {
    let doc = json!({ "state": { "reported": { "ComponentConfig": component_config } } });
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

/// The default config as a compact JSON string (for reporting when no shadow exists).
fn default_config_str() -> String {
    serde_json::to_string(&default_config()).unwrap_or_else(|_| "{}".to_string())
}

#[async_trait]
impl ConfigSource for ShadowConfigSource {
    async fn load(&self) -> Result<Value> {
        let rt = ipc::global();
        rt.connect().await?;
        // The raw `ComponentConfig` string is reported back verbatim so it byte-matches
        // `desired` and clears the delta; it is also parsed into the config we return.
        let config_str = match rt.get_shadow(&self.thing_name, Some(self.shadow_name.clone())).await {
            Ok(bytes) if !bytes.is_empty() => {
                let doc: Value = serde_json::from_slice(&bytes)?;
                extract_config_str(&doc).unwrap_or_else(default_config_str)
            }
            // Shadow does not exist yet (or is empty): bootstrap a default.
            _ => {
                tracing::info!(shadow = %self.shadow_name, "SHADOW: no shadow document; using default config");
                default_config_str()
            }
        };
        // Acknowledge by reporting the applied config back verbatim (clears the delta).
        report_config(rt, &self.thing_name, &self.shadow_name, &config_str).await;
        let config: Value = serde_json::from_str(&config_str).unwrap_or_else(|_| default_config());
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
                        // The delta's `state` carries the changed `ComponentConfig` (a string).
                        if let Ok(doc) = serde_json::from_slice::<Value>(&payload) {
                            if let Some(cfg_str) = doc
                                .get("state")
                                .and_then(|s| s.get("ComponentConfig"))
                                .and_then(Value::as_str)
                            {
                                // Report the EXACT string back to clear the delta (byte-match),
                                // then parse it for the consumer.
                                report_config(rt, &thing, &shadow, cfg_str).await;
                                if let Ok(cfg) = serde_json::from_str::<Value>(cfg_str) {
                                    if out_tx.send(cfg).is_err() {
                                        break; // consumer gone
                                    }
                                }
                            }
                        }
                    }
                    ("get", "rejected") => {
                        tracing::warn!(shadow = %shadow, "SHADOW: shadow missing; reporting default config");
                        report_config(rt, &thing, &shadow, &default_config_str()).await;
                    }
                    _ => {} // update/accepted, get/accepted, etc. — ignored
                }
            }
        });
        Some(out_rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `extract_config_str` must return the `ComponentConfig` string **verbatim**
    /// (key order preserved), so it can be reported back byte-for-byte to clear the
    /// shadow delta. Re-serializing would reorder keys and the delta would never
    /// clear (the on-device storm bug this guards against).
    #[test]
    fn extract_config_str_preserves_desired_string_verbatim() {
        // Note the deliberately non-alphabetical key order.
        let desired = r#"{"logging":{"level":"DEBUG"},"component":{"global":{"publish_interval":7}}}"#;
        let doc = json!({ "state": { "desired": { "ComponentConfig": desired } } });
        let extracted = extract_config_str(&doc).expect("desired present");
        assert_eq!(extracted, desired, "must be byte-identical (no re-serialization)");
    }

    #[test]
    fn extract_config_str_falls_back_to_reported() {
        let reported = r#"{"component":{"global":{"publish_interval":3}}}"#;
        let doc = json!({ "state": { "reported": { "ComponentConfig": reported } } });
        assert_eq!(extract_config_str(&doc).as_deref(), Some(reported));
    }

    #[test]
    fn extract_config_str_none_when_absent() {
        assert!(extract_config_str(&json!({ "state": {} })).is_none());
        assert!(extract_config_str(&json!({})).is_none());
    }

    #[test]
    fn default_config_str_is_valid_json() {
        let s = default_config_str();
        let v: Value = serde_json::from_str(&s).expect("default config is valid JSON");
        assert!(v.get("component").is_some());
    }
}
