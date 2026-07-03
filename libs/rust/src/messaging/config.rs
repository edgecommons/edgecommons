//! # Messaging — standalone configuration
//!
//! **One-liner purpose**: Parse the `-m STANDALONE <path>` messaging config file
//! describing the local broker and (optionally) AWS IoT Core.
//!
//! ## Overview
//! The file shape matches the Java/Python `standalone-messaging-sample.json`:
//!
//! ```json
//! { "messaging": {
//!     "local":   { "host": "localhost", "port": 1883, "clientId": "c-local",
//!                  "credentials": { "username": "u", "password": "p" } },
//!     "iotCore": { "endpoint": "...", "port": 8883, "clientId": "c-iot",
//!                  "credentials": { "certPath": "...", "keyPath": "...", "caPath": "..." } } } }
//! ```
//!
//! ## Semantics & Architecture
//! - Pure deserialization (`serde`); the only I/O is reading the file in [`MessagingConfig::load`].
//! - `iotCore` is optional. Its TLS connection is implemented in a later
//!   sub-step; until then, attempting to use it surfaces a clear error rather
//!   than connecting insecurely.
//! - Error handling: [`crate::error::Result`]; no panics.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo() -> ggcommons::Result<()> {
//! use ggcommons::messaging::config::MessagingConfig;
//! let mc = MessagingConfig::load("standalone-messaging.json").await?;
//! println!("local broker: {}:{}", mc.messaging.local.resolved_host()?, mc.messaging.local.port);
//! # Ok(())
//! # }
//! ```
//!
//! ## Related Modules
//! - [`crate::messaging::provider::mqtt`] — consumes this config to connect.

use std::path::Path;

use serde::Deserialize;

use crate::error::Result;

/// Top-level standalone messaging configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct MessagingConfig {
    pub messaging: Messaging,
}

/// The `messaging` object: a required local broker, an optional IoT Core, and an
/// optional MQTT Last-Will-and-Testament.
#[derive(Debug, Clone, Deserialize)]
pub struct Messaging {
    pub local: BrokerConfig,
    #[serde(rename = "iotCore", default)]
    pub iot_core: Option<BrokerConfig>,
    /// The optional `messaging.lwt` section (UNS-CANONICAL-DESIGN §6): an MQTT
    /// Last-Will-and-Testament registered on the **local-broker** connection at
    /// CONNECT (rumqttc re-sends the same options on reconnect, so the will is
    /// re-registered for free). The IPC transport DEBUG-logs and ignores it.
    #[serde(default)]
    pub lwt: Option<LwtConfig>,
}

/// The `messaging.lwt` section (UNS-CANONICAL-DESIGN §6, D-U9/M7).
///
/// There is deliberately NO retain field — the will is always registered with
/// `retain=false` (D9). `payload` is kept as a raw JSON value: a string is
/// published verbatim as UTF-8 bytes; any other JSON value is serialized to
/// compact JSON bytes; absent = empty bytes. `qos` accepts `0`/`1` (number-tolerant:
/// Greengrass-style `1.0` parses too) and defaults to `1`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LwtConfig {
    /// The will topic (e.g. the component's UNS state topic). Required.
    pub topic: String,
    /// The will payload, published VERBATIM (a string or a JSON object).
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    /// Will QoS (0 or 1), default 1. Number-tolerant (accepts `1` and `1.0`).
    #[serde(default)]
    pub qos: Option<serde_json::Value>,
}

impl LwtConfig {
    /// The effective QoS: the configured value, or the schema default 1 when
    /// absent. Non-numeric shapes surface as the raw value at validation time
    /// (see the provider's `build_last_will`).
    pub fn qos_or_default(&self) -> i64 {
        match &self.qos {
            None => 1,
            Some(v) => v
                .as_i64()
                .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
                .unwrap_or(-1), // out-of-domain sentinel — rejected by build_last_will
        }
    }

