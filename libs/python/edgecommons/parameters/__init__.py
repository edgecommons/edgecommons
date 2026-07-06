"""Parameters (``gg.get_parameters()``) — an independent, offline-first service for externalized
**configuration parameters**, paralleling ``credentials`` (secrets) but for non-secret-by-default
settings a component reads from a central/host source (AWS SSM Parameter Store, a mounted
ConfigMap/Secret directory, environment variables, or a custom backend).

Like the vault, reads are served from a local cache (never the network), so a component keeps
running when the source is unreachable. The cache is source-aware: a remote source persists
encrypted (reusing the credentials :class:`~edgecommons.credentials.vault.LocalVault` on-disk format)
while an already-local source (``mountedDir``, ``env``) uses an in-memory cache.

The on-disk format and the API surface mirror the Java/Rust/TS ports.

Example::

    from edgecommons.parameters import open_from_config
    params = open_from_config({"source": {"type": "env", "prefix": "GG_PARAM_"},
                               "sync": {"names": ["/myapp/db/host"]}})
    host = params.get("/myapp/db/host")        # offline-first, from the local cache
    pool = params.get_int("/myapp/db/poolSize")
"""
from .config import open_from_config
from .errors import ParameterError
from .service import DefaultParameterService, ParameterService, ParameterStats
from .source import (
    EnvSource,
    MountedDirSource,
    ParamValue,
    ParameterSource,
    is_projection_artifact,
)

__all__ = [
    "open_from_config",
    "ParameterError",
    "ParameterService",
    "DefaultParameterService",
    "ParameterStats",
    "ParameterSource",
    "ParamValue",
    "EnvSource",
    "MountedDirSource",
    "is_projection_artifact",
]
