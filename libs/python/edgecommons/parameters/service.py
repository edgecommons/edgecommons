"""Parameter service.

``gg.get_parameters()`` returns a :class:`ParameterService` — offline-first, source-agnostic reads
of externalized parameters. :class:`DefaultParameterService` caches whatever a
:class:`~edgecommons.parameters.source.ParameterSource` provides and serves reads from the cache
(never the network), refreshing the declared names/paths selectively in the background / on demand.

The cache is **source-aware**: a remote source (SSM, …) uses a persistent **encrypted** cache —
reusing the credentials :class:`~edgecommons.credentials.vault.LocalVault` (same normative on-disk
format) — so values survive restarts and offline. An already-local source (``mountedDir``, ``env``)
uses an in-memory cache.

Mirrors the Rust reference (``libs/rust/src/parameters/service.rs``).
"""
import json
import logging
import threading
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Dict, List, Optional, Tuple

from .errors import ParameterError
from .source import ParameterSource

logger = logging.getLogger("edgecommons.parameters")

SECURE_LABEL = "secure"
VERSION_LABEL = "pversion"


def _now_ms() -> int:
    return int(time.time() * 1000)


class _Cached:
    """A cached parameter value (decrypted, in memory). ``secure`` values must not be logged."""

    __slots__ = ("value", "secure", "version")

    def __init__(self, value: bytes, secure: bool, version: Optional[str]):
        self.value = value
        self.secure = secure
        self.version = version


class _ParamCache(ABC):
    """The cache layer behind the service (offline-first read store)."""

    @abstractmethod
    def get(self, name: str) -> Optional[_Cached]:
        ...

    @abstractmethod
    def put(self, name: str, c: _Cached) -> None:
        ...

    @abstractmethod
    def entries(self, prefix: str) -> List[Tuple[str, _Cached]]:
        ...

    @abstractmethod
    def __len__(self) -> int:
        ...


class _MemoryCache(_ParamCache):
    """In-memory cache for already-local sources (``mountedDir``, ``env``)."""

    def __init__(self):
        self._map: Dict[str, _Cached] = {}
        self._lock = threading.Lock()

    def get(self, name: str) -> Optional[_Cached]:
        with self._lock:
            return self._map.get(name)

    def put(self, name: str, c: _Cached) -> None:
        with self._lock:
            self._map[name] = c

    def entries(self, prefix: str) -> List[Tuple[str, _Cached]]:
        with self._lock:
            return [(k, v) for k, v in sorted(self._map.items()) if k.startswith(prefix)]

    def __len__(self) -> int:
        with self._lock:
            return len(self._map)


class _VaultCache(_ParamCache):
    """Persistent encrypted cache for remote sources — reuses the credentials
    :class:`~edgecommons.credentials.vault.LocalVault` (the same normative, cross-language on-disk
    format). The parameter value is the secret bytes; ``secure`` and the upstream version ride along
    as labels."""

    def __init__(self, vault, lock: threading.Lock):
        self._vault = vault
        self._lock = lock

    def get(self, name: str) -> Optional[_Cached]:
        with self._lock:
            self._vault.reload_if_changed()
            s = self._vault.get(name)
        if s is None:
            return None
        return _Cached(
            value=s.bytes(),
            secure=s.labels.get(SECURE_LABEL) == "true",
            version=s.labels.get(VERSION_LABEL),
        )

    def put(self, name: str, c: _Cached) -> None:
        labels = {SECURE_LABEL: "true" if c.secure else "false"}
        if c.version is not None:
            labels[VERSION_LABEL] = c.version
        with self._lock:
            self._vault.reload_if_changed()
            self._vault.put(name, c.value, source="parameter", labels=labels)

    def entries(self, prefix: str) -> List[Tuple[str, _Cached]]:
        with self._lock:
            self._vault.reload_if_changed()
            metas = self._vault.list(prefix)
            out: List[Tuple[str, _Cached]] = []
            for m in metas:
                s = self._vault.get(m.name)
                if s is not None:
                    out.append((m.name, _Cached(
                        value=s.bytes(),
                        secure=s.labels.get(SECURE_LABEL) == "true",
                        version=s.labels.get(VERSION_LABEL),
                    )))
        return out

    def __len__(self) -> int:
        with self._lock:
            self._vault.reload_if_changed()
            return len(self._vault.list(""))


