//! # Vault cryptographic primitives
//!
//! **One-liner purpose**: The small, universal crypto building blocks the vault format is built
//! on, chosen so every language port (Java JCE, Python `cryptography`, Node `crypto`) can
//! reproduce the exact bytes.
//!
//! ## Overview
//! - **AEAD**: AES-256-GCM, 96-bit nonce, 128-bit tag appended to the ciphertext. Used both for
//!   record payloads (key = DEK) and for wrapping the DEK (key = KEK).
//! - **KDF**: HKDF-SHA256 derives the vault MAC key from the DEK.
//! - **MAC**: HMAC-SHA256 over a length-prefixed canonical byte string (see [`super::format`]),
//!   verified in constant time.
//!
//! ## Semantics & Architecture
//! - Stateless free functions; no globals. Key material is passed as fixed-size arrays and the
//!   caller is responsible for holding it in [`zeroize`]-ing buffers.
//! - Error strategy: all failures map to [`EdgeCommonsError::Credentials`] with a non-sensitive message
//!   (never includes key or plaintext bytes).
//!
//! ## Safety & Panics
//! Does not panic on attacker-controlled input: AEAD open and MAC verify return `Err`, not panic.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::EdgeCommonsError;
use crate::Result;

/// AES-256-GCM key/DEK/KEK length in bytes.
pub const KEY_LEN: usize = 32;
/// AES-GCM nonce length in bytes (96-bit, the standard/interoperable choice).
pub const NONCE_LEN: usize = 12;

type HmacSha256 = Hmac<Sha256>;

/// Fill a fixed-size array with cryptographically secure random bytes.
///
/// # Panics
/// Panics only if the OS RNG is unavailable, which is unrecoverable for a security primitive.
pub fn random<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).expect("OS RNG unavailable");
    buf
}

/// AES-256-GCM seal: returns `ciphertext || tag`.
///
/// # Pre-conditions
/// `nonce` must be unique per `(key, message)`. The vault generates a fresh random nonce per
/// record version and per DEK-wrap, stored alongside the ciphertext.
pub fn seal(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| EdgeCommonsError::Credentials("AEAD seal failed".into()))
}

/// AES-256-GCM open of `ciphertext || tag`. Fails (does not panic) on a bad key/nonce/AAD/tag.
pub fn open(key: &[u8; KEY_LEN], nonce: &[u8; NONCE_LEN], aad: &[u8], ct_and_tag: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct_and_tag, aad })
        .map(Zeroizing::new)
        .map_err(|_| EdgeCommonsError::Credentials("AEAD open failed (wrong key, tampered data, or AAD mismatch)".into()))
}

/// Derive the vault MAC key from the DEK: `HKDF-SHA256(ikm=DEK, salt=vaultId, info="…/mac")`.
///
/// Domain-separated from the encryption use of the DEK so the MAC key and the AEAD key are
/// independent.
pub fn derive_mac_key(dek: &[u8; KEY_LEN], vault_id: &str) -> Zeroizing<[u8; KEY_LEN]> {
    let hk = Hkdf::<Sha256>::new(Some(vault_id.as_bytes()), dek);
    let mut okm = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(b"edgecommons-vault/v1/mac", okm.as_mut())
        .expect("HKDF expand of 32 bytes never fails");
    okm
}

/// HMAC-SHA256 of `input` under `mac_key`.
pub fn hmac(mac_key: &[u8; KEY_LEN], input: &[u8]) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(mac_key).expect("HMAC accepts any key length");
    m.update(input);
    m.finalize().into_bytes().into()
}

/// Constant-time verification that `HMAC-SHA256(mac_key, input) == expected`.
pub fn hmac_verify(mac_key: &[u8; KEY_LEN], input: &[u8], expected: &[u8]) -> bool {
    let mut m = <HmacSha256 as Mac>::new_from_slice(mac_key).expect("HMAC accepts any key length");
    m.update(input);
    m.verify_slice(expected).is_ok()
}
