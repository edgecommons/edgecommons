"""
Typed view over the optional ``health`` config section (Phase 1c health slice, FR-HB-1).

The ``health`` section configures the minimal HTTP health server that exposes the Kubernetes
liveness/readiness/startup probes. It is parsed verbatim from the canonical schema's ``health``
object::

    {
        "enabled": true,            # explicit on/off; absent => platform-profile default
        "port": 8081,               # TCP port the server binds (0.0.0.0)
        "livenessPath": "/livez",   # GET -> 200 while the process is alive
        "readinessPath": "/readyz", # GET -> 200 only when connected && ready && !shuttingDown
        "startupPath": "/startupz"  # GET -> reuses readiness semantics
    }

``enabled`` is intentionally tri-state: ``None`` means "not specified in config" so the caller can
apply the precedence *explicit config ▸ platform-profile default (on for KUBERNETES) ▸ off* (FR-RT-3).
The path/port defaults mirror the schema defaults exactly, so the parsed object is usable even when the
config omits the ``health`` section entirely. Mirrors the canonical Java ``HealthConfiguration``.
"""

import json
from typing import Optional


class HealthConfiguration:
    """Parsed ``health`` config section with schema-aligned defaults."""

    #: Default port the health server binds when the config omits ``port`` (matches the schema).
    DEFAULT_PORT = 8081
    #: Default liveness probe path.
    DEFAULT_LIVENESS_PATH = "/livez"
    #: Default readiness probe path.
    DEFAULT_READINESS_PATH = "/readyz"
    #: Default startup probe path.
    DEFAULT_STARTUP_PATH = "/startupz"

    def __init__(self, health_json: Optional[dict]):
        # None == "not specified" so the resolver's KUBERNETES default can apply (FR-RT-3).
        self._enabled: Optional[bool] = None
        self._port = HealthConfiguration.DEFAULT_PORT
        self._liveness_path = HealthConfiguration.DEFAULT_LIVENESS_PATH
        self._readiness_path = HealthConfiguration.DEFAULT_READINESS_PATH
        self._startup_path = HealthConfiguration.DEFAULT_STARTUP_PATH

        if health_json is not None:
            if "enabled" in health_json and health_json.get("enabled") is not None:
                self._enabled = bool(health_json.get("enabled"))
            self._port = int(health_json.get("port", self._port))
            self._liveness_path = health_json.get("livenessPath", self._liveness_path)
            self._readiness_path = health_json.get("readinessPath", self._readiness_path)
            self._startup_path = health_json.get("startupPath", self._startup_path)

    @property
    def enabled(self) -> Optional[bool]:
        """Explicit ``health.enabled`` (``True``/``False``), or ``None`` when the config omits it.

        ``None`` signals the caller to fall through to the platform-profile default (on for
        KUBERNETES, off elsewhere) per the FR-RT-3 precedence.
        """
        return self._enabled

    @property
    def port(self) -> int:
        """TCP port the server binds on ``0.0.0.0`` (default 8081)."""
        return self._port

    @property
    def liveness_path(self) -> str:
        """Path served by the liveness probe (default ``/livez``)."""
        return self._liveness_path

    @property
    def readiness_path(self) -> str:
        """Path served by the readiness probe (default ``/readyz``)."""
        return self._readiness_path

    @property
    def startup_path(self) -> str:
        """Path served by the startup probe (default ``/startupz``); reuses readiness semantics."""
        return self._startup_path

    def to_dict(self) -> dict:
        """Round-trip the parsed values back to the schema shape (``enabled`` omitted when unset)."""
        out = {
            "port": self._port,
            "livenessPath": self._liveness_path,
            "readinessPath": self._readiness_path,
            "startupPath": self._startup_path,
        }
        if self._enabled is not None:
            out["enabled"] = self._enabled
        return out

    def __str__(self) -> str:
        return json.dumps(self.to_dict(), indent=2)