@dataclass
class ParameterStats:
    """Non-sensitive parameter-subsystem stats."""
    parameter_count: int = 0
    # Age of the last successful refresh, ms (None if never refreshed).
    last_refresh_age_ms: Optional[int] = None
    refresh_failures: int = 0
    source: str = ""


class ParameterService(ABC):
    """The public parameter interface (depend on this, not :class:`DefaultParameterService`)."""

    @abstractmethod
    def get(self, name: str) -> Optional[str]:
        """The value of ``name`` as a UTF-8 string, or ``None``. Served from the local cache."""

    @abstractmethod
    def get_bytes(self, name: str) -> Optional[bytes]:
        """The raw value bytes of ``name``."""

    @abstractmethod
    def get_by_path(self, path: str) -> Dict[str, str]:
        """All cached parameters under ``path`` (the prefix), as name -> string value."""

    @abstractmethod
    def names(self, prefix: str) -> List[str]:
        """Cached parameter names under ``prefix`` (metadata only — no values)."""

    @abstractmethod
    def refresh(self) -> None:
        """Force an immediate pull of the declared names/paths from the source into the cache."""

    @abstractmethod
    def stats(self) -> ParameterStats:
        """Non-sensitive stats for observability."""

    # ----- typed accessors -----
    def get_int(self, name: str) -> Optional[int]:
        """The value parsed as an integer."""
        s = self.get(name)
        if s is None:
            return None
        try:
            return int(s.strip())
        except ValueError as e:
            raise ParameterError(f"parameter '{name}' is not an integer: {e}") from None

    def get_bool(self, name: str) -> Optional[bool]:
        """The value parsed as a boolean (``true``/``false``/``1``/``0``, case-insensitive)."""
        s = self.get(name)
        if s is None:
            return None
        v = s.strip().lower()
        if v in ("true", "1", "yes", "on"):
            return True
        if v in ("false", "0", "no", "off"):
            return False
        raise ParameterError(f"parameter '{name}' is not a boolean: {v}")

    def get_json(self, name: str):
        """The value parsed as JSON."""
        b = self.get_bytes(name)
        if b is None:
            return None
        try:
            return json.loads(b)
        except ValueError as e:
            raise ParameterError(f"parameter '{name}' is not JSON: {e}") from None

    def get_string_list(self, name: str) -> Optional[List[str]]:
        """A ``StringList`` value (comma-separated) as a list."""
        s = self.get(name)
        if s is None:
            return None
        if s == "":
            return []
        return [x.strip() for x in s.split(",")]


class _Inner:
    """The shared refresh-able core (source + cache + selection + counters). Shared between the
    background refresh thread and the service so they operate on the same state."""

    def __init__(self, source: ParameterSource, cache: _ParamCache,
                 sync_names: List[str], sync_paths: List[Tuple[str, bool]]):
        self.source = source
        self.cache = cache
        self.sync_names = sync_names
        self.sync_paths = sync_paths
        self._counter_lock = threading.Lock()
        self.last_refresh_ms: Optional[int] = None
        self.failures = 0

    def refresh(self) -> None:
        any_err: Optional[Exception] = None
        for name in self.sync_names:
            try:
                v = self.source.fetch(name)
            except Exception as e:
                logger.warning(f"parameter refresh failed for '{name}' (keeping cached value): {e}")
                any_err = e
                continue
            if v is not None:
                self.cache.put(name, _Cached(v.value, v.secure, v.version))
        for path, recursive in self.sync_paths:
            try:
                items = self.source.fetch_by_path(path, recursive)
            except Exception as e:
                logger.warning(f"parameter path refresh failed for '{path}' (keeping cached values): {e}")
                any_err = e
                continue
            for name, v in items:
                self.cache.put(name, _Cached(v.value, v.secure, v.version))
        if any_err is not None:
            with self._counter_lock:
                self.failures += 1
            # Offline-first: a refresh failure is non-fatal when we already have cached values.
            if len(self.cache) == 0:
                raise any_err
        else:
            with self._counter_lock:
                self.last_refresh_ms = _now_ms()


