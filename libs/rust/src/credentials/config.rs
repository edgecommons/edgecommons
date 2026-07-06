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

use super::keyprovider::{EnvKeyProvider, FileKeyProvider, DEFAULT_KEK_ENV_VAR};
use super::service::DefaultCredentialService;
use super::vault::LocalVault;
use crate::error::EdgeCommonsError;
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
    pub audit: AuditConfig,
}

/// Credential access-audit settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuditConfig {
    /// Emit access events (op/name/version/source/outcome, never the value) to the audit log.
    /// On by default — a secrets subsystem should record access; set `false` to silence it.
    pub enabled: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
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

/// KEK custodian selection (`file` | `env` | `kms` | `greengrass` | `pkcs11`).
///
/// `kind` (the `type` field) is `Option`: `None` means *unspecified*, which lets the credentials
/// init site distinguish "explicitly `file`" from "absent" so it can apply the platform-profile
/// default (env on KUBERNETES, FR-CRED-6) before falling back to the library default `file`
/// ([`build_key_provider`]). An explicit `type` always wins.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyProviderConfig {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub key_path: Option<String>,
    /// env: name of the env var holding the base64-encoded 32-byte KEK (default
    /// [`keyprovider::DEFAULT_KEK_ENV_VAR`](super::keyprovider::DEFAULT_KEK_ENV_VAR),
    /// i.e. `EDGECOMMONS_VAULT_KEK`).
    pub env_var: Option<String>,
    pub kms_key_id: Option<String>,
    pub region: Option<String>,
    /// Override the KMS endpoint (floci/LocalStack/VPC endpoint).
    pub endpoint_url: Option<String>,
    // ----- pkcs11 (HSM/TPM/SoftHSM) custodian -----
    /// Path to the PKCS#11 module (e.g. `/usr/lib/softhsm/libsofthsm2.so`).
    pub module_path: Option<String>,
    /// Token label to select the slot.
    pub token_label: Option<String>,
    /// Label of the AES KEK object on the token.
    pub key_label: Option<String>,
    /// Env var holding the User PIN (preferred over an inline `pin`).
    pub pin_env: Option<String>,
    /// Inline User PIN (discouraged — prefer `pinEnv`).
    pub pin: Option<String>,
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

