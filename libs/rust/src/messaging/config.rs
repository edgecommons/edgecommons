//! # Messaging — standalone configuration
//!
//! **One-liner purpose**: Parse the `-m STANDALONE <path>` messaging config file
//! describing the local broker and (optionally) a generic northbound MQTT broker.
//!
//! ## Overview
//! The file shape matches the Java/Python `standalone-messaging-sample.json`:
//!
//! ```json
//! { "messaging": {
//!     "local":   { "host": "localhost", "port": 1883, "clientId": "c-local",
//!                  "qos": { "publish": 1, "subscribe": 1 },
//!                  "credentials": { "username": "u", "password": "p" } },
//!     "northbound": { "host": "...", "port": 8883, "clientId": "c-north",
//!                  "qos": { "publish": 1, "subscribe": 1 },
//!                  "credentials": { "certPath": "...", "keyPath": "...", "caPath": "..." } } } }
//! ```
//!
//! ## Semantics & Architecture
//! - Pure deserialization (`serde`); the only I/O is reading the file in [`MessagingConfig::load`].
//! - `northbound` is optional. It is a normal MQTT broker connection; TLS/auth are selected
//!   from its credentials block the same way as `local`.
//! - Error handling: [`crate::error::Result`]; no panics.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo() -> edgecommons::Result<()> {
//! use edgecommons::messaging::config::MessagingConfig;
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

use crate::error::{EdgeCommonsError, Result};

/// Top-level standalone messaging configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct MessagingConfig {
    pub messaging: Messaging,
}

/// The `messaging` object: a required local broker and an optional northbound broker.
#[derive(Debug, Clone, Deserialize)]
pub struct Messaging {
    pub local: BrokerConfig,
    #[serde(default)]
    pub northbound: Option<BrokerConfig>,
}

/// Effective QoS defaults aggregated from `messaging.local.qos` and
/// `messaging.northbound.qos`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QosConfig {
    /// Local MQTT broker defaults. Supports QoS 0/1/2.
    #[serde(default)]
    pub local: QosDefaults,
    /// Northbound MQTT broker defaults. Supports QoS 0/1/2.
    #[serde(default)]
    pub northbound: QosDefaults,
}

impl Default for QosConfig {
    fn default() -> Self {
        Self {
            local: QosDefaults::default(),
            northbound: QosDefaults::default(),
        }
    }
}

impl Messaging {
    pub fn qos_config(&self) -> QosConfig {
        QosConfig {
            local: self.local.qos.clone(),
            northbound: self
                .northbound
                .as_ref()
                .map(|broker| broker.qos.clone())
                .unwrap_or_default(),
        }
    }
}

/// Publish/subscribe QoS defaults for one broker side.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QosDefaults {
    #[serde(default = "default_qos_one")]
    pub publish: u8,
    #[serde(default = "default_qos_one")]
    pub subscribe: u8,
}

impl Default for QosDefaults {
    fn default() -> Self {
        Self {
            publish: 1,
            subscribe: 1,
        }
    }
}

fn default_qos_one() -> u8 {
    1
}

impl QosConfig {
    /// Validate local and northbound values are in 0..=2.
    ///
    /// # Errors
    /// Returns a config error when a configured value exceeds the broker-side domain.
    pub fn validate(&self) -> Result<()> {
        validate_qos(self.local.publish, 2, "messaging.local.qos.publish")?;
        validate_qos(self.local.subscribe, 2, "messaging.local.qos.subscribe")?;
        validate_qos(
            self.northbound.publish,
            2,
            "messaging.northbound.qos.publish",
        )?;
        validate_qos(
            self.northbound.subscribe,
            2,
            "messaging.northbound.qos.subscribe",
        )?;
        Ok(())
    }
}

fn validate_qos(value: u8, max: u8, field: &str) -> Result<()> {
    if value > max {
        return Err(crate::error::EdgeCommonsError::Config(format!(
            "{field} must be 0..{max} (got {value})"
        )));
    }
    Ok(())
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
    pub qos: QosDefaults,
    #[serde(default)]
    pub credentials: Option<Credentials>,
}

