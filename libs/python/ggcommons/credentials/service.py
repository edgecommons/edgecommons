"""Credential service (the public seam) + Secret / SecretMeta value types."""
import json
import threading
from dataclasses import dataclass, field
from typing import Dict, List, Optional

from .errors import CredentialError


class Secret:
    """A decrypted secret value plus metadata. ``repr`` redacts the bytes — never log the value."""

    def __init__(self, name: str, version: str, _bytes: bytes, labels: Dict[str, str],
                 created_ms: int, source: str, content_type: str):
        self.name = name
        self.version = version
        self._bytes = _bytes
        self.labels = labels
        self.created_ms = created_ms
        self.source = source
        self.content_type = content_type

    def bytes(self) -> bytes:
        return self._bytes

    def as_str(self) -> str:
        try:
            return self._bytes.decode("utf-8")
        except UnicodeDecodeError:
            raise CredentialError("secret is not valid UTF-8") from None

    def as_json(self):
        try:
            return json.loads(self._bytes)
        except ValueError as e:
            raise CredentialError(f"secret is not JSON: {e}") from None

    def __repr__(self) -> str:
        return f"Secret(name={self.name!r}, version={self.version!r}, bytes=<{len(self._bytes)} redacted>)"


@dataclass(frozen=True)
class SecretMeta:
    """Metadata for a secret version — safe to log/list (no value)."""
    name: str
    version: str
    created_ms: int
    source: str
    ttl_secs: Optional[int] = None
    labels: Dict[str, str] = field(default_factory=dict)


class CredentialService:
    """Public interface over the vault. Depend on this; the default impl wraps a LocalVault."""

    def get(self, name: str) -> Optional[Secret]:
        raise NotImplementedError

    def get_version(self, name: str, version: str) -> Optional[Secret]:
        raise NotImplementedError

    def exists(self, name: str) -> bool:
        raise NotImplementedError

    def list(self, prefix: str = "") -> List[SecretMeta]:
        raise NotImplementedError

    def versions(self, name: str) -> List[str]:
        raise NotImplementedError

    def put(self, name: str, value: bytes, **opts) -> str:
        raise NotImplementedError

    def delete(self, name: str) -> bool:
        raise NotImplementedError

    # convenience views
    def get_bytes(self, name: str) -> Optional[bytes]:
        s = self.get(name)
        return s.bytes() if s else None

    def get_string(self, name: str) -> Optional[str]:
        s = self.get(name)
        return s.as_str() if s else None

    def get_json(self, name: str):
        s = self.get(name)
        return s.as_json() if s else None


class DefaultCredentialService(CredentialService):
    """A :class:`~ggcommons.credentials.vault.LocalVault` behind a lock; refreshes on read."""

    def __init__(self, vault):
        self._vault = vault
        self._lock = threading.Lock()

    def get(self, name: str) -> Optional[Secret]:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.get(name)

    def get_version(self, name: str, version: str) -> Optional[Secret]:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.get_version(name, version)

    def exists(self, name: str) -> bool:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.exists(name)

    def list(self, prefix: str = "") -> List[SecretMeta]:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.list(prefix)

    def versions(self, name: str) -> List[str]:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.versions(name)

    def put(self, name: str, value: bytes, **opts) -> str:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.put(name, value, **opts)

    def delete(self, name: str) -> bool:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.delete(name)
