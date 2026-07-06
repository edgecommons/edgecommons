//! # Parameters config
//!
//! **One-liner purpose**: Parse the `parameters` config section into a [`ParametersConfig`] and
//! build a [`super::service::DefaultParameterService`] from it — selecting the [`ParameterSource`]
//! backend, choosing a source-aware cache (persistent-encrypted for remote sources, in-memory for
//! already-local ones), and optionally bootstrapping the declared names/paths into the cache.
//!
//! ## Overview
//! Phase 1 ships three sources: `awsSsm` (remote; behind the `parameters-aws` feature), `mountedDir`
//! (K8s ConfigMap/Secret volumes, Docker secrets), and `env`. Numeric fields parse leniently because
//! Greengrass delivers config numbers as doubles.
//!
//! The cache decision is **source-aware** (a remote source persists encrypted so values survive
//! restarts/offline; a local source uses memory because the backend is itself always available), but
//! `cache.persist` can override it explicitly.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Deserializer};

use super::service::{DefaultParameterService, ParameterService};
use super::source::{EnvSource, MountedDirSource, ParameterSource};
use crate::Result;
use crate::credentials::{KeyProviderConfig, LocalVault, build_key_provider};
use crate::error::EdgeCommonsError;

// Greengrass stores config numbers as doubles (e.g. 300.0). Accept an int or an integer-valued
// float for the numeric fields below.
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .ok_or_else(|| serde::de::Error::custom("expected a non-negative integer")),
        other => Err(serde::de::Error::custom(format!(
            "expected a number, got {other}"
        ))),
    }
}

/// The `parameters` config section.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ParametersConfig {
    pub source: ParamSourceConfig,
    pub cache: CacheConfig,
    #[serde(deserialize_with = "lenient_u64")]
    pub refresh_interval_secs: u64,
    pub bootstrap_on_start: bool,
    pub sync: ParamSyncSelect,
}

impl Default for ParametersConfig {
    fn default() -> Self {
        Self {
            source: ParamSourceConfig::default(),
            cache: CacheConfig::default(),
            refresh_interval_secs: 300,
            bootstrap_on_start: true,
            sync: ParamSyncSelect::default(),
        }
    }
}

/// Which parameter backend to read from, and its per-source settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ParamSourceConfig {
    /// `none` | `awsSsm` | `mountedDir` | `env`.
    #[serde(rename = "type")]
    pub kind: String,
    // ----- awsSsm -----
    pub region: Option<String>,
    /// Override the SSM endpoint (floci/LocalStack/VPC endpoint).
    pub endpoint_url: Option<String>,
    /// Decrypt `SecureString` parameters (flagging them `secure`). Default true.
    pub with_decryption: bool,
    // ----- mountedDir -----
    /// Root directory for the `mountedDir` source.
    pub root: Option<String>,
    /// Parameter-name prefixes whose values are sensitive (a Secret volume vs a ConfigMap volume).
    pub secure_paths: Vec<String>,
    // ----- env -----
    /// Env-var prefix for the `env` source (e.g. `GG_PARAM_`).
    pub prefix: Option<String>,
}

impl Default for ParamSourceConfig {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            region: None,
            endpoint_url: None,
            with_decryption: true,
            root: None,
            secure_paths: Vec::new(),
            prefix: None,
        }
    }
}

impl ParamSourceConfig {
    /// Whether this source is remote (network-backed) — drives the default cache persistence.
    fn is_remote(&self) -> bool {
        matches!(self.kind.as_str(), "awsSsm")
    }
}

/// Offline-first cache settings. The cache reuses the credentials [`LocalVault`] on-disk format when
/// persistent (the same normative, cross-language store).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CacheConfig {
    /// Force persistence on/off. Unset (the default) is source-aware: persist for remote sources,
    /// in-memory for already-local ones.
    pub persist: Option<bool>,
    /// On-disk path for the persistent cache vault.
    pub path: String,
    /// KEK custodian for the persistent cache (reuses the credentials key-provider config).
    pub key_provider: KeyProviderConfig,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            persist: None,
            path: "param-cache".to_string(),
            key_provider: KeyProviderConfig::default(),
        }
    }
}

/// Which parameters to pull on refresh (explicit names/paths — selective sync / least privilege).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ParamSyncSelect {
    pub names: Vec<String>,
    pub paths: Vec<PathEntry>,
}