    /// The will payload bytes: a JSON string verbatim as UTF-8 bytes; any other
    /// JSON value as its compact JSON bytes; absent/null as empty bytes.
    pub fn payload_bytes(&self) -> Vec<u8> {
        match &self.payload {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::String(s)) => s.clone().into_bytes(),
            Some(other) => other.to_string().into_bytes(),
        }
    }
}

/// Connection settings for a single broker.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerConfig {
    /// Hostname for the local broker.
    #[serde(default)]
    pub host: Option<String>,
    /// Endpoint for IoT Core (alias for host in that section).
    #[serde(default)]
    pub endpoint: Option<String>,
    pub port: u16,
    pub client_id: String,
    #[serde(default)]
    pub credentials: Option<Credentials>,
}

impl BrokerConfig {
    /// The broker host, preferring `host` then `endpoint`.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Config` | Neither `host` nor `endpoint` is set | Add one to the messaging config |
    pub fn resolved_host(&self) -> Result<&str> {
        self.host
            .as_deref()
            .or(self.endpoint.as_deref())
            .ok_or_else(|| {
                crate::error::GgError::Config("broker config has no host/endpoint".to_string())
            })
    }
}

/// Broker credentials: username/password (local) or cert/key/CA (IoT Core / TLS).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub cert_path: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
    #[serde(default)]
    pub ca_path: Option<String>,
}

impl MessagingConfig {
    /// Load and parse the messaging config from a JSON file.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub async fn load(path: impl AsRef<Path>) -> Result<MessagingConfig>`
    /// - Reads the file asynchronously and deserializes it.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Io` | File missing or unreadable | Check the `-m STANDALONE <path>` argument |
    /// | `GgError::Json` | File is not valid messaging JSON | Fix the file shape |
    pub async fn load(path: impl AsRef<Path>) -> Result<MessagingConfig> {
        let bytes = tokio::fs::read(path.as_ref()).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_only() {
        let json = r#"{ "messaging": { "local": {
            "host": "localhost", "port": 1883, "clientId": "c" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(mc.messaging.local.resolved_host().unwrap(), "localhost");
        assert_eq!(mc.messaging.local.port, 1883);
        assert!(mc.messaging.iot_core.is_none());
    }

    #[test]
    fn accepts_kubernetes_service_dns_host() {
        // FR-MSG-2: a k8s Service DNS name is an opaque host string — no special handling, no
        // insecure behavior. It flows through verbatim as the broker host.
        let json = r#"{ "messaging": { "local": {
            "host": "emqx.mqtt.svc.cluster.local", "port": 1883, "clientId": "c" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            mc.messaging.local.resolved_host().unwrap(),
            "emqx.mqtt.svc.cluster.local"
        );
        assert_eq!(mc.messaging.local.port, 1883);
    }

    #[test]
    fn single_broker_topology_when_iotcore_absent() {
        // FR-MSG-3: no `iotCore` section => single-broker (local only / air-gapped).
        let json = r#"{ "messaging": { "local": {
            "host": "emqx.mqtt.svc.cluster.local", "port": 1883, "clientId": "c" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert!(
            mc.messaging.iot_core.is_none(),
            "absent iotCore => single-broker topology"
        );
    }

    #[test]
    fn dual_broker_topology_when_iotcore_present() {
        // FR-MSG-3: an `iotCore` section => dual-MQTT (local broker + AWS IoT Core).
        let json = r#"{ "messaging": {
            "local": { "host": "emqx.mqtt.svc.cluster.local", "port": 1883, "clientId": "l" },
            "iotCore": { "endpoint": "x.iot.amazonaws.com", "port": 8883, "clientId": "i",
                         "credentials": { "certPath": "c.pem", "keyPath": "k.pem", "caPath": "ca.pem" } } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert!(
            mc.messaging.iot_core.is_some(),
            "present iotCore => dual-broker topology"
        );
        assert_eq!(
            mc.messaging.iot_core.unwrap().resolved_host().unwrap(),
            "x.iot.amazonaws.com"
        );
    }

