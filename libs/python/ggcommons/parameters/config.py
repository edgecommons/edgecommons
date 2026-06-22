"""Parameters config.

Parse the ``parameters`` config section and build a :class:`DefaultParameterService` from it —
selecting the :class:`~ggcommons.parameters.source.ParameterSource` backend, choosing a source-aware
cache (persistent-encrypted for remote sources, in-memory for already-local ones), and optionally
bootstrapping the declared names/paths into the cache.

Phase 1 ships three sources: ``awsSsm`` (remote; needs boto3), ``mountedDir`` (K8s ConfigMap/Secret
volumes, Docker secrets), and ``env``. Numeric fields parse leniently because Greengrass delivers
config numbers as doubles.

Mirrors the Rust reference (``libs/rust/src/parameters/config.rs``). Uses plain dicts (no
dataclasses) and an ``open_from_config(parameters_cfg, namespace="")`` entry, matching the
credentials module's style.
"""
import logging
import threading
from typing import List, Optional, Tuple

from ..credentials import LocalVault, build_key_provider
from .errors import ParameterError
from .service import DefaultParameterService
from .source import EnvSource, MountedDirSource, ParameterSource

logger = logging.getLogger("ggcommons.parameters")

# Remote (network-backed) source kinds — drive the default cache persistence.
_REMOTE_KINDS = ("awsSsm",)


def _lenient_int(value, default: int) -> int:
    """Greengrass delivers config numbers as doubles (300.0). Accept int or integer-valued float."""
    if value is None:
        return default
    if isinstance(value, bool):
        raise ParameterError("expected a number")
    if isinstance(value, (int, float)):
        return int(value)
    raise ParameterError(f"expected a number, got {value!r}")


def _path_entries(paths) -> List[Tuple[str, bool]]:
    """Normalize sync.paths entries to (path, recursive) tuples. A bare string is recursive; an
    object ``{"path": ..., "recursive": <bool>}`` honours its flag (default recursive)."""
    out: List[Tuple[str, bool]] = []
    for entry in paths or []:
        if isinstance(entry, str):
            out.append((entry, True))
        elif isinstance(entry, dict) and "path" in entry:
            out.append((entry["path"], bool(entry.get("recursive", True))))
        else:
            raise ParameterError(f"invalid sync.paths entry: {entry!r}")
    return out


def _build_source(source_cfg: dict) -> ParameterSource:
    """Build the :class:`ParameterSource` backend named by ``source.type``."""
    cfg = source_cfg or {}
    kind = cfg.get("type", "none")
    if kind == "env":
        prefix = cfg.get("prefix") or "GG_PARAM_"
        return EnvSource(prefix)
    if kind == "mountedDir":
        root = cfg.get("root")
        if not root:
            raise ParameterError("mountedDir source requires source.root")
        return MountedDirSource(root, cfg.get("securePaths", []) or [])
    if kind == "awsSsm":
        from .ssm import AwsSsmSource
        return AwsSsmSource(
            region=cfg.get("region"),
            endpoint_url=cfg.get("endpointUrl"),
            with_decryption=bool(cfg.get("withDecryption", True)),
        )
    raise ParameterError(
        f"parameter source '{kind}' is not available "
        "(supported: 'env', 'mountedDir', 'awsSsm')"
    )


def open_from_config(parameters_cfg: dict, namespace: str = "") -> DefaultParameterService:
    """Build a :class:`DefaultParameterService` from a parsed ``parameters`` config dict.

    Select the source backend, pick a source-aware cache (persistent-encrypted for remote sources,
    in-memory for local ones — overridable via ``cache.persist``), wire the declared sync
    names/paths, optionally bootstrap the cache from the source, then start the background refresh.

    ``namespace`` is accepted to mirror the credentials entry point but, like the Rust port, the
    parameter keys are **not** namespaced (the cache path is already per-component templated).
    """
    cfg = parameters_cfg or {}
    source_cfg = cfg.get("source", {}) or {}
    source = _build_source(source_cfg)

    sync_cfg = cfg.get("sync", {}) or {}
    sync_names = list(sync_cfg.get("names", []) or [])
    sync_paths = _path_entries(sync_cfg.get("paths", []))

    refresh_interval_secs = _lenient_int(cfg.get("refreshIntervalSecs"), 300)
    bootstrap = bool(cfg.get("bootstrapOnStart", True))

    # Source-aware default: remote sources persist encrypted (survive restart/offline); local
    # sources stay in memory (the backend is itself always available). `cache.persist` overrides.
    cache_cfg = cfg.get("cache", {}) or {}
    persist = cache_cfg.get("persist")
    if persist is None:
        persist = source_cfg.get("type") in _REMOTE_KINDS
    else:
        persist = bool(persist)

    if persist:
        path = cache_cfg.get("path", "param-cache")
        provider = build_key_provider(cache_cfg.get("keyProvider", {}) or {}, f"{path}.key")
        # keep_versions=1: the cache only ever needs the latest value of each parameter.
        vault = LocalVault.open(path, provider, 1)
        lock = threading.Lock()
        service = DefaultParameterService.with_persistent_cache(
            source, vault, lock, sync_names, sync_paths
        )
    else:
        service = DefaultParameterService.with_memory_cache(source, sync_names, sync_paths)

    if bootstrap:
        # Offline-first: a bootstrap failure is non-fatal — the component starts and can retry via
        # refresh(). A persisted cache from a prior run still serves reads while the source is down.
        try:
            service.refresh()
        except Exception as e:
            logger.warning(f"parameter bootstrap refresh failed (continuing; cache may be empty): {e}")

    # Background refresh on the configured interval (0 disables; the thread stops on close()).
    return service.with_refresh(refresh_interval_secs)
