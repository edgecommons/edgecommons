//! # Local vault
//!
//! **One-liner purpose**: The encrypted-at-rest secret store — opens/creates the vault file,
//! seals/opens records under the DEK, persists atomically under a cross-process lock, and serves
//! versioned reads.
//!
//! ## Overview
//! A vault is a single JSON file ([`super::format::VaultFile`]). On open, the DEK is unwrapped via
//! the [`KeyProvider`] and the file MAC is verified; reads decrypt in memory. Writes append a new
//! monotonic version, prune to `keep_versions`, recompute the MAC, and persist with a temp→rename
//! atomic write while holding an advisory lock on a sidecar `.lock` file — so the **shared device
//! vault** stays consistent across concurrent component processes.
//!
//! ## Semantics & Architecture
//! - Single-writer-at-a-time via the file lock; readers are lock-free and see whole files only
//!   (atomic rename). [`LocalVault::reload_if_changed`] gives cross-process read freshness.
//! - All secret plaintext and key material live in [`zeroize`]-ing buffers.
//! - Errors map to [`GgError::Credentials`]; a MAC/AEAD failure is fail-closed (never returns
//!   partial/plaintext data).

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use fs4::fs_std::FileExt;
use zeroize::Zeroizing;

use super::crypto;
use super::format::{self, KekInfo, SecretEntry, VaultFile, VersionEntry, FORMAT_VERSION};
use super::keyprovider::KeyProvider;
use super::service::{Secret, SecretMeta};
use crate::error::GgError;
use crate::Result;

/// Options controlling a `put`.
#[derive(Debug, Clone, Default)]
pub struct PutOptions {
    pub ttl_secs: Option<u64>,
    pub labels: BTreeMap<String, String>,
    pub content_type: Option<String>,
    /// `central` when written by the sync engine; defaults to `local`.
    pub source: Option<String>,
    pub central_version_id: Option<String>,
}

/// The encrypted local secret store.
pub struct LocalVault {
    path: PathBuf,
    vault_id: String,
    dek: Zeroizing<[u8; crypto::KEY_LEN]>,
    /// Retained past open for phase-2 KEK rotation / re-wrap (DEK lives in memory, so reads don't
    /// need it).
    #[allow(dead_code)]
    key_provider: Arc<dyn KeyProvider>,
    kek: KekInfo,
    secrets: BTreeMap<String, SecretEntry>,
    keep_versions: usize,
    /// (mtime, len) of the file as last loaded — drives [`reload_if_changed`].
    stamp: Option<(SystemTime, u64)>,
}

