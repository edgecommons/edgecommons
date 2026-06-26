"""Parameter sources (the pluggable seam).

A :class:`ParameterSource` is the backend the parameter service reads from — AWS SSM (cloud), a
mounted directory (K8s ConfigMap/Secret volumes, Docker secrets), env vars, or a custom
host-supplied source. The service (cache, refresh, typed reads) is identical regardless of source.

Mirrors the Rust reference (``libs/rust/src/parameters/source.rs``).
"""
import os
from abc import ABC, abstractmethod
from typing import List, Optional, Tuple

from .errors import ParameterError


class ParamValue:
    """A parameter value fetched from a source.

    ``secure`` values (SSM ``SecureString``, a ``mountedDir`` secret path, …) must never be logged.
    """

    def __init__(self, value: bytes, secure: bool = False, version: Optional[str] = None):
        self.value = value
        self.secure = secure
        self.version = version

    @classmethod
    def plain(cls, value: bytes) -> "ParamValue":
        """Construct a non-secure value."""
        return cls(value, secure=False, version=None)

    def __repr__(self) -> str:
        # Never render the raw bytes — a value may be secure.
        return f"ParamValue(secure={self.secure}, version={self.version!r}, bytes=<{len(self.value)} redacted>)"


def is_projection_artifact(file_name: str) -> bool:
    """True for kubelet/Docker volume-projection artifacts and hidden entries — anything whose file
    name begins with ``"."``.

    This is the single source of truth for the dotfile filter that skips the kubelet symlink farm
    (``..data``, ``..2026_06_25_...`` timestamped dirs, and the ``..data_tmp`` swap-staging entry).
    Reused by the ``CONFIGMAP`` config source so the filter stays identical across the parameters and
    config subsystems (FR-CFG-4). Mirrors the canonical Java ``MountedDirSource.isProjectionArtifact``.

    Args:
        file_name: the bare file name (not a path).

    Returns:
        ``True`` if the entry is a projection artifact / hidden file to ignore.
    """
    return file_name.startswith(".")


class ParameterSource(ABC):
    """The pluggable parameter backend."""

    @abstractmethod
    def fetch(self, name: str) -> Optional[ParamValue]:
        """Fetch one parameter by name, or ``None`` if it does not exist."""

    @abstractmethod
    def fetch_by_path(self, path: str, recursive: bool) -> List[Tuple[str, ParamValue]]:
        """Fetch every parameter under ``path`` (recursively when ``recursive``). Empty when absent."""

    @abstractmethod
    def source_id(self) -> str:
        """Stable id for diagnostics/stats (e.g. ``"awsSsm"``, ``"mountedDir"``, ``"env"``)."""


# ---------------------------------------------------------------------------
# EnvSource — parameters from environment variables (containers / dev / STANDALONE).
# ---------------------------------------------------------------------------


class EnvSource(ParameterSource):
    """Reads parameters from environment variables under a prefix.

    A name ``/myapp/db/host`` maps to the env var ``<PREFIX>MYAPP_DB_HOST`` and back. Values are
    treated as non-secure (env is plaintext).
    """

    def __init__(self, prefix: str):
        self._prefix = prefix

    def _to_env(self, name: str) -> str:
        body = "".join(
            "_" if c in "/-." else c.upper()
            for c in name.lstrip("/")
        )
        return f"{self._prefix}{body}"

    def _from_env(self, var: str) -> Optional[str]:
        if not var.startswith(self._prefix):
            return None
        rest = var[len(self._prefix):]
        return "/" + rest.lower().replace("_", "/")

    def fetch(self, name: str) -> Optional[ParamValue]:
        val = os.environ.get(self._to_env(name))
        if val is None:
            return None
        return ParamValue.plain(val.encode("utf-8"))

    def fetch_by_path(self, path: str, recursive: bool) -> List[Tuple[str, ParamValue]]:
        out: List[Tuple[str, ParamValue]] = []
        for k, v in os.environ.items():
            name = self._from_env(k)
            if name is not None and name.startswith(path):
                out.append((name, ParamValue.plain(v.encode("utf-8"))))
        return out

    def source_id(self) -> str:
        return "env"


# ---------------------------------------------------------------------------
# MountedDirSource — parameters from a directory tree (K8s ConfigMap/Secret volumes,
# Docker secrets at /run/secrets, bare config dirs). No API client / RBAC needed.
# ---------------------------------------------------------------------------


class MountedDirSource(ParameterSource):
    """Reads parameters from files under a root directory.

    A file at ``<root>/myapp/db/host`` is the parameter ``/myapp/db/host`` with the file's bytes as
    its value. Files whose parameter name falls under one of ``secure_paths`` are flagged ``secure``
    (a K8s Secret volume vs a ConfigMap volume).
    """

    def __init__(self, root: str, secure_paths: List[str]):
        self._root = root
        self._secure_paths = secure_paths or []

    def _is_secure(self, name: str) -> bool:
        return any(name.startswith(p) for p in self._secure_paths)

    def _name_to_path(self, name: str) -> str:
        return os.path.join(self._root, name.lstrip("/"))

    def _walk(self, directory: str, recursive: bool, out: List[Tuple[str, ParamValue]]) -> None:
        """Recursively collect files under ``directory`` into ``out``, keyed by parameter name
        (relative to root, ``/``-separated). Skips dotfiles/dirs — K8s projects volumes with internal
        ``..data`` / ``..2025_…`` symlinked entries that must not be surfaced as parameters."""
        try:
            entries = os.listdir(directory)
        except FileNotFoundError:
            return
        except OSError as e:
            raise ParameterError(f"read dir {directory}: {e}") from None
        for fname in entries:
            if is_projection_artifact(fname):
                continue  # K8s internal (..data, ..2025_...) / hidden
            path = os.path.join(directory, fname)
            if os.path.isdir(path):
                if recursive:
                    self._walk(path, recursive, out)
            else:
                rel = os.path.relpath(path, self._root)
                name = "/" + rel.replace("\\", "/")
                try:
                    with open(path, "rb") as f:
                        value = f.read()
                except OSError as e:
                    raise ParameterError(f"read {path}: {e}") from None
                out.append((name, ParamValue(value, secure=self._is_secure(name), version=None)))

    def fetch(self, name: str) -> Optional[ParamValue]:
        path = self._name_to_path(name)
        if os.path.isdir(path):
            # A directory (not a file) at that name is "not a parameter".
            return None
        try:
            with open(path, "rb") as f:
                value = f.read()
        except FileNotFoundError:
            return None
        except IsADirectoryError:
            return None
        except OSError as e:
            raise ParameterError(f"read {path}: {e}") from None
        return ParamValue(value, secure=self._is_secure(name), version=None)

    def fetch_by_path(self, path: str, recursive: bool) -> List[Tuple[str, ParamValue]]:
        base = self._name_to_path(path)
        out: List[Tuple[str, ParamValue]] = []
        self._walk(base, recursive, out)
        return out

    def source_id(self) -> str:
        return "mountedDir"