/// Build a KEK custodian from a [`KeyProviderConfig`] (shared by the vault and the parameter
/// cache). `default_key_path` is used for the `file` provider when `keyPath` is absent.
///
/// `default_kind` is the platform-profile default custodian type applied when
/// `keyProvider.type` is unspecified (FR-CRED-6, precedence FR-RT-3): the EFFECTIVE type is
/// `explicit keyProvider.type ▸ default_kind ▸ "file"`. Callers without a platform default
/// (e.g. the parameter cache) pass `None`, preserving the library default `file`.
pub(crate) fn build_key_provider(
    kp: &KeyProviderConfig,
    default_key_path: &str,
    default_kind: Option<&str>,
) -> Result<std::sync::Arc<dyn super::keyprovider::KeyProvider>> {
    let kind = kp.kind.as_deref().or(default_kind).unwrap_or("file");
    match kind {
        "file" => {
            let key_path = kp.key_path.clone().unwrap_or_else(|| default_key_path.to_string());
            if let Some(dir) = std::path::Path::new(&key_path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let fp = if std::path::Path::new(&key_path).exists() {
                FileKeyProvider::from_keyfile(&key_path)?
            } else {
                FileKeyProvider::generate_keyfile(&key_path)?
            };
            Ok(std::sync::Arc::new(fp))
        }
        "env" => {
            // Software-KEK from an env var (typically a mounted k8s Secret). Cryptographically
            // identical to `file` given the same raw KEK; the env var NAME defaults to
            // EDGECOMMONS_VAULT_KEK when `keyProvider.envVar` is absent (FR-CRED-3).
            let env_var = kp.env_var.as_deref().unwrap_or(DEFAULT_KEK_ENV_VAR);
            let p = EnvKeyProvider::from_env(env_var)?;
            Ok(std::sync::Arc::new(p))
        }
        #[cfg(feature = "credentials-aws")]
        "kms" | "greengrass" => {
            let key_id = kp
                .kms_key_id
                .clone()
                .ok_or_else(|| EdgeCommonsError::Credentials("kms key provider requires keyProvider.kmsKeyId".to_string()))?;
            let p = super::keyprovider::KmsKeyProvider::new(key_id, kp.region.clone(), kp.endpoint_url.clone())?;
            Ok(std::sync::Arc::new(p))
        }
        #[cfg(feature = "credentials-pkcs11")]
        "pkcs11" => {
            let module_path = kp
                .module_path
                .clone()
                .ok_or_else(|| EdgeCommonsError::Credentials("pkcs11 key provider requires keyProvider.modulePath".into()))?;
            let token_label = kp
                .token_label
                .clone()
                .ok_or_else(|| EdgeCommonsError::Credentials("pkcs11 key provider requires keyProvider.tokenLabel".into()))?;
            let key_label = kp
                .key_label
                .clone()
                .ok_or_else(|| EdgeCommonsError::Credentials("pkcs11 key provider requires keyProvider.keyLabel".into()))?;
            let pin = match (&kp.pin_env, &kp.pin) {
                (Some(env), _) => std::env::var(env)
                    .map_err(|_| EdgeCommonsError::Credentials(format!("pkcs11 keyProvider.pinEnv '{env}' is not set")))?,
                (None, Some(p)) => p.clone(),
                (None, None) => {
                    return Err(EdgeCommonsError::Credentials(
                        "pkcs11 key provider requires keyProvider.pinEnv or keyProvider.pin".into(),
                    ))
                }
            };
            let p = super::keyprovider::Pkcs11KeyProvider::new(&module_path, &token_label, key_label, pin)?;
            Ok(std::sync::Arc::new(p))
        }
        other => Err(EdgeCommonsError::Credentials(format!(
            "key provider '{other}' is not available (supported: 'file', 'env'; 'kms'/'greengrass' need the credentials-aws feature; 'pkcs11' needs the credentials-pkcs11 feature)"
        ))),
    }
}

/// Open a vault and build the default credential service from a parsed config (no namespacing).
pub fn open(config: &CredentialsConfig) -> Result<DefaultCredentialService> {
    open_namespaced(config, "")
}

/// As [`open`], but transparently namespacing every key under `namespace` (typically
/// `<thingName>/<componentName>`) so a shared device vault / fleet central store can't collide.
///
/// Uses the library default KEK custodian (`file`) when `keyProvider.type` is unspecified. The
/// runtime builder calls [`open_namespaced_with_default`] to apply the platform-profile default
/// (env on KUBERNETES, FR-CRED-6).
pub fn open_namespaced(config: &CredentialsConfig, namespace: &str) -> Result<DefaultCredentialService> {
    open_namespaced_with_default(config, namespace, None)
}

/// As [`open_namespaced`], but applying `default_kind` as the KEK custodian when
/// `keyProvider.type` is unspecified (FR-CRED-6, precedence FR-RT-3): the effective type is
/// `explicit keyProvider.type ▸ default_kind ▸ "file"`. The runtime builder threads the resolved
/// platform's [`crate::platform::profile_credentials_key_provider`] here (env on KUBERNETES). This
/// only changes the default provider *type*; it does **not** enable credentials (the caller opens a
/// vault only when a `credentials` config section is present).
pub fn open_namespaced_with_default(
    config: &CredentialsConfig,
    namespace: &str,
    default_kind: Option<&str>,
) -> Result<DefaultCredentialService> {
    let provider =
        build_key_provider(&config.vault.key_provider, &format!("{}.key", config.vault.path), default_kind)?;

    let vault = LocalVault::open(&config.vault.path, provider, config.vault.keep_versions)?;

    let service = match config.central.kind.as_str() {
        "none" => {
            let shared = std::sync::Arc::new(std::sync::Mutex::new(vault));
            DefaultCredentialService::with_sync(shared, None, namespace.to_string())
        }
        "awsSecretsManager" => open_central(vault, &config.central, namespace)?,
        other => return Err(EdgeCommonsError::Credentials(format!("central source '{other}' is not supported"))),
    };

    // Access auditing on by default (config can disable) — logs op/name/version/source/outcome,
    // never the value.
    let audit = config.audit.enabled.then(super::audit::log_sink);
    Ok(service.with_audit(audit))
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
    Err(EdgeCommonsError::Credentials(
        "central source 'awsSecretsManager' requires the 'credentials-aws' feature".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::CredentialService;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;

    #[test]
    fn build_key_provider_file_generates_then_loads_a_keyfile() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("vault.key");
        let kp = KeyProviderConfig {
            kind: Some("file".to_string()),
            key_path: Some(key_path.to_string_lossy().into_owned()),
            ..KeyProviderConfig::default()
        };
        // First call generates the keyfile; the second loads the same one (both arms covered).
        let _p1 = build_key_provider(&kp, "ignored.key", None).expect("generate keyfile");
        assert!(key_path.exists(), "the file provider must create its keyfile");
        let _p2 = build_key_provider(&kp, "ignored.key", None).expect("load existing keyfile");
    }

    #[test]
    fn build_key_provider_default_kind_applies_when_type_is_unspecified() {
        // Unspecified type + a platform default of "file" → the file provider (FR-CRED-6).
        let dir = tempfile::tempdir().unwrap();
        let kp = KeyProviderConfig::default();
        let default_key = dir.path().join("default.key");
        let _ = build_key_provider(&kp, &default_key.to_string_lossy(), Some("file")).expect("file default");
        assert!(default_key.exists(), "the default key path is used when keyPath is absent");
    }

    #[test]
    fn build_key_provider_env_reads_the_kek_and_errors_when_missing() {
        let var = "EDGECOMMONS_TEST_KEK_CFG";
        // SAFETY: a test-only env var with a unique name; no other test reads/writes it.
        unsafe { std::env::set_var(var, B64.encode([9u8; 32])) };
        let kp = KeyProviderConfig {
            kind: Some("env".to_string()),
            env_var: Some(var.to_string()),
            ..KeyProviderConfig::default()
        };
        assert!(build_key_provider(&kp, "n/a", None).is_ok());
        unsafe { std::env::remove_var(var) };
        assert!(
            build_key_provider(&kp, "n/a", None).is_err(),
            "a missing env KEK must be a hard error"
        );
    }

    #[test]
    fn build_key_provider_rejects_unknown_kind() {
        let kp = KeyProviderConfig {
            kind: Some("nonsense".to_string()),
            ..KeyProviderConfig::default()
        };
        let err = match build_key_provider(&kp, "n/a", None) {
            Err(e) => e,
            Ok(_) => panic!("an unknown key provider kind must error"),
        };
        assert!(format!("{err}").contains("nonsense"), "error names the bad provider: {err}");
    }

    #[test]
    fn open_namespaced_opens_a_local_vault_with_audit_on_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = CredentialsConfig {
            vault: VaultConfig {
                path: dir.path().join("v").to_string_lossy().into_owned(),
                ..VaultConfig::default()
            },
            ..CredentialsConfig::default()
        };
        let svc = open_namespaced_with_default(&cfg, "thing/comp", None).expect("open local vault");
        svc.put("api/token", b"xyz", super::super::vault::PutOptions::default()).unwrap();
        assert_eq!(svc.get_string("api/token").unwrap().unwrap(), "xyz");
        // The namespace is transparent: the caller sees the bare key, not "thing/comp/api/token".
        let names: Vec<_> = svc.list("").unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["api/token".to_string()]);
    }

    #[test]
    fn open_rejects_unknown_central_source() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = CredentialsConfig {
            vault: VaultConfig {
                path: dir.path().join("v").to_string_lossy().into_owned(),
                ..VaultConfig::default()
            },
            central: CentralConfig { kind: "mysteryCloud".to_string(), ..CentralConfig::default() },
            ..CredentialsConfig::default()
        };
        assert!(open(&cfg).is_err(), "an unknown central.type must be rejected");
    }

    #[test]
    fn awssecretsmanager_central_requires_the_aws_feature() {
        // Without `credentials-aws` the awsSecretsManager source is unavailable and must error
        // (rather than silently degrade). This is the non-AWS `open_central` stub.
        let dir = tempfile::tempdir().unwrap();
        let cfg = CredentialsConfig {
            vault: VaultConfig {
                path: dir.path().join("v").to_string_lossy().into_owned(),
                ..VaultConfig::default()
            },
            central: CentralConfig { kind: "awsSecretsManager".to_string(), ..CentralConfig::default() },
            ..CredentialsConfig::default()
        };
        let result = open(&cfg);
        #[cfg(not(feature = "credentials-aws"))]
        assert!(result.is_err(), "awsSecretsManager needs the credentials-aws feature");
        // (With the feature on, opening may instead fail later on AWS config — not asserted here.)
        let _ = result;
    }

    #[test]
    fn sync_entry_accepts_bare_string_or_object_form() {
        let bare: SyncEntry = serde_json::from_value(serde_json::json!("db/password")).unwrap();
        assert_eq!(bare.name, "db/password");
        assert!(bare.from.is_none());

        let obj: SyncEntry =
            serde_json::from_value(serde_json::json!({ "name": "tls", "from": "fleet/tls" })).unwrap();
        assert_eq!(obj.name, "tls");
        assert_eq!(obj.from.as_deref(), Some("fleet/tls"));
    }

    #[test]
    fn numeric_fields_parse_leniently_from_greengrass_doubles() {
        // Greengrass delivers config numbers as doubles (e.g. 5.0); they must still parse to ints.
        let cfg: CredentialsConfig = serde_json::from_value(serde_json::json!({
            "vault": { "keepVersions": 5.0, "cacheTtlSecs": 120.0 },
            "central": { "refreshIntervalSecs": 30.0 }
        }))
        .unwrap();
        assert_eq!(cfg.vault.keep_versions, 5);
        assert_eq!(cfg.vault.cache_ttl_secs, 120);
        assert_eq!(cfg.central.refresh_interval_secs, 30);
    }
}
