"""
The pure precedence resolver and platform auto-detector (DESIGN-core sec 4 / 5).

Maps parse-time inputs (explicit flags, then environment, then the
platform-profile defaults) to a single :class:`ResolvedProfile` consumed by
every subsystem initializer.

One rule governs every defaultable setting::

    resolve(setting) = explicit flag > platform-profile default > library default

All functions are pure (no I/O beyond the explicitly-injected filesystem probe
used for Kubernetes detection), which keeps the resolver and detector
unit-testable in isolation.

**Phase 0:** :attr:`~ggcommons.platform.platform.Platform.GREENGRASS` and
:attr:`~ggcommons.platform.platform.Platform.HOST` both default their config
source to ``GG_CONFIG`` (a faithful re-expression of today's behavior — HOST
does *not* flip to ``FILE`` until Phase 1).

**Phase 1a:** :attr:`~ggcommons.platform.platform.Platform.KUBERNETES` now has a
profile (transport ``MQTT``, config source ``CONFIGMAP``) and resolves cleanly —
a service-account-token pod auto-detects to it. The IPC x KUBERNETES rejection
still holds (the IPC lock). Identity for KUBERNETES still uses the Phase-0
:func:`resolve_identity` env probe; the Downward-API identity, the ``prometheus``
metrics target, stdout-JSON logging and the HTTP health endpoint are deferred to
later Phase-1 sub-phases.
"""

import logging
import os
from dataclasses import dataclass
from typing import Callable, List, Mapping, Optional

from ggcommons.platform.platform import Platform
from ggcommons.platform.transport import Transport

logger = logging.getLogger("PlatformResolver")

#: Nucleus-injected env var pointing at the IPC domain socket (definitive GREENGRASS signal).
ENV_GG_IPC_SOCKET = "AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT"
#: Nucleus-injected component service-UID (definitive GREENGRASS signal).
ENV_GG_SVCUID = "SVCUID"
#: Greengrass-injected IoT Thing name (identity probe; mirrors ConfigManagerBuilder).
ENV_THING_NAME = "AWS_IOT_THING_NAME"
#: Confirming (secondary) Kubernetes signal. The token file is the primary, definitive one.
ENV_K8S_SERVICE_HOST = "KUBERNETES_SERVICE_HOST"
#: Projected service-account token path: the primary, definitive Kubernetes signal.
K8S_SA_TOKEN_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/token"

#: The library-default identity when no thing name is available (matches today's behavior).
DEFAULT_IDENTITY = "NOT_GREENGRASS"


@dataclass(frozen=True)
class PlatformProfile:
    """A platform profile: the table of per-subsystem *defaults* for a platform (DESIGN-core sec 3).

    Pure data; the resolver consults it only for settings the caller did not set
    explicitly. Phase 0 carries only the two defaultable settings the resolver
    actually injects — the default messaging ``transport`` and the default
    ``config_source``. Later phases append more fields (additive; no resolver
    change).
    """

    transport: Transport
    config_source: str


#: The platform-profile table (DESIGN-core sec 3). GREENGRASS and HOST deliberately default the
#: config source to ``GG_CONFIG`` to preserve current behavior. KUBERNETES (Phase 1a) defaults to the
#: ``MQTT`` transport and the k8s-native ``CONFIGMAP`` config source.
#:
#: TODO (Phase 1b-1d): the KUBERNETES profile's metrics/logging/credentials/streaming/identity
#: defaults (prometheus target, stdout-JSON sink, env KeyProvider, PVC buffer, Downward-API identity)
#: are not yet modeled here — for Phase 1a those subsystems keep their current library defaults.
PROFILES: Mapping[Platform, PlatformProfile] = {
    Platform.GREENGRASS: PlatformProfile(Transport.IPC, "GG_CONFIG"),
    Platform.HOST: PlatformProfile(Transport.MQTT, "GG_CONFIG"),
    Platform.KUBERNETES: PlatformProfile(Transport.MQTT, "CONFIGMAP"),
}


@dataclass(frozen=True)
class ResolverInputs:
    """The parse-time inputs to the resolver.

    Any field may be ``None``, meaning "not specified — fall back to detection /
    the profile default".

    Args:
        platform: explicit ``--platform`` value, or ``None`` for ``auto``.
        transport: explicit ``--transport`` value, or ``None`` to derive from the platform.
        config_args: explicit ``-c/--config`` vector, or ``None`` when ``-c`` is omitted.
        thing: explicit ``-t/--thing`` value, or ``None``.
    """

    platform: Optional[Platform]
    transport: Optional[Transport]
    config_args: Optional[List[str]]
    thing: Optional[str]


