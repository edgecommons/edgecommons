"""Key providers (KEK custodians). Phase 1 ships :class:`FileKeyProvider`.

The DEK is wrapped with AES-256-GCM under the KEK, AAD-bound to the vault id — identical to the
Rust reference, so a vault wrapped by one language unwraps in another.
"""
import base64
import os
from abc import ABC, abstractmethod

from . import crypto
from .errors import CredentialError
from .format import dek_wrap_aad


class KeyProvider(ABC):
    """Wraps/unwraps the vault DEK without exposing the KEK."""

    @property
    @abstractmethod
    def provider_id(self) -> str:
        ...

    @abstractmethod
    def wrap_dek(self, vault_id: str, dek: bytes) -> dict:
        """Return the ``kek`` dict persisted in the vault file."""

    @abstractmethod
    def unwrap_dek(self, vault_id: str, kek: dict) -> bytes:
        """Recover the DEK from a ``kek`` dict."""


class FileKeyProvider(KeyProvider):
    """KEK held as 32 bytes in a local key file (standalone / offline-fallback custodian)."""

    def __init__(self, kek: bytes):
        if len(kek) != crypto.KEY_LEN:
            raise CredentialError(f"KEK must be {crypto.KEY_LEN} bytes")
        self._kek = kek

    @classmethod
    def from_keyfile(cls, path: str) -> "FileKeyProvider":
        with open(path, "rb") as f:
            return cls(f.read())

    @classmethod
    def generate_keyfile(cls, path: str) -> "FileKeyProvider":
        kek = crypto.random(crypto.KEY_LEN)
        with open(path, "wb") as f:
            f.write(kek)
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass
        return cls(kek)

    @property
    def provider_id(self) -> str:
        return "file"

    def wrap_dek(self, vault_id: str, dek: bytes) -> dict:
        nonce = crypto.random(crypto.NONCE_LEN)
        wrapped = crypto.seal(self._kek, nonce, dek_wrap_aad(vault_id), dek)
        return {
            "provider": "file",
            "alg": "AES-256-GCM",
            "wrapNonce": base64.b64encode(nonce).decode("ascii"),
            "wrappedDek": base64.b64encode(wrapped).decode("ascii"),
        }

    def unwrap_dek(self, vault_id: str, kek: dict) -> bytes:
        nonce_b = kek.get("wrapNonce")
        if not nonce_b:
            raise CredentialError("file KEK: missing wrapNonce")
        nonce = base64.b64decode(nonce_b)
        wrapped = base64.b64decode(kek["wrappedDek"])
        return crypto.open_(self._kek, nonce, dek_wrap_aad(vault_id), wrapped)
