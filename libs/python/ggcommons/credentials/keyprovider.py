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


class KmsKeyProvider(KeyProvider):
    """KMS-wrapped DEK custodian: the DEK is encrypted by an AWS KMS CMK (the KEK never leaves KMS)
    and unwrapped via ``kms:Decrypt`` — using AWS creds / TES on Greengrass. The encryption context
    binds the wrapped DEK to the vault id (anti-swap). Mirrors the Rust ``mod kms``.

    ``boto3`` is imported lazily so non-KMS components don't require it at import time.
    """

    def __init__(self, key_id: str, region: str = None, endpoint_url: str = None):
        import boto3  # imported lazily so non-KMS components don't require it at import time

        if not key_id:
            raise CredentialError("kms key provider requires keyProvider.kmsKeyId")
        self._key_id = key_id
        self._client = boto3.client("kms", region_name=region, endpoint_url=endpoint_url)

    @property
    def provider_id(self) -> str:
        return "kms"

    def wrap_dek(self, vault_id: str, dek: bytes) -> dict:
        try:
            resp = self._client.encrypt(
                KeyId=self._key_id,
                Plaintext=dek,
                EncryptionContext={"vaultId": vault_id},
            )
        except Exception as e:
            raise CredentialError(f"kms encrypt: {e}") from None
        ct = resp.get("CiphertextBlob")
        if ct is None:
            raise CredentialError("kms encrypt: no ciphertext")
        return {
            "provider": "kms",
            "alg": "aws-kms",
            "wrappedDek": base64.b64encode(bytes(ct)).decode("ascii"),
            "kmsKeyId": self._key_id,
        }

    def unwrap_dek(self, vault_id: str, kek: dict) -> bytes:
        try:
            ct = base64.b64decode(kek["wrappedDek"])
        except Exception:
            raise CredentialError("kms: bad wrappedDek") from None
        try:
            resp = self._client.decrypt(
                CiphertextBlob=ct,
                KeyId=self._key_id,
                EncryptionContext={"vaultId": vault_id},
            )
        except Exception as e:
            raise CredentialError(f"kms decrypt: {e}") from None
        pt = resp.get("Plaintext")
        if pt is None:
            raise CredentialError("kms decrypt: no plaintext")
        return bytes(pt)


class Pkcs11KeyProvider(KeyProvider):
    """PKCS#11 (HSM/TPM/SoftHSM) DEK custodian — mirrors the Rust ``Pkcs11KeyProvider``. A
    non-extractable AES-256 key on the token is the KEK; the DEK is wrapped with AES-256-GCM
    *inside* the token, so the KEK never leaves hardware. The GCM AAD binds the wrapped DEK to the
    vault id (anti-swap) — same on-disk ``kek`` shape as :class:`FileKeyProvider` (provider
    ``pkcs11``, alg ``AES-256-GCM``, wrapNonce + wrappedDek).

    Uses ``python-pkcs11`` (imported lazily so non-HSM components don't require it). Selected via
    ``keyProvider.type = "pkcs11"`` with ``modulePath`` / ``tokenLabel`` / ``keyLabel`` and
    ``pinEnv`` (preferred) or ``pin``.
    """

    def __init__(self, module_path: str, token_label: str, key_label: str, pin: str):
        import pkcs11  # imported lazily so non-HSM components don't require it at import time

        self._key_label = key_label
        self._pin = pin
        try:
            self._lib = pkcs11.lib(module_path)
            self._token = self._lib.get_token(token_label=token_label)
        except Exception as e:
            raise CredentialError(f"pkcs11 load module/token '{token_label}': {e}") from None

    @property
    def provider_id(self) -> str:
        return "pkcs11"

    def _key(self, session):
        from pkcs11 import ObjectClass

        try:
            return session.get_key(object_class=ObjectClass.SECRET_KEY, label=self._key_label)
        except Exception as e:
            raise CredentialError(f"pkcs11 find key '{self._key_label}': {e}") from None

    def wrap_dek(self, vault_id: str, dek: bytes) -> dict:
        from pkcs11 import GCMParams, Mechanism

        iv = crypto.random(crypto.NONCE_LEN)
        aad = dek_wrap_aad(vault_id)
        try:
            with self._token.open(user_pin=self._pin) as session:
                key = self._key(session)
                ct = key.encrypt(dek, mechanism=Mechanism.AES_GCM, mechanism_param=GCMParams(iv, aad, 128))
        except CredentialError:
            raise
        except Exception as e:
            raise CredentialError(f"pkcs11 wrap: {e}") from None
        return {
            "provider": "pkcs11",
            "alg": "AES-256-GCM",
            "wrapNonce": base64.b64encode(iv).decode("ascii"),
            "wrappedDek": base64.b64encode(bytes(ct)).decode("ascii"),
        }

    def unwrap_dek(self, vault_id: str, kek: dict) -> bytes:
        from pkcs11 import GCMParams, Mechanism

        nonce_b = kek.get("wrapNonce")
        if not nonce_b:
            raise CredentialError("pkcs11 KEK: missing wrapNonce")
        iv = base64.b64decode(nonce_b)
        ct = base64.b64decode(kek["wrappedDek"])
        aad = dek_wrap_aad(vault_id)
        try:
            with self._token.open(user_pin=self._pin) as session:
                key = self._key(session)
                pt = key.decrypt(ct, mechanism=Mechanism.AES_GCM, mechanism_param=GCMParams(iv, aad, 128))
        except CredentialError:
            raise
        except Exception as e:
            raise CredentialError(f"pkcs11 unwrap: {e}") from None
        return bytes(pt)
