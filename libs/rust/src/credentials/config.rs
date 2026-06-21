//! # Credentials config
//!
//! **One-liner purpose**: Parse the `credentials` config section into a [`CredentialsConfig`] and
//! build a [`super::service::DefaultCredentialService`] from it — opening the local vault and, when
//! a central source is configured, starting the sync engine.
//!
//! ## Overview
//! Phase 1 ships the `file` key provider; phase 2 adds the `awsSecretsManager` central source
//! (behind the `credentials-aws` feature). Numeric fields parse leniently because Greengrass
//! delivers config numbers as doubles.

use serde::{Deserialize, Deserializer};

use super::keyprovider::FileKeyProvider;
use super::service::DefaultCredentialService;
use super::vault::LocalVault;
use crate::error::GgError;
use crate::Result;

// Greengrass stores config numbers as doubles (e.g. 300.0). Accept an int or an integer-valued
// float for the numeric fields below.
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        other => Err(serde::de::Error::custom(format!("expected a number, got {other}"))),
    }
}

fn lenient_usize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<usize, D::Error> {
    lenient_u64(d).map(|v| v as usize)
}

/// The `credentials` config section.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CredentialsConfig {
    pub vault: VaultConfig,
    pub central: CentralConfig,
}

/// Local vault settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VaultConfig {
    pub path: String,
    pub key_provider: KeyProviderConfig,
    #[serde(deserialize_with = "lenient_usize")]
    pub keep_versions: usize,
    #[serde(deserialize_with = "lenient_u64")]
    pub cache_ttl_secs: u64,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            path: "vault".to_string(),
            key_provider: KeyProviderConfig::default(),
            keep_versions: 2,
            cache_ttl_secs: 300,
        }
    }
}

/// KEK custodian selection. Only `file` is implemented so far.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyProviderConfig {
    #[serde(rename = "type")]
    pub kind: String,
    pub key_path: Option<String>,
    pub kms_key_id: Option<String>,
    pub region: Option<String>,
}

impl Default for KeyProviderConfig {
    fn default() -> Self {
        Self { kind: "file".to_string(), key_path: None, kms_key_id: None, region: None }
    }
}

/// Central upstream source + sync settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CentralConfig {
    /// `none` | `awsSecretsManager`.
    #[serde(rename = "type")]
    pub kind: String,
    pub region: Option<String>,
    /// Override the Secrets Manager endpoint (floci/LocalStack/VPC endpoint).
    pub endpoint_url: Option<String>,
    #[serde(deserialize_with = "lenient_u64")]
    pub refresh_interval_secs: u64,
    pub bootstrap_on_start: bool,
    pub sync: SyncSelect,
}

impl Default for CentralConfig {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            region: None,
            endpoint_url: None,
            refresh_interval_secs: 300,
            bootstrap_on_start: true,
            sync: SyncSelect::default(),
        }
    }
}

/// Which secrets to sync (explicit names — selective sync / least privilege).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncSelect {
    pub secrets: Vec<SyncEntry>,
}

/// One secret to sync. Accepts a bare string (the caller-facing name; its central id defaults to
/// the namespaced path — a per-device secret) or `{ "name": ..., "from": <central id> }` to point
/// at a shared/fleet secret id that bypasses the auto-namespace.
#[derive(Debug, Clone)]
pub struct SyncEntry {
    pub name: String,
    pub from: Option<String>,
}

impl<'de> Deserialize<'de> for SyncEntry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Str(String),
            Obj {
                name: String,
                #[serde(default)]
                from: Option<String>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Str(name) => SyncEntry { name, from: None },
            Raw::Obj { name, from } => SyncEntry { name, from },
        })
    }
}

/// Open a vault and build the default credential service from a parsed config (no namespacing).
pub fn open(config: &CredentialsConfig) -> Result<DefaultCredentialService> {
    open_namespaced(config, "")
}

/// As [`open`], but transparently namespacing every key under `namespace` (typically
/// `<thingName>/<componentName>`) so a shared device vault / fleet central store can't collide.
pub fn open_namespaced(config: &CredentialsConfig, namespace: &str) -> Result<DefaultCredentialService> {
    let provider = match config.vault.key_provider.kind.as_str() {
        "file" => {
            let key_path = config
                .vault
                .key_provider
                .key_path
                .clone()
                .unwrap_or_else(|| format!("{}.key", config.vault.path));
            if let Some(dir) = std::path::Path::new(&key_path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let fp = if std::path::Path::new(&key_path).exists() {
                FileKeyProvider::from_keyfile(&key_path)?
            } else {
                FileKeyProvider::generate_keyfile(&key_path)?
            };
            std::sync::Arc::new(fp) as std::sync::Arc<dyn super::keyprovider::KeyProvider>
        }
        other => {
            return Err(GgError::Credentials(format!(
                "key provider '{other}' is not implemented yet (supported: 'file')"
            )))
        }
    };

    let vault = LocalVault::open(&config.vault.path, provider, config.vault.keep_versions)?;

    match config.central.kind.as_str() {
        "none" => {
            let shared = std::sync::Arc::new(std::sync::Mutex::new(vault));
            Ok(DefaultCredentialService::with_sync(shared, None, namespace.to_string()))
        }
        "awsSecretsManager" => open_central(vault, &config.central, namespace),
        other => Err(GgError::Credentials(format!("central source '{other}' is not supported"))),
    }
}

#[cfg(feature = "credentials-aws")]
fn open_central(vault: LocalVault, central: &CentralConfig, namespace: &str) -> Result<DefaultCredentialService> {
    use super::central::{AwsSecretsManagerSource, CentralVaultSource};
    use super::sync::SyncEngine;
    use std::sync::{Arc, Mutex};

    let source: Arc<dyn CentralVaultSource> =
        Arc::new(AwsSecretsManagerSource::new(central.region.clone(), central.endpoint_url.clone())?);
    let vault = Arc::new(Mutex::new(vault));
    let secrets: Vec<(String, Option<String>)> =
        central.sync.secrets.iter().map(|e| (e.name.clone(), e.from.clone())).collect();
    let sync = SyncEngine::start(
        vault.clone(),
        source,
        namespace.to_string(),
        secrets,
        central.refresh_interval_secs,
        central.bootstrap_on_start,
    );
    Ok(DefaultCredentialService::with_sync(vault, Some(sync), namespace.to_string()))
}

#[cfg(not(feature = "credentials-aws"))]
fn open_central(_vault: LocalVault, _central: &CentralConfig, _namespace: &str) -> Result<DefaultCredentialService> {
    Err(GgError::Credentials(
        "central source 'awsSecretsManager' requires the 'credentials-aws' feature".to_string(),
    ))
}
