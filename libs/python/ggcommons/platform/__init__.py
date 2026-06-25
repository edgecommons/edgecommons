"""
The platform x transport runtime model (DESIGN-core / docs/platform).

Exposes the two runtime axes (:class:`Platform`, :class:`Transport`), the
platform-profile table and the pure precedence resolver / auto-detector that
maps parse-time inputs to a single :class:`ResolvedProfile` consumed by every
subsystem initializer. Mirrors the canonical Java ``com.breissinger.ggcommons.platform``.
"""

from ggcommons.platform.platform import Platform
from ggcommons.platform.transport import Transport
from ggcommons.platform.resolver import (
    DEFAULT_IDENTITY,
    ENV_GG_IPC_SOCKET,
    ENV_GG_SVCUID,
    ENV_K8S_SERVICE_HOST,
    ENV_THING_NAME,
    K8S_SA_TOKEN_PATH,
    PROFILES,
    PlatformProfile,
    ResolvedProfile,
    ResolverInputs,
    detect_platform,
    resolve_identity,
    resolve_profile,
    validate,
)

__all__ = [
    "Platform",
    "Transport",
    "PlatformProfile",
    "ResolvedProfile",
    "ResolverInputs",
    "PROFILES",
    "detect_platform",
    "resolve_identity",
    "resolve_profile",
    "validate",
    "DEFAULT_IDENTITY",
    "ENV_GG_IPC_SOCKET",
    "ENV_GG_SVCUID",
    "ENV_THING_NAME",
    "ENV_K8S_SERVICE_HOST",
    "K8S_SA_TOKEN_PATH",
]