impl BrokerConfig {
    /// The broker host, preferring `host` then `endpoint`.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `EdgeCommonsError::Config` | Neither `host` nor `endpoint` is set | Add one to the messaging config |
    pub fn resolved_host(&self) -> Result<&str> {
        self.host
            .as_deref()
            .or(self.endpoint.as_deref())
            .ok_or_else(|| {
                crate::error::EdgeCommonsError::Config(
                    "broker config has no host/endpoint".to_string(),
                )
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
    /// | `EdgeCommonsError::Io` | File missing or unreadable | Check the `-m STANDALONE <path>` argument |
    /// | `EdgeCommonsError::Json` | File is not valid messaging JSON | Fix the file shape |
    pub async fn load(path: impl AsRef<Path>) -> Result<MessagingConfig> {
        let bytes = tokio::fs::read(path.as_ref()).await?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if value
            .get("messaging")
            .and_then(|messaging| messaging.get("lwt"))
            .is_some()
        {
            return Err(EdgeCommonsError::Config(
                "messaging.lwt is not supported; uns-bridge derives its site Last-Will internally"
                    .to_string(),
            ));
        }
        if value
            .get("messaging")
            .and_then(|messaging| messaging.get("qos"))
            .is_some()
        {
            return Err(EdgeCommonsError::Config(
                "messaging.qos is not supported; configure QoS under messaging.local.qos and messaging.northbound.qos"
                    .to_string(),
            ));
        }
        let config: MessagingConfig = serde_json::from_value(value)?;
        config.messaging.qos_config().validate()?;
        Ok(config)
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
        assert!(mc.messaging.northbound.is_none());
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
    fn single_broker_topology_when_northbound_absent() {
        // FR-MSG-3: no `northbound` section => single-broker (local only / air-gapped).
        let json = r#"{ "messaging": { "local": {
            "host": "emqx.mqtt.svc.cluster.local", "port": 1883, "clientId": "c" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert!(
            mc.messaging.northbound.is_none(),
            "absent northbound => single-broker topology"
        );
    }

    #[test]
    fn dual_broker_topology_when_northbound_present() {
        // FR-MSG-3: a `northbound` section => dual-MQTT (local broker + northbound MQTT).
        let json = r#"{ "messaging": {
            "local": { "host": "emqx.mqtt.svc.cluster.local", "port": 1883, "clientId": "l" },
            "northbound": { "host": "broker.example.com", "port": 8883, "clientId": "n",
                         "credentials": { "certPath": "c.pem", "keyPath": "k.pem", "caPath": "ca.pem" } } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        assert!(
            mc.messaging.northbound.is_some(),
            "present northbound => dual-broker topology"
        );
        assert_eq!(
            mc.messaging.northbound.unwrap().resolved_host().unwrap(),
            "broker.example.com"
        );
    }

    #[test]
    fn parses_qos_defaults() {
        let json = r#"{ "messaging": {
            "local": { "host": "localhost", "port": 1883, "clientId": "l",
                       "qos": { "publish": 2, "subscribe": 0 } },
            "northbound": { "host": "broker.example.com", "port": 8883, "clientId": "n",
                            "qos": { "publish": 2, "subscribe": 0 } } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let qos = mc.messaging.qos_config();
        assert_eq!(qos.local.publish, 2);
        assert_eq!(qos.local.subscribe, 0);
        assert_eq!(qos.northbound.publish, 2);
        assert_eq!(qos.northbound.subscribe, 0);
        qos.validate().unwrap();
    }

    #[test]
    fn qos_defaults_to_one_and_rejects_out_of_range_northbound_qos() {
        let json = r#"{ "messaging": { "local": {
            "host": "localhost", "port": 1883, "clientId": "l" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let qos = mc.messaging.qos_config();
        assert_eq!(qos.local.publish, 1);
        assert_eq!(qos.local.subscribe, 1);
        assert_eq!(qos.northbound.publish, 1);
        assert_eq!(qos.northbound.subscribe, 1);

        let invalid = r#"{ "messaging": {
            "local": { "host": "localhost", "port": 1883, "clientId": "l" },
            "northbound": { "host": "broker.example.com", "port": 8883, "clientId": "n",
                            "qos": { "publish": 3 } } } }"#;
        let mc: MessagingConfig = serde_json::from_str(invalid).unwrap();
        assert!(mc.messaging.qos_config().validate().is_err());
    }

    #[test]
    fn parses_northbound_endpoint_alias() {
        let json = r#"{ "messaging": {
            "local": { "host": "localhost", "port": 1883, "clientId": "l" },
            "northbound": { "endpoint": "broker.example.com", "port": 8883, "clientId": "n" } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let northbound = mc.messaging.northbound.unwrap();
        assert_eq!(northbound.resolved_host().unwrap(), "broker.example.com");
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
            "northbound": { "endpoint": "e", "port": 8883, "clientId": "n",
                         "credentials": { "certPath": "c.pem", "keyPath": "k.pem", "caPath": "ca.pem" } } } }"#;
        let mc: MessagingConfig = serde_json::from_str(json).unwrap();
        let local_creds = mc.messaging.local.credentials.unwrap();
        assert_eq!(local_creds.username.as_deref(), Some("u"));
        assert_eq!(local_creds.password.as_deref(), Some("p"));
        let northbound_creds = mc.messaging.northbound.unwrap().credentials.unwrap();
        assert_eq!(northbound_creds.cert_path.as_deref(), Some("c.pem"));
        assert_eq!(northbound_creds.key_path.as_deref(), Some("k.pem"));
        assert_eq!(northbound_creds.ca_path.as_deref(), Some("ca.pem"));
    }

    #[tokio::test]
    async fn load_reads_and_parses_a_file() {
        let dir = std::env::temp_dir().join(format!("edgecommons-mc-{}", uuid::Uuid::new_v4()));
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

    #[tokio::test]
    async fn load_rejects_generic_messaging_lwt() {
        let dir = std::env::temp_dir().join(format!("edgecommons-mc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("messaging.json");
        std::fs::write(
            &path,
            r#"{ "messaging": {
                "local": { "host": "localhost", "port": 1884, "clientId": "c" },
                "lwt": { "topic": "ecv1/d/uns-bridge/main/state" }
            } }"#,
        )
        .unwrap();

        let err = MessagingConfig::load(&path).await.unwrap_err();
        assert!(err.to_string().contains("messaging.lwt is not supported"));
    }

    #[tokio::test]
    async fn load_rejects_top_level_messaging_qos() {
        let dir = std::env::temp_dir().join(format!("edgecommons-mc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("messaging.json");
        std::fs::write(
            &path,
            r#"{ "messaging": {
                "local": { "host": "localhost", "port": 1884, "clientId": "c" },
                "qos": { "local": { "publish": 1 } }
            } }"#,
        )
        .unwrap();

        let err = MessagingConfig::load(&path).await.unwrap_err();
        assert!(err.to_string().contains("messaging.qos is not supported"));
    }
}