@dataclass(frozen=True)
class ResolvedProfile:
    """The fully resolved runtime settings every subsystem initializer consumes (DESIGN-core sec 4).

    Produced once, right after argument parse and before messaging init, from
    parse-time inputs only (flags > env > messaging-config payload).

    Args:
        platform: the resolved platform (after auto-detection / explicit flag).
        transport: the resolved messaging transport (validated against the platform).
        config_source: the resolved ``-c/--config`` argument vector (explicit, else the
            profile default as a single-element list).
        identity: the resolved IoT Thing name (identity), never ``None``.
    """

    platform: Platform
    transport: Transport
    config_source: List[str]
    identity: str


def _is_set(env: Optional[Mapping[str, str]], key: str) -> bool:
    if not env:
        return False
    value = env.get(key)
    return value is not None and value != ""


def detect_platform(
    env: Optional[Mapping[str, str]],
    file_exists: Optional[Callable[[str], bool]] = None,
) -> Platform:
    """Auto-detect the platform from the environment (DESIGN-core sec 5).

    The signal order is load-bearing: a containerized Nucleus component can set
    both Greengrass and Kubernetes signals, and GREENGRASS must win. First match
    wins; HOST is the fallback.

    Args:
        env: the process environment (e.g. ``os.environ``).
        file_exists: predicate answering whether a given path exists (the SA
            token). Defaults to the real filesystem probe.

    Returns:
        The detected platform.
    """
    if file_exists is None:
        file_exists = os.path.exists

    # 1. GREENGRASS — Nucleus-injected signals exist nowhere else (definitive).
    if _is_set(env, ENV_GG_IPC_SOCKET) or _is_set(env, ENV_GG_SVCUID):
        return Platform.GREENGRASS
    # 2. KUBERNETES — projected SA token (primary); service host (confirming/secondary).
    if file_exists(K8S_SA_TOKEN_PATH) or _is_set(env, ENV_K8S_SERVICE_HOST):
        return Platform.KUBERNETES
    # 3. HOST — fallback.
    return Platform.HOST


def validate(platform: Platform, transport: Transport) -> None:
    """Validate the platform/transport combination — the IPC lock (DESIGN-core sec 4.1).

    IPC is valid only on a Greengrass Nucleus, which provides the IPC domain
    socket.

    Raises:
        ValueError: if ``transport == IPC and platform != GREENGRASS``.
    """
    if transport == Transport.IPC and platform != Platform.GREENGRASS:
        raise ValueError(
            "IPC transport requires --platform GREENGRASS (the Nucleus provides "
            f"the IPC socket); got platform={platform.value}"
        )


def resolve_identity(
    thing: Optional[str],
    platform: Optional[Platform],
    env: Optional[Mapping[str, str]],
) -> str:
    """Resolve the IoT Thing name / identity (DESIGN-core sec 6.2).

    Order: explicit ``-t/--thing``, then the ``AWS_IOT_THING_NAME`` env probe,
    then the library fallback. For Phase 0 the GREENGRASS and HOST platforms
    share the same probe, so behavior is unchanged; KUBERNETES Downward-API
    identity is Phase 1.

    Args:
        thing: the explicit thing name, or ``None``.
        platform: the resolved platform (reserved for the Phase-1 Kubernetes branch).
        env: the process environment.

    Returns:
        The resolved identity, never ``None``.
    """
    if thing is not None:
        return thing
    from_env = None if env is None else env.get(ENV_THING_NAME)
    if from_env:  # present and non-empty (an empty AWS_IOT_THING_NAME is treated as absent)
        return from_env
    return DEFAULT_IDENTITY


def resolve_profile(
    inputs: ResolverInputs,
    env: Optional[Mapping[str, str]],
) -> ResolvedProfile:
    """Resolve the runtime profile from parse-time inputs and the environment (DESIGN-core sec 4).

    Args:
        inputs: the parsed CLI flags (any field ``None`` = unset).
        env: the process environment (typically ``os.environ``).

    Returns:
        The fully resolved profile.

    Raises:
        ValueError: if the resolved platform has no profile in this build, or the
            platform/transport combination is illegal (IPC lock).
    """
    auto_detected = inputs.platform is None
    platform = detect_platform(env) if auto_detected else inputs.platform
    basis = "auto-detected" if auto_detected else "explicit --platform"

    profile = PROFILES.get(platform)
    if profile is None:
        valid = ", ".join(sorted(p.value for p in PROFILES))
        raise ValueError(
            f"Platform {platform.value} is not supported in this build (no profile). "
            f"Valid platforms: {valid}."
        )

    transport = inputs.transport if inputs.transport is not None else profile.transport
    validate(platform, transport)

    config_source = (
        list(inputs.config_args)
        if inputs.config_args is not None
        else [profile.config_source]
    )

    identity = resolve_identity(inputs.thing, platform, env)

    logger.info(
        "Resolved platform=%s (basis=%s) transport=%s configSource=%s identity=%s",
        platform.value,
        basis,
        transport.value,
        config_source[0],
        identity,
    )

    return ResolvedProfile(platform, transport, config_source, identity)