impl LocalVault {
    /// Open an existing vault or create a new empty one at `path`.
    ///
    /// # Post-conditions
    /// On success the DEK is unwrapped and (for an existing file) the MAC verified. A new vault is
    /// created with a random `vaultId` + DEK, the DEK wrapped via `key_provider`, and persisted.
    pub fn open(path: impl AsRef<Path>, key_provider: Arc<dyn KeyProvider>, keep_versions: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let keep_versions = keep_versions.max(1);
        if path.exists() {
            let vf = read_file(&path)?;
            Self::from_file(path, vf, key_provider, keep_versions)
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| GgError::Credentials(format!("create vault dir: {e}")))?;
            }
            let vault_id = uuid::Uuid::new_v4().to_string();
            let dek = Zeroizing::new(crypto::random::<{ crypto::KEY_LEN }>());
            let kek = key_provider.wrap_dek(&vault_id, &dek)?;
            let mut v = Self {
                path,
                vault_id,
                dek,
                key_provider,
                kek,
                secrets: BTreeMap::new(),
                keep_versions,
                stamp: None,
            };
            v.save()?;
            Ok(v)
        }
    }

    fn from_file(path: PathBuf, vf: VaultFile, key_provider: Arc<dyn KeyProvider>, keep_versions: usize) -> Result<Self> {
        if vf.format != FORMAT_VERSION {
            return Err(GgError::Credentials(format!("unsupported vault format {}", vf.format)));
        }
        let dek = key_provider.unwrap_dek(&vf.vault_id, &vf.kek)?;
        verify_mac(&dek, &vf)?;
        let stamp = file_stamp(&path).ok();
        Ok(Self {
            path,
            vault_id: vf.vault_id,
            dek,
            key_provider,
            kek: vf.kek,
            secrets: vf.secrets,
            keep_versions,
            stamp,
        })
    }

    /// The vault's stable id.
    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    /// Latest version of `name`, decrypted, or `None` if absent.
    pub fn get(&self, name: &str) -> Result<Option<Secret>> {
        match self.secrets.get(name).and_then(|e| e.versions.last()) {
            Some(v) => Ok(Some(self.decrypt(name, v)?)),
            None => Ok(None),
        }
    }

    /// A specific version of `name`, decrypted.
    pub fn get_version(&self, name: &str, version: &str) -> Result<Option<Secret>> {
        match self.secrets.get(name).and_then(|e| e.versions.iter().find(|v| v.version == version)) {
            Some(v) => Ok(Some(self.decrypt(name, v)?)),
            None => Ok(None),
        }
    }

    /// Whether a secret exists (no decryption).
    pub fn exists(&self, name: &str) -> bool {
        self.secrets.get(name).is_some_and(|e| !e.versions.is_empty())
    }

    /// Metadata for all secrets whose name starts with `prefix` (empty = all). Never decrypts.
    pub fn list(&self, prefix: &str) -> Vec<SecretMeta> {
        self.secrets
            .iter()
            .filter(|(n, _)| n.starts_with(prefix))
            .filter_map(|(n, e)| e.versions.last().map(|v| meta_of(n, v)))
            .collect()
    }

    /// Version ids retained for `name` (oldest→newest).
    pub fn versions(&self, name: &str) -> Vec<String> {
        self.secrets
            .get(name)
            .map(|e| e.versions.iter().map(|v| v.version.clone()).collect())
            .unwrap_or_default()
    }

    /// Write a new version of `name` and persist. Prunes to `keep_versions`.
    pub fn put(&mut self, name: &str, plaintext: &[u8], opts: PutOptions) -> Result<String> {
        let next = self.next_version(name);
        let nonce: [u8; crypto::NONCE_LEN] = crypto::random();
        let aad = format::record_aad(&self.vault_id, name, &next);
        let ct = crypto::seal(&self.dek, &nonce, &aad, plaintext)?;
        let entry = VersionEntry {
            version: next.clone(),
            created_ms: now_ms(),
            ttl_secs: opts.ttl_secs,
            source: opts.source.unwrap_or_else(|| "local".to_string()),
            central_version_id: opts.central_version_id,
            labels: opts.labels,
            content_type: opts.content_type.unwrap_or_else(|| "application/octet-stream".to_string()),
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ct),
        };
        let versions = &mut self.secrets.entry(name.to_string()).or_insert_with(|| SecretEntry { versions: vec![] }).versions;
        versions.push(entry);
        let keep = self.keep_versions;
        if versions.len() > keep {
            let drop = versions.len() - keep;
            versions.drain(0..drop);
        }
        self.save()?;
        Ok(next)
    }

    /// Remove a secret entirely and persist.
    pub fn delete(&mut self, name: &str) -> Result<bool> {
        let removed = self.secrets.remove(name).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Re-read the vault from disk if the file changed since last load (cross-process freshness).
    /// Returns `true` if a reload happened.
    pub fn reload_if_changed(&mut self) -> Result<bool> {
        let cur = file_stamp(&self.path).ok();
        if cur == self.stamp {
            return Ok(false);
        }
        let vf = read_file(&self.path)?;
        // DEK is unchanged for a given vault; re-verify integrity with the held DEK.
        verify_mac(&self.dek, &vf)?;
        self.secrets = vf.secrets;
        self.kek = vf.kek;
        self.stamp = cur;
        Ok(true)
    }

    fn next_version(&self, name: &str) -> String {
        let n = self
            .secrets
            .get(name)
            .and_then(|e| e.versions.last())
            .and_then(|v| v.version.parse::<u64>().ok())
            .unwrap_or(0);
        format!("{:08}", n + 1)
    }

    fn decrypt(&self, name: &str, v: &VersionEntry) -> Result<Secret> {
        let nonce: [u8; crypto::NONCE_LEN] = B64
            .decode(&v.nonce)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| GgError::Credentials("bad record nonce".into()))?;
        let ct = B64.decode(&v.ciphertext).map_err(|_| GgError::Credentials("bad ciphertext".into()))?;
        let aad = format::record_aad(&self.vault_id, name, &v.version);
        let bytes = crypto::open(&self.dek, &nonce, &aad, &ct)?;
        Ok(Secret {
            name: name.to_string(),
            version: v.version.clone(),
            bytes: Zeroizing::new(bytes.to_vec()),
            labels: v.labels.clone(),
            created_ms: v.created_ms,
            source: v.source.clone(),
            content_type: v.content_type.clone(),
        })
    }

    /// Recompute the MAC and persist atomically under the cross-process lock.
    fn save(&mut self) -> Result<()> {
        let mac_key = crypto::derive_mac_key(&self.dek, &self.vault_id);
        let input = format::mac_input(&self.vault_id, &self.secrets, decode_b64);
        let mac = B64.encode(crypto::hmac(&mac_key, &input));
        let vf = VaultFile {
            format: FORMAT_VERSION,
            vault_id: self.vault_id.clone(),
            kek: self.kek.clone(),
            secrets: self.secrets.clone(),
            mac,
        };
        let json = serde_json::to_vec_pretty(&vf).map_err(|e| GgError::Credentials(format!("serialize vault: {e}")))?;

        // Coordinate concurrent writers (shared device vault) via an advisory lock on a sidecar.
        let lock_path = self.path.with_extension("lock");
        let lock = File::create(&lock_path).map_err(|e| GgError::Credentials(format!("open vault lock: {e}")))?;
        lock.lock_exclusive().map_err(|e| GgError::Credentials(format!("lock vault: {e}")))?;

        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| GgError::Credentials(format!("write vault tmp: {e}")))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| GgError::Credentials(format!("rename vault: {e}")))?;
        let _ = FileExt::unlock(&lock);

        self.stamp = file_stamp(&self.path).ok();
        Ok(())
    }
}