class _Refresher:
    """Owns the background refresh thread (daemon); stops + joins it on :meth:`close`. Mirrors the
    credentials SyncEngine daemon-thread + stop-flag + join-on-close pattern."""

    def __init__(self, inner: _Inner, interval_secs: int):
        self._inner = inner
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._loop, args=(interval_secs,), daemon=True)
        self._thread.start()

    def _loop(self, interval_secs: int) -> None:
        while not self._stop.wait(interval_secs):
            try:
                self._inner.refresh()
            except Exception:
                # Already counted/logged in Inner.refresh; keep the thread alive.
                pass

    def close(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2)


class DefaultParameterService(ParameterService):
    """Default :class:`ParameterService`: a :class:`ParameterSource` behind an offline-first cache,
    optionally refreshed by a background thread."""

    def __init__(self, source: ParameterSource, cache: _ParamCache,
                 sync_names: List[str], sync_paths: List[Tuple[str, bool]]):
        self._inner = _Inner(source, cache, sync_names, sync_paths)
        self._refresher: Optional[_Refresher] = None

    @classmethod
    def with_persistent_cache(cls, source: ParameterSource, vault, lock: threading.Lock,
                              sync_names: List[str], sync_paths: List[Tuple[str, bool]]
                              ) -> "DefaultParameterService":
        """Build with a persistent encrypted cache (the credentials LocalVault) — for remote sources."""
        return cls(source, _VaultCache(vault, lock), sync_names, sync_paths)

    @classmethod
    def with_memory_cache(cls, source: ParameterSource,
                          sync_names: List[str], sync_paths: List[Tuple[str, bool]]
                          ) -> "DefaultParameterService":
        """Build with an in-memory cache — for already-local sources (``mountedDir``, ``env``)."""
        return cls(source, _MemoryCache(), sync_names, sync_paths)

    def with_refresh(self, interval_secs: int) -> "DefaultParameterService":
        """Start a background refresh thread that re-pulls the declared names/paths every
        ``interval_secs`` (0 disables it). The thread stops on :meth:`close`. Fluent; returns self."""
        if interval_secs > 0:
            self._refresher = _Refresher(self._inner, interval_secs)
        return self

    def get(self, name: str) -> Optional[str]:
        b = self.get_bytes(name)
        if b is None:
            return None
        try:
            return b.decode("utf-8")
        except UnicodeDecodeError:
            raise ParameterError(f"parameter '{name}' is not UTF-8") from None

    def get_bytes(self, name: str) -> Optional[bytes]:
        c = self._inner.cache.get(name)
        return c.value if c is not None else None

    def get_by_path(self, path: str) -> Dict[str, str]:
        out: Dict[str, str] = {}
        for name, c in self._inner.cache.entries(path):
            try:
                out[name] = c.value.decode("utf-8")
            except UnicodeDecodeError:
                continue
        return out

    def names(self, prefix: str) -> List[str]:
        return [n for n, _ in self._inner.cache.entries(prefix)]

    def refresh(self) -> None:
        self._inner.refresh()

    def stats(self) -> ParameterStats:
        with self._inner._counter_lock:
            last = self._inner.last_refresh_ms
            failures = self._inner.failures
        return ParameterStats(
            parameter_count=len(self._inner.cache),
            last_refresh_age_ms=(max(0, _now_ms() - last) if last is not None else None),
            refresh_failures=failures,
            source=self._inner.source.source_id(),
        )

    def close(self) -> None:
        """Stop the background refresh thread (if any). RAII isn't a thing in Python — mirror how
        SyncEngine / CredentialMetricsBridge expose ``close()``."""
        if self._refresher is not None:
            self._refresher.close()
            self._refresher = None