/// One path to sync. Accepts a bare string (recursive) or `{ "path": ..., "recursive": <bool> }`.
#[derive(Debug, Clone)]
pub struct PathEntry {
    pub path: String,
    pub recursive: bool,
}

impl<'de> Deserialize<'de> for PathEntry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", untagged)]
        enum Raw {
            Str(String),
            Obj {
                path: String,
                #[serde(default = "default_true")]
                recursive: bool,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Str(path) => PathEntry {
                path,
                recursive: true,
            },
            Raw::Obj { path, recursive } => PathEntry { path, recursive },
        })
    }
}

fn default_true() -> bool {
    true
}

/// Build the [`ParameterSource`] backend named by `source.kind`.
fn build_source(source: &ParamSourceConfig) -> Result<Arc<dyn ParameterSource>> {
    match source.kind.as_str() {
        "env" => {
            let prefix = source
                .prefix
                .clone()
                .unwrap_or_else(|| "GG_PARAM_".to_string());
            Ok(Arc::new(EnvSource::new(prefix)))
        }
        "mountedDir" => {
            let root = source.root.clone().ok_or_else(|| {
                EdgeCommonsError::Parameters("mountedDir source requires source.root".into())
            })?;
            Ok(Arc::new(MountedDirSource::new(
                root,
                source.secure_paths.clone(),
            )))
        }
        #[cfg(feature = "parameters-aws")]
        "awsSsm" => {
            let s = super::ssm::AwsSsmSource::new(
                source.region.clone(),
                source.endpoint_url.clone(),
                source.with_decryption,
            )?;
            Ok(Arc::new(s))
        }
        other => Err(EdgeCommonsError::Parameters(format!(
            "parameter source '{other}' is not available (supported: 'env', 'mountedDir'; 'awsSsm' needs the parameters-aws feature)"
        ))),
    }
}

/// Build a [`DefaultParameterService`] from a parsed config.
///
/// # Purpose
/// Select the source backend, pick a source-aware cache (persistent-encrypted for remote sources,
/// in-memory for local ones — overridable via `cache.persist`), wire the declared sync names/paths,
/// and optionally bootstrap the cache from the source.
///
/// # Errors
/// | Error Variant | Condition | Recovery |
/// |---------------|-----------|----------|
/// | `EdgeCommonsError::Parameters` | Unknown source type, missing required source field, or vault open failure | Fix the `parameters` config section |
pub fn open(config: &ParametersConfig) -> Result<DefaultParameterService> {
    let source = build_source(&config.source)?;
    let sync_names = config.sync.names.clone();
    let sync_paths: Vec<(String, bool)> = config
        .sync
        .paths
        .iter()
        .map(|p| (p.path.clone(), p.recursive))
        .collect();

    // Source-aware default: remote sources persist encrypted (survive restart/offline); local
    // sources stay in memory (the backend is itself always available). `cache.persist` overrides.
    let persist = config
        .cache
        .persist
        .unwrap_or_else(|| config.source.is_remote());

    let service = if persist {
        // The parameter cache has no platform-profile KEK default (FR-CRED-6 applies only to the
        // credentials vault) — pass `None` so the library default `file` is preserved.
        let provider = build_key_provider(
            &config.cache.key_provider,
            &format!("{}.key", config.cache.path),
            None,
        )?;
        // keep_versions = 1: the cache only ever needs the latest value of each parameter.
        let vault = LocalVault::open(&config.cache.path, provider, 1)?;
        let vault = Arc::new(Mutex::new(vault));
        DefaultParameterService::with_persistent_cache(source, vault, sync_names, sync_paths)
    } else {
        DefaultParameterService::with_memory_cache(source, sync_names, sync_paths)
    };

    if config.bootstrap_on_start {
        // Offline-first: a bootstrap failure is non-fatal — the component starts and can retry via
        // refresh(). A persisted cache from a prior run still serves reads while the source is down.
        if let Err(e) = service.refresh() {
            tracing::warn!(error = %e, "parameter bootstrap refresh failed (continuing; cache may be empty)");
        }
    }

    // Background refresh on the configured interval (0 disables; the thread stops on drop).
    Ok(service.with_refresh(config.refresh_interval_secs))
}
