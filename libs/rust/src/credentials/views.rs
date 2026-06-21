//! # Typed credential views
//!
//! **One-liner purpose**: Thin, well-known JSON shapes layered over an opaque [`super::Secret`] —
//! AWS credentials, HTTP basic auth, a TLS bundle, and Kafka SASL. The vault still stores opaque
//! bytes; these are convenience parses ([`super::CredentialService`] exposes `get_*` accessors).
//!
//! ## Canonical JSON (camelCase; the cross-language contract)
//! - AWS creds: `{ "accessKeyId", "secretAccessKey", "sessionToken"?, "expiry"? }`
//! - basic auth: `{ "username", "password" }`
//! - TLS bundle: `{ "certPem", "keyPem", "caPem"? }`
//! - Kafka SASL: `{ "mechanism"?, "username", "password" }` (mechanism defaults to `PLAIN`)

use serde::Deserialize;

use super::service::Secret;
use crate::error::GgError;
use crate::Result;

/// Parse a secret's bytes into a typed view `T`, with a non-sensitive error naming the expectation.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(secret: &Secret, kind: &str) -> Result<T> {
    serde_json::from_slice(secret.bytes())
        .map_err(|e| GgError::Credentials(format!("secret '{}' is not {kind} JSON: {e}", secret.name)))
}

/// AWS credentials stored as a secret.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default)]
    pub session_token: Option<String>,
    /// RFC3339 expiry, when the secret carries temporary credentials.
    #[serde(default)]
    pub expiry: Option<String>,
}

/// HTTP basic auth (username/password).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

/// A TLS bundle (PEM strings).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsBundle {
    pub cert_pem: String,
    pub key_pem: String,
    #[serde(default)]
    pub ca_pem: Option<String>,
}

/// Kafka SASL credentials.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KafkaSasl {
    #[serde(default = "default_mechanism")]
    pub mechanism: String,
    pub username: String,
    pub password: String,
}

fn default_mechanism() -> String {
    "PLAIN".to_string()
}
