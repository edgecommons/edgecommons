"""Vault cryptographic primitives (must match the Rust reference byte-for-byte).

AES-256-GCM (96-bit nonce, 128-bit tag appended), HKDF-SHA256 for the MAC key, HMAC-SHA256
with constant-time verify. See ``docs/CREDENTIALS.md`` §4 and ``vault-test-vectors/``.
"""
import hashlib
import hmac as _hmac
import os

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.hashes import SHA256
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

from .errors import CredentialError

KEY_LEN = 32
NONCE_LEN = 12


def random(n: int) -> bytes:
    """``n`` cryptographically secure random bytes."""
    return os.urandom(n)


def seal(key: bytes, nonce: bytes, aad: bytes, plaintext: bytes) -> bytes:
    """AES-256-GCM seal; returns ``ciphertext || tag``."""
    return AESGCM(key).encrypt(nonce, plaintext, aad)


def open_(key: bytes, nonce: bytes, aad: bytes, ct_and_tag: bytes) -> bytes:
    """AES-256-GCM open; raises :class:`CredentialError` (never returns plaintext) on failure."""
    try:
        return AESGCM(key).decrypt(nonce, ct_and_tag, aad)
    except InvalidTag:
        raise CredentialError("AEAD open failed (wrong key, tampered data, or AAD mismatch)") from None


def derive_mac_key(dek: bytes, vault_id: str) -> bytes:
    """``HKDF-SHA256(ikm=dek, salt=vault_id, info="ggcommons-vault/v1/mac")`` → 32 bytes."""
    return HKDF(algorithm=SHA256(), length=KEY_LEN, salt=vault_id.encode("utf-8"),
                info=b"ggcommons-vault/v1/mac").derive(dek)


def hmac_sha256(mac_key: bytes, data: bytes) -> bytes:
    """HMAC-SHA256 of ``data`` under ``mac_key``."""
    return _hmac.new(mac_key, data, hashlib.sha256).digest()


def hmac_verify(mac_key: bytes, data: bytes, expected: bytes) -> bool:
    """Constant-time check that ``HMAC-SHA256(mac_key, data) == expected``."""
    return _hmac.compare_digest(hmac_sha256(mac_key, data), expected)
