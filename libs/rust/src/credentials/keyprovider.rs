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

#[cfg(feature = "credentials-aws")]
pub use kms::KmsKeyProvider;

#[cfg(feature = "credentials-aws")]
mod kms {
    //! KMS-wrapped DEK custodian: the DEK is encrypted by an AWS KMS CMK (the KEK never leaves
    //! KMS) and unwrapped via `kms:Decrypt` — using AWS creds / TES on Greengrass. The encryption
    //! context binds the wrapped DEK to the vault id (anti-swap). The client is loaded on a
    //! dedicated thread so construction is safe inside the library's async `build()`.
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    use aws_sdk_kms::error::DisplayErrorContext;
    use aws_sdk_kms::primitives::Blob;
    use aws_sdk_kms::Client;
    use tokio::runtime::Runtime;
    use zeroize::Zeroizing;

    use super::super::crypto::KEY_LEN;
    use super::super::format::KekInfo;
    use super::KeyProvider;
    use crate::error::GgError;
    use crate::Result;

    pub struct KmsKeyProvider {
        rt: Runtime,
        client: Client,
        key_id: String,
    }

    impl KmsKeyProvider {
        pub fn new(key_id: String, region: Option<String>, endpoint_url: Option<String>) -> Result<Self> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("ggcommons-kms")
                .build()
                .map_err(|e| GgError::Credentials(format!("tokio runtime: {e}")))?;
            let client = std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        rt.block_on(async {
                            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
                            if let Some(r) = region {
                                loader = loader.region(aws_sdk_kms::config::Region::new(r));
                            }
                            if let Some(url) = endpoint_url {
                                loader = loader.endpoint_url(url);
                            }
                            Client::new(&loader.load().await)
                        })
                    })
                    .join()
                    .map_err(|_| GgError::Credentials("kms client init thread panicked".into()))
            })?;
            Ok(Self { rt, client, key_id })
        }
    }

    impl KeyProvider for KmsKeyProvider {
        fn provider_id(&self) -> &str {
            "kms"
        }

        fn wrap_dek(&self, vault_id: &str, dek: &[u8; KEY_LEN]) -> Result<KekInfo> {
            let resp = self
                .rt
                .block_on(
                    self.client
                        .encrypt()
                        .key_id(&self.key_id)
                        .plaintext(Blob::new(dek.to_vec()))
                        .encryption_context("vaultId", vault_id)
                        .send(),
                )
                .map_err(|e| GgError::Credentials(format!("kms encrypt: {}", DisplayErrorContext(&e))))?;
            let ct = resp
                .ciphertext_blob()
                .ok_or_else(|| GgError::Credentials("kms encrypt: no ciphertext".into()))?
                .as_ref()
                .to_vec();
            Ok(KekInfo {
                provider: "kms".to_string(),
                alg: "aws-kms".to_string(),
                wrap_nonce: None,
                wrapped_dek: B64.encode(ct),
                kms_key_id: Some(self.key_id.clone()),
            })
        }

        fn unwrap_dek(&self, vault_id: &str, kek: &KekInfo) -> Result<Zeroizing<[u8; KEY_LEN]>> {
            let ct = B64
                .decode(&kek.wrapped_dek)
                .map_err(|_| GgError::Credentials("kms: bad wrappedDek".into()))?;
            let resp = self
                .rt
                .block_on(
                    self.client
                        .decrypt()
                        .ciphertext_blob(Blob::new(ct))
                        .key_id(&self.key_id)
                        .encryption_context("vaultId", vault_id)
                        .send(),
                )
                .map_err(|e| GgError::Credentials(format!("kms decrypt: {}", DisplayErrorContext(&e))))?;
            let pt = resp
                .plaintext()
                .ok_or_else(|| GgError::Credentials("kms decrypt: no plaintext".into()))?;
            let arr: [u8; KEY_LEN] = pt
                .as_ref()
                .try_into()
                .map_err(|_| GgError::Credentials("kms: unwrapped DEK wrong length".into()))?;
            Ok(Zeroizing::new(arr))
        }
    }
}