    #[test]
    fn parses_iotcore_endpoint_alias() {
        let json = r#"{ "messaging": {
            "local": { "host": "localhost", "port": 1883, "clientId": "l" },
            "iotCore": { "endpoint": "x.iot.amazonaws.com", "port": 8883, "clientId": "i" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let iot = mc.messaging.iot_core.unwrap();
        assert_eq!(iot.resolved_host().unwrap(), "x.iot.amazonaws.com");
    }

    #[test]
    fn resolved_host_errors_without_host_or_endpoint() {
        let json = r#"{ "messaging": { "local": { "port": 1883, "clientId": "c" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert!(mc.messaging.local.resolved_host().is_err());
    }

    #[test]
    fn parses_credentials_both_kinds() {
        let json = r#"{ "messaging": {
            "local": { "host": "h", "port": 1883, "clientId": "l",
                       "credentials": { "username": "u", "password": "p" } },
            "iotCore": { "endpoint": "e", "port": 8883, "clientId": "i",
                         "credentials": { "certPath": "c.pem", "keyPath": "k.pem", "caPath": "ca.pem" } } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let local_creds = mc.messaging.local.credentials.unwrap();
        assert_eq!(local_creds.username.as_deref(), Some("u"));
        assert_eq!(local_creds.password.as_deref(), Some("p"));
        let iot_creds = mc.messaging.iot_core.unwrap().credentials.unwrap();
        assert_eq!(iot_creds.cert_path.as_deref(), Some("c.pem"));
        assert_eq!(iot_creds.key_path.as_deref(), Some("k.pem"));
        assert_eq!(iot_creds.ca_path.as_deref(), Some("ca.pem"));
    }

    #[tokio::test]
    async fn load_reads_and_parses_a_file() {
        let dir = std::env::temp_dir().join(format!("ggcommons-mc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("messaging.json");
        std::fs::write(
            &path,
            r#"{ "messaging": { "local": { "host": "localhost", "port": 1884, "clientId": "c" } } }"#,
        )
        .unwrap();
        let mc = MessagingConfig::load(&path).await.unwrap();
        assert_eq!(mc.messaging.local.port, 1884);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn load_missing_file_is_error() {
        let result = MessagingConfig::load("/no/such/messaging.json").await;
        assert!(result.is_err());
    }

    #[test]
    fn parses_lwt_section_with_defaults() {
        let json = r#"{ "messaging": {
            "local": { "host": "h", "port": 1883, "clientId": "c" },
            "lwt": { "topic": "ecv1/gw-01/uns-bridge/main/state" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let lwt = mc.messaging.lwt.expect("lwt parsed");
        assert_eq!(lwt.topic, "ecv1/gw-01/uns-bridge/main/state");
        assert_eq!(lwt.qos_or_default(), 1, "qos defaults to 1");
        assert!(lwt.payload_bytes().is_empty(), "absent payload => empty bytes");
    }

    #[test]
    fn lwt_string_payload_is_verbatim_and_object_is_compact_json() {
        let json = r#"{ "messaging": {
            "local": { "host": "h", "port": 1883, "clientId": "c" },
            "lwt": { "topic": "t", "payload": "gone", "qos": 0 } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let lwt = mc.messaging.lwt.unwrap();
        assert_eq!(lwt.payload_bytes(), b"gone");
        assert_eq!(lwt.qos_or_default(), 0);

        let json = r#"{ "messaging": {
            "local": { "host": "h", "port": 1883, "clientId": "c" },
            "lwt": { "topic": "t", "payload": { "status": "UNREACHABLE" }, "qos": 1.0 } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let lwt = mc.messaging.lwt.unwrap();
        assert_eq!(lwt.payload_bytes(), br#"{"status":"UNREACHABLE"}"#);
        assert_eq!(lwt.qos_or_default(), 1, "number-tolerant qos (1.0 == 1)");
    }
}
