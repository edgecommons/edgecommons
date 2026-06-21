//! # Credentials config
//!
//! **One-liner purpose**: Parse the `credentials` config section into a [`CredentialsConfig`] and
//! build a [`super::service::DefaultCredentialService`] from it.
//!
//! ## Overview
//! Phase 1 supports the `file` key provider and `central.type: none` (local-only vault). Other key
//! providers (`kms`/`greengrass`/`pkcs11`) and central sources are accepted in the schema but
//! return a clear "not yet implemented" error until their phase lands.

use std::sync::Arc;

use serde::Deserialize;

use super::keyprovider::FileKeyProvider;
use super::service::DefaultCredentialService;
use super::vault::LocalVault;
use crate::error::GgError;
use crate::Result;

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
    /// Shared device vault path (default below is a per-OS-agnostic placeholder; resolve via
    /// config templates / set explicitly in the recipe).
    pub path: String,
    pub key_provider: KeyProviderConfig,
    pub keep_versions: usize,
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

/// KEK custodian selection. Only `file` is implemented in phase 1.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyProviderConfig {
    /// `file` | `env` | `kms` | `greengrass` | `pkcs11`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `file`: path to the 32-byte key file (generated if absent).
    pub key_path: Option<String>,
    /// `kms`/`greengrass`: CMK id/arn.
    pub kms_key_id: Option<String>,
    pub region: Option<String>,
}

impl Default for KeyProviderConfig {
    fn default() -> Self {
        Self { kind: "file".to_string(), key_path: None, kms_key_id: None, region: None }
    }
}

/// Central upstream source. Phase 1 only supports `none`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CentralConfig {
    /// `none` | `awsSecretsManager` | `awsSsm`.
    #[serde(rename = "type")]
    pub kind: String,
}

impl Default for CentralConfig {
    fn default() -> Self {
        Self { kind: "none".to_string() }
    }
}

/// Open a vault and build the default credential service from a parsed config.
///
/// # Errors
/// `GgError::Credentials` for an unimplemented key provider / central source, or any vault open
/// failure (bad key, integrity check, I/O).
pub fn open(config: &CredentialsConfig) -> Result<DefaultCredentialService> {
    let provider = match config.vault.key_provider.kind.as_str() {
        "file" => {
            let key_path = config
                .vault
                .key_provider
                .key_path
                .clone()
                .unwrap_or_else(|| format!("{}.key", config.vault.path));
            let fp = if std::path::Path::new(&key_path).exists() {
                FileKeyProvider::from_keyfile(&key_path)?
            } else {
                if let Some(dir) = std::path::Path::new(&key_path).parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                FileKeyProvider::generate_keyfile(&key_path)?
            };
            Arc::new(fp) as Arc<dyn super::keyprovider::KeyProvider>
        }
        other => {
            return Err(GgError::Credentials(format!(
                "key provider '{other}' is not implemented yet (phase 1 supports 'file')"
            )))
        }
    };

    if config.central.kind != "none" {
        return Err(GgError::Credentials(format!(
            "central source '{}' is not implemented yet (phase 2)",
            config.central.kind
        )));
    }

    let vault = LocalVault::open(&config.vault.path, provider, config.vault.keep_versions)?;
    Ok(DefaultCredentialService::new(vault))
}