fn read_file(path: &Path) -> Result<VaultFile> {
    let bytes = std::fs::read(path).map_err(|e| GgError::Credentials(format!("read vault: {e}")))?;
    serde_json::from_slice(&bytes).map_err(|e| GgError::Credentials(format!("parse vault: {e}")))
}

fn verify_mac(dek: &[u8; crypto::KEY_LEN], vf: &VaultFile) -> Result<()> {
    let mac_key = crypto::derive_mac_key(dek, &vf.vault_id);
    let input = format::mac_input(&vf.vault_id, &vf.secrets, decode_b64);
    let expected = B64.decode(&vf.mac).map_err(|_| GgError::Credentials("bad MAC encoding".into()))?;
    if crypto::hmac_verify(&mac_key, &input, &expected) {
        Ok(())
    } else {
        Err(GgError::Credentials("vault integrity check failed (tampered or wrong key)".into()))
    }
}

fn decode_b64(s: &str) -> Vec<u8> {
    B64.decode(s).unwrap_or_default()
}

fn meta_of(name: &str, v: &VersionEntry) -> SecretMeta {
    SecretMeta {
        name: name.to_string(),
        version: v.version.clone(),
        created_ms: v.created_ms,
        ttl_secs: v.ttl_secs,
        source: v.source.clone(),
        labels: v.labels.clone(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn file_stamp(path: &Path) -> std::io::Result<(SystemTime, u64)> {
    let m = std::fs::metadata(path)?;
    Ok((m.modified()?, m.len()))
}
