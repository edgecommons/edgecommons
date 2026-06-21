//! # Vault on-disk format (normative — identical across all language ports)
//!
//! **One-liner purpose**: The serde model of the vault file plus the exact byte constructions
//! (AEAD AAD and the MAC input) that every binding must reproduce.
//!
//! ## Overview
//! The vault is a single JSON file (see [`VaultFile`]). Numbers that are encrypted are stored as
//! base64; the secret name map is a [`BTreeMap`] so JSON key order is deterministic. Integrity is
//! a single HMAC ([`VaultFile::mac`]) over a **length-prefixed canonical byte string** (not the
//! JSON text) so it is insensitive to JSON formatting differences between languages.
//!
//! ## Semantics & Architecture
//! - `format = 1`. `vaultId` is a stable per-vault UUID, bound into every AAD and the MAC so
//!   records cannot be copied between vaults.
//! - **Record AAD** (`record_aad`): `ggcommons-vault/v1|<vaultId>|<name>|<version>` — binds each
//!   ciphertext to its identity *and* version (anti-swap, anti in-file rollback of a record).
//! - **DEK-wrap AAD** (`dek_wrap_aad`): `ggcommons-vault/v1/dek-wrap|<vaultId>`.
//! - **MAC input** (`mac_input`): length-prefixed concatenation of the full secret set (raw
//!   nonce/ciphertext bytes, not base64); see the function for the exact layout. The MAC key is
//!   `HKDF-SHA256(DEK, salt=vaultId, info="…/mac")`.
//!
//! ## Design choices
//! Length-prefixed binary MAC input (rather than canonical JSON) is how interoperable encrypted
//! formats (age, JWE) stay byte-identical across independent implementations without sharing code.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current on-disk format version.
pub const FORMAT_VERSION: u32 = 1;

/// The whole vault file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultFile {
    pub format: u32,
    pub vault_id: String,
    pub kek: KekInfo,
    /// Secret name → entry. `BTreeMap` keeps JSON keys sorted (deterministic output).
    pub secrets: BTreeMap<String, SecretEntry>,
    /// base64(HMAC-SHA256) over [`mac_input`].
    pub mac: String,
}

/// How the DEK is wrapped — written by the [`super::keyprovider::KeyProvider`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KekInfo {
    /// Custodian id: `file` | `kms` | `greengrass` | `pkcs11` | `env`.
    pub provider: String,
    /// Wrap algorithm, e.g. `AES-256-GCM` (file/env) or `aws-kms` (kms).
    pub alg: String,
    /// base64 nonce used to wrap the DEK (AEAD wrap providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap_nonce: Option<String>,
    /// base64 wrapped DEK (AEAD ciphertext, or the KMS ciphertext blob).
    pub wrapped_dek: String,
    /// KMS key id/arn (provider = kms/greengrass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_key_id: Option<String>,
}

/// All retained versions of one secret (newest last).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretEntry {
    pub versions: Vec<VersionEntry>,
}

/// One encrypted version of a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    /// Monotonic, zero-padded 8-digit version (e.g. `00000003`).
    pub version: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
    /// `local` (written via `put`) or `central` (pulled from the upstream vault).
    pub source: String,
    /// Upstream version id for change detection (central source).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub central_version_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    /// base64 AEAD nonce for this version.
    pub nonce: String,
    /// base64 AEAD `ciphertext || tag`.
    pub ciphertext: String,
}

fn default_content_type() -> String {
    "application/octet-stream".to_string()
}

/// AAD binding a record's ciphertext to its vault, name, and version.
pub fn record_aad(vault_id: &str, name: &str, version: &str) -> Vec<u8> {
    format!("ggcommons-vault/v1|{vault_id}|{name}|{version}").into_bytes()
}

/// AAD binding the wrapped DEK to its vault.
pub fn dek_wrap_aad(vault_id: &str) -> Vec<u8> {
    format!("ggcommons-vault/v1/dek-wrap|{vault_id}").into_bytes()
}

/// Build the canonical MAC input over the whole secret set.
///
/// # Semantics
/// Layout (all integers little-endian; `lp(x)` = `u32_le(len) ‖ x`):
/// ```text
/// b"ggcommons-vault/v1/mac"
///   ‖ lp(vaultId)
///   ‖ u32_le(secret_count)
///   ‖ for each secret (BTreeMap order = name byte order):
///       lp(name) ‖ u32_le(version_count)
///         ‖ for each version (array order):
///             lp(version) ‖ u64_le(createdMs) ‖ u64_le(ttlSecs or 0)
///             ‖ lp(source) ‖ lp(centralVersionId or "")
///             ‖ lp(nonce_raw) ‖ lp(ciphertext_raw)
/// ```
/// `nonce_raw`/`ciphertext_raw` are the **decoded** bytes (the MAC is over raw bytes, not the
/// base64 text). Authenticates every field that matters; insensitive to JSON formatting.
pub fn mac_input(
    vault_id: &str,
    secrets: &BTreeMap<String, SecretEntry>,
    decode_b64: impl Fn(&str) -> Vec<u8>,
) -> Vec<u8> {
    fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(b"ggcommons-vault/v1/mac");
    lp(&mut out, vault_id.as_bytes());
    out.extend_from_slice(&(secrets.len() as u32).to_le_bytes());
    for (name, entry) in secrets {
        lp(&mut out, name.as_bytes());
        out.extend_from_slice(&(entry.versions.len() as u32).to_le_bytes());
        for v in &entry.versions {
            lp(&mut out, v.version.as_bytes());
            out.extend_from_slice(&v.created_ms.to_le_bytes());
            out.extend_from_slice(&v.ttl_secs.unwrap_or(0).to_le_bytes());
            lp(&mut out, v.source.as_bytes());
            lp(&mut out, v.central_version_id.as_deref().unwrap_or("").as_bytes());
            lp(&mut out, &decode_b64(&v.nonce));
            lp(&mut out, &decode_b64(&v.ciphertext));
        }
    }
    out
}
