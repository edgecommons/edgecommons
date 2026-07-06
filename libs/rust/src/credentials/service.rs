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
use crate::Result;
use crate::error::EdgeCommonsError;

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
        std::str::from_utf8(&self.bytes)
            .map_err(|_| EdgeCommonsError::Credentials("secret is not valid UTF-8".into()))
    }

    /// The value parsed as JSON.
    pub fn as_json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.bytes)
            .map_err(|e| EdgeCommonsError::Credentials(format!("secret is not JSON: {e}")))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("name", &self.name)
            .field("version", &self.version)
            .field(
                "bytes",
                &format_args!("<{} bytes redacted>", self.bytes.len()),
            )
            .field("source", &self.source)
            .finish()
    }
}

/// Non-sensitive credential-subsystem stats (for the metrics bridge). Never includes values.
#[derive(Debug, Clone, Default)]
pub struct CredentialStats {
    pub secret_count: u64,
    /// Age of the last successful central sync, ms (None if no central sync / never synced).
    pub last_sync_age_ms: Option<u64>,
    pub sync_failures: u64,
    pub rotations: u64,
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

    /// Non-sensitive stats for observability (default: just the secret count).
    fn stats(&self) -> CredentialStats {
        CredentialStats {
            secret_count: self.list("").map(|v| v.len() as u64).unwrap_or(0),
            ..CredentialStats::default()
        }
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

    // ----- typed views (thin parses over the opaque secret; see `views`) -----

    /// AWS credentials stored at `name`.
    fn get_aws_credentials(&self, name: &str) -> Result<Option<super::views::AwsCredentials>> {
        match self.get(name)? {
            Some(s) => Ok(Some(super::views::parse(&s, "AWS credentials")?)),
            None => Ok(None),
        }
    }
    /// HTTP basic-auth credentials stored at `name`.
    fn get_basic_auth(&self, name: &str) -> Result<Option<super::views::BasicAuth>> {
        match self.get(name)? {
            Some(s) => Ok(Some(super::views::parse(&s, "basic auth")?)),
            None => Ok(None),
        }
    }
    /// A TLS bundle stored at `name`.
    fn get_tls_bundle(&self, name: &str) -> Result<Option<super::views::TlsBundle>> {
        match self.get(name)? {
            Some(s) => Ok(Some(super::views::parse(&s, "a TLS bundle")?)),
            None => Ok(None),
        }
    }
    /// Kafka SASL credentials stored at `name`.
    fn get_kafka_sasl(&self, name: &str) -> Result<Option<super::views::KafkaSasl>> {
        match self.get(name)? {
            Some(s) => Ok(Some(super::views::parse(&s, "Kafka SASL")?)),
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
    /// Audit sink for access events (`None` = auditing off). Set via [`with_audit`](Self::with_audit);
    /// the config path enables it (`credentials.audit.enabled`) with the default logging sink.
    audit: Option<Arc<dyn super::audit::AuditSink>>,
}

impl DefaultCredentialService {
    /// Wrap an opened [`LocalVault`] (standalone, no central sync, no namespacing, no audit).
    pub fn new(vault: LocalVault) -> Self {
        Self {
            vault: Arc::new(Mutex::new(vault)),
            _sync: None,
            namespace: String::new(),
            audit: None,
        }
    }

    /// Wrap a shared vault that a [`SyncEngine`] also writes to, with the given key namespace.
    pub fn with_sync(
        vault: Arc<Mutex<LocalVault>>,
        sync: Option<SyncEngine>,
        namespace: String,
    ) -> Self {
        Self {
            vault,
            _sync: sync,
            namespace,
            audit: None,
        }
    }

    /// Attach (or clear) the audit sink — access events are emitted to it. Fluent; returns `self`.
    pub fn with_audit(mut self, sink: Option<Arc<dyn super::audit::AuditSink>>) -> Self {
        self.audit = sink;
        self
    }

    /// Emit an audit event if an audit sink is configured (no-op otherwise).
    fn audit_event(
        &self,
        op: &'static str,
        name: &str,
        version: &str,
        source: &str,
        outcome: &'static str,
    ) {
        if let Some(sink) = &self.audit {
            sink.record(&super::audit::AuditEvent {
                op,
                name: name.to_string(),
                version: version.to_string(),
                source: source.to_string(),
                outcome,
            });
        }
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
            full.strip_prefix(&format!("{}/", self.namespace))
                .unwrap_or(full)
                .to_string()
        }
    }
}

impl CredentialService for DefaultCredentialService {
    fn get(&self, name: &str) -> Result<Option<Secret>> {
        // Scope the vault lock so it's released before the audit sink is called.
        let result = {
            let mut v = self.locked();
            v.reload_if_changed()?;
            v.get(&self.full(name))?.map(|mut s| {
                s.name = self.rel(&s.name);
                s
            })
        };
        match &result {
            Some(s) => self.audit_event("get", name, &s.version, &s.source, "hit"),
            None => self.audit_event("get", name, "-", "-", "miss"),
        }
        Ok(result)
    }
    fn get_version(&self, name: &str, version: &str) -> Result<Option<Secret>> {
        let result = {
            let mut v = self.locked();
            v.reload_if_changed()?;
            v.get_version(&self.full(name), version)?.map(|mut s| {
                s.name = self.rel(&s.name);
                s
            })
        };
        match &result {
            Some(s) => self.audit_event("get", name, &s.version, &s.source, "hit"),
            None => self.audit_event("get", name, version, "-", "miss"),
        }
        Ok(result)
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
        let version = {
            let mut v = self.locked();
            v.reload_if_changed()?;
            v.put(&self.full(name), value, opts)?
        };
        self.audit_event("put", name, &version, "local", "ok");
        Ok(version)
    }
    fn delete(&self, name: &str) -> Result<bool> {
        let deleted = {
            let mut v = self.locked();
            v.reload_if_changed()?;
            v.delete(&self.full(name))?
        };
        self.audit_event(
            "delete",
            name,
            "-",
            "-",
            if deleted { "ok" } else { "miss" },
        );
        Ok(deleted)
    }
    fn refresh(&self) -> Result<()> {
        if let Some(sync) = &self._sync {
            sync.sync_now();
        }
        Ok(())
    }

    fn stats(&self) -> CredentialStats {
        let secret_count = self.list("").map(|v| v.len() as u64).unwrap_or(0);
        let (last_sync_age_ms, sync_failures, rotations) = match &self._sync {
            Some(s) => {
                let (last_ok, failures, rotations) = s.stats();
                (
                    last_ok.map(|ms| now_ms_service().saturating_sub(ms)),
                    failures,
                    rotations,
                )
            }
            None => (None, 0, 0),
        };
        CredentialStats {
            secret_count,
            last_sync_age_ms,
            sync_failures,
            rotations,
        }
    }
}

fn now_ms_service() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
