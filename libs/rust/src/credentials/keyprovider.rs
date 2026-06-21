//! # Key providers (KEK custodians)
//!
//! **One-liner purpose**: The unlock seam — wrap/unwrap the vault's Data Encryption Key (DEK)
//! through a Key Encryption Key (KEK) whose custodian is software (file/env) or hardware/remote
//! (HSM/TPM via PKCS#11, AWS KMS — added in later phases).
//!
//! ## Overview
//! The [`KeyProvider`] trait is intentionally narrow: given the DEK it returns a [`KekInfo`]
//! record for the file; given a [`KekInfo`] it returns the unwrapped DEK. The KEK itself never
//! leaves the custodian (for hardware/KMS) and never lands on disk in plaintext (for file/env).
//!
//! ## Semantics & Architecture
//! - Phase 1 ships [`FileKeyProvider`]: the KEK is 32 bytes in a `0600` key file; the DEK is
//!   wrapped with AES-256-GCM under the KEK. `kms`/`greengrass`/`pkcs11` providers slot in behind
//!   this same trait without a format change.
//! - All key material is held in [`zeroize`]-ing buffers.

use std::path::Path;

use zeroize::Zeroizing;

use super::crypto::{self, KEY_LEN, NONCE_LEN};
use super::format::{dek_wrap_aad, KekInfo};
use crate::error::GgError;
use crate::Result;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

/// A custodian that wraps/unwraps the vault DEK. Implementations must be `Send + Sync` so the
/// vault can live behind a lock shared across threads.
pub trait KeyProvider: Send + Sync {
    /// Custodian id written to [`KekInfo::provider`] (e.g. `"file"`).
    fn provider_id(&self) -> &str;

    /// Wrap `dek` for `vault_id`, producing the [`KekInfo`] persisted in the vault file.
    fn wrap_dek(&self, vault_id: &str, dek: &[u8; KEY_LEN]) -> Result<KekInfo>;

    /// Unwrap the DEK described by `kek` for `vault_id`.
    fn unwrap_dek(&self, vault_id: &str, kek: &KekInfo) -> Result<Zeroizing<[u8; KEY_LEN]>>;
}

/// KEK held as 32 bytes in a local key file (the standalone / offline-fallback custodian).
///
/// The DEK is wrapped with AES-256-GCM under the KEK, AAD-bound to the vault id. Protect the key
/// file with `0600` perms; rotate by re-wrapping the DEK under a new KEK.
pub struct FileKeyProvider {
    kek: Zeroizing<[u8; KEY_LEN]>,
}

impl FileKeyProvider {
    /// Construct from raw 32-byte key material (e.g. read from an env var or test vector).
    pub fn from_bytes(kek: [u8; KEY_LEN]) -> Self {
        Self { kek: Zeroizing::new(kek) }
    }

    /// Load the KEK from a key file (exactly 32 raw bytes).
    ///
    /// # Errors
    /// `GgError::Credentials` if the file is missing or not 32 bytes.
    pub fn from_keyfile(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())
            .map_err(|e| GgError::Credentials(format!("read key file: {e}")))?;
        let kek: [u8; KEY_LEN] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| GgError::Credentials(format!("key file must be {KEY_LEN} bytes")))?;
        Ok(Self::from_bytes(kek))
    }

    /// Generate a fresh random KEK and write it to `path` (caller should set `0600`).
    /// Returns the provider for immediate use.
    pub fn generate_keyfile(path: impl AsRef<Path>) -> Result<Self> {
        let kek: [u8; KEY_LEN] = crypto::random();
        std::fs::write(path.as_ref(), kek)
            .map_err(|e| GgError::Credentials(format!("write key file: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path.as_ref(), std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self::from_bytes(kek))
    }
}

impl KeyProvider for FileKeyProvider {
    fn provider_id(&self) -> &str {
        "file"
    }

    fn wrap_dek(&self, vault_id: &str, dek: &[u8; KEY_LEN]) -> Result<KekInfo> {
        let nonce: [u8; NONCE_LEN] = crypto::random();
        let wrapped = crypto::seal(&self.kek, &nonce, &dek_wrap_aad(vault_id), dek)?;
        Ok(KekInfo {
            provider: "file".to_string(),
            alg: "AES-256-GCM".to_string(),
            wrap_nonce: Some(B64.encode(nonce)),
            wrapped_dek: B64.encode(wrapped),
            kms_key_id: None,
        })
    }

    fn unwrap_dek(&self, vault_id: &str, kek: &KekInfo) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        let nonce_b = kek
            .wrap_nonce
            .as_ref()
            .ok_or_else(|| GgError::Credentials("file KEK: missing wrapNonce".into()))?;
        let nonce: [u8; NONCE_LEN] = B64
            .decode(nonce_b)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| GgError::Credentials("file KEK: bad wrapNonce".into()))?;
        let wrapped = B64
            .decode(&kek.wrapped_dek)
            .map_err(|_| GgError::Credentials("file KEK: bad wrappedDek".into()))?;
        let dek = crypto::open(&self.kek, &nonce, &dek_wrap_aad(vault_id), &wrapped)?;
        let arr: [u8; KEY_LEN] = dek
            .as_slice()
            .try_into()
            .map_err(|_| GgError::Credentials("unwrapped DEK wrong length".into()))?;
        Ok(Zeroizing::new(arr))
    }
}
