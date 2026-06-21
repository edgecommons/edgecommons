"""Credential service (the public seam) + Secret / SecretMeta value types."""
import json
import threading
from dataclasses import dataclass, field, replace
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

    def refresh(self) -> None:
        """Force an immediate pull from the central source (no-op without central sync)."""
        return None

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
    """A :class:`~ggcommons.credentials.vault.LocalVault` behind a lock; refreshes on read.

    ``namespace`` (``<thingName>/<componentName>``) is transparently prepended to every key and
    stripped from returned names, so a shared device vault can't collide across components.
    """

    def __init__(self, vault, namespace: str = "", sync=None, lock: Optional[threading.Lock] = None):
        self._vault = vault
        self._lock = lock if lock is not None else threading.Lock()
        self._namespace = namespace
        self._sync = sync

    def _full(self, name: str) -> str:
        return f"{self._namespace}/{name}" if self._namespace else name

    def _rel(self, full: str) -> str:
        prefix = self._namespace + "/"
        return full[len(prefix):] if self._namespace and full.startswith(prefix) else full

    def get(self, name: str) -> Optional[Secret]:
        with self._lock:
            self._vault.reload_if_changed()
            s = self._vault.get(self._full(name))
        if s is not None:
            s.name = self._rel(s.name)
        return s

    def get_version(self, name: str, version: str) -> Optional[Secret]:
        with self._lock:
            self._vault.reload_if_changed()
            s = self._vault.get_version(self._full(name), version)
        if s is not None:
            s.name = self._rel(s.name)
        return s

    def exists(self, name: str) -> bool:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.exists(self._full(name))

    def list(self, prefix: str = "") -> List[SecretMeta]:
        with self._lock:
            self._vault.reload_if_changed()
            metas = self._vault.list(self._full(prefix))
        return [replace(m, name=self._rel(m.name)) for m in metas]

    def versions(self, name: str) -> List[str]:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.versions(self._full(name))

    def put(self, name: str, value: bytes, **opts) -> str:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.put(self._full(name), value, **opts)

    def delete(self, name: str) -> bool:
        with self._lock:
            self._vault.reload_if_changed()
            return self._vault.delete(self._full(name))

    def refresh(self) -> None:
        if self._sync is not None:
            self._sync.sync_now()
