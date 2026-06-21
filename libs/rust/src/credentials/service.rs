//! # Credential service (the testable seam)
//!
//! **One-liner purpose**: The public, transport-agnostic interface over the vault —
//! `gg.credentials()` returns a [`CredentialService`]; [`DefaultCredentialService`] wraps a
//! [`LocalVault`] behind a lock and refreshes cross-process changes on read.
//!
//! ## Overview
//! [`Secret`] carries the decrypted value in a [`zeroize`]-ing buffer and **never logs it**
//! ([`Secret`]'s `Debug` redacts the bytes). [`SecretMeta`] is metadata only. Typed convenience
//! views (AWS creds, basic-auth, TLS bundle, Kafka SASL) are deferred to a later phase and will be
//! thin accessors over [`Secret`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use super::sync::SyncEngine;
use super::vault::{LocalVault, PutOptions};
use crate::error::GgError;
use crate::Result;

/// A decrypted secret value plus its metadata. The bytes are zeroized on drop and redacted from
/// `Debug`; do not log or serialize them.
#[derive(Clone)]
pub struct Secret {
    pub name: String,
    pub version: String,
    pub(crate) bytes: Zeroizing<Vec<u8>>,
    pub labels: BTreeMap<String, String>,
    pub created_ms: u64,
    pub source: String,
    pub content_type: String,
}

impl Secret {
    /// The raw secret bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The value as UTF-8 (errors if not valid UTF-8).
    pub fn as_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.bytes).map_err(|_| GgError::Credentials("secret is not valid UTF-8".into()))
    }

    /// The value parsed as JSON.
    pub fn as_json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.bytes).map_err(|e| GgError::Credentials(format!("secret is not JSON: {e}")))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("bytes", &format_args!("<{} bytes redacted>", self.bytes.len()))
            .field("source", &self.source)
            .finish()
    }
}

/// Metadata for a secret version — safe to log/list (no value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    pub name: String,
    pub version: String,
    pub created_ms: u64,
    pub ttl_secs: Option<u64>,
    pub source: String,
    pub labels: BTreeMap<String, String>,
}

/// The public credential interface (depend on this, not [`DefaultCredentialService`]).
pub trait CredentialService: Send + Sync {
    /// Latest version of `name`, or `None`.
    fn get(&self, name: &str) -> Result<Option<Secret>>;
    /// A specific version of `name`.
    fn get_version(&self, name: &str, version: &str) -> Result<Option<Secret>>;
    /// Whether a secret exists.
    fn exists(&self, name: &str) -> Result<bool>;
    /// Metadata for all secrets under `prefix` (empty = all). Never returns values.
    fn list(&self, prefix: &str) -> Result<Vec<SecretMeta>>;
    /// Retained version ids for `name` (oldest→newest).
    fn versions(&self, name: &str) -> Result<Vec<String>>;
    /// Write a local-only secret version; returns the new version id.
    fn put(&self, name: &str, value: &[u8], opts: PutOptions) -> Result<String>;
    /// Remove a secret entirely.
    fn delete(&self, name: &str) -> Result<bool>;
    /// Force an immediate pull from the central source (no-op when no central sync is configured).
    fn refresh(&self) -> Result<()> {
        Ok(())
    }

    /// The value as bytes (convenience).
    fn get_bytes(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self.get(name)?.map(|s| s.bytes.clone()))
    }
    /// The value as a UTF-8 string (convenience).
    fn get_string(&self, name: &str) -> Result<Option<String>> {
        match self.get(name)? {
            Some(s) => Ok(Some(s.as_str()?.to_string())),
            None => Ok(None),
        }
    }
    /// The value parsed as JSON (convenience).
    fn get_json(&self, name: &str) -> Result<Option<serde_json::Value>> {
        match self.get(name)? {
            Some(s) => Ok(Some(s.as_json()?)),
            None => Ok(None),
        }
    }
}

/// The default [`CredentialService`]: a [`LocalVault`] behind a mutex. Each read first picks up any
/// cross-process change (the shared device vault may be written by another component).
pub struct DefaultCredentialService {
    vault: Arc<Mutex<LocalVault>>,
    /// Owns the central sync background thread (RAII); `None` for a standalone local vault.
    _sync: Option<SyncEngine>,
    /// Transparent key namespace (`<thingName>/<componentName>`), or empty for no namespacing.
    /// Prepended to every key so a shared device vault (and a fleet's central store) can't collide
    /// across components/devices; stripped from returned names so callers see their own keys.
    namespace: String,
}

impl DefaultCredentialService {
    /// Wrap an opened [`LocalVault`] (standalone, no central sync, no namespacing).
    pub fn new(vault: LocalVault) -> Self {
        Self { vault: Arc::new(Mutex::new(vault)), _sync: None, namespace: String::new() }
    }

    /// Wrap a shared vault that a [`SyncEngine`] also writes to, with the given key namespace.
    pub fn with_sync(vault: Arc<Mutex<LocalVault>>, sync: Option<SyncEngine>, namespace: String) -> Self {
        Self { vault, _sync: sync, namespace }
    }

    /// The shared vault handle (so a [`SyncEngine`] can be constructed against the same store).
    pub fn vault_arc(&self) -> Arc<Mutex<LocalVault>> {
        self.vault.clone()
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, LocalVault> {
        self.vault.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Map a caller-facing key to its namespaced storage key.
    fn full(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.namespace, name)
        }
    }

    /// Strip the namespace from a storage key for the caller.
    fn rel(&self, full: &str) -> String {
        if self.namespace.is_empty() {
            full.to_string()
        } else {
            full.strip_prefix(&format!("{}/", self.namespace)).unwrap_or(full).to_string()
        }
    }
}

impl CredentialService for DefaultCredentialService {
    fn get(&self, name: &str) -> Result<Option<Secret>> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        Ok(v.get(&self.full(name))?.map(|mut s| {
            s.name = self.rel(&s.name);
            s
        }))
    }
    fn get_version(&self, name: &str, version: &str) -> Result<Option<Secret>> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        Ok(v.get_version(&self.full(name), version)?.map(|mut s| {
            s.name = self.rel(&s.name);
            s
        }))
    }
    fn exists(&self, name: &str) -> Result<bool> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        Ok(v.exists(&self.full(name)))
    }
    fn list(&self, prefix: &str) -> Result<Vec<SecretMeta>> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        // List within this component's namespace and strip it from the returned names.
        Ok(v.list(&self.full(prefix))
            .into_iter()
            .map(|mut m| {
                m.name = self.rel(&m.name);
                m
            })
            .collect())
    }
    fn versions(&self, name: &str) -> Result<Vec<String>> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        Ok(v.versions(&self.full(name)))
    }
    fn put(&self, name: &str, value: &[u8], opts: PutOptions) -> Result<String> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        v.put(&self.full(name), value, opts)
    }
    fn delete(&self, name: &str) -> Result<bool> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        v.delete(&self.full(name))
    }
    fn refresh(&self) -> Result<()> {
        if let Some(sync) = &self._sync {
            sync.sync_now();
        }
        Ok(())
    }
}
