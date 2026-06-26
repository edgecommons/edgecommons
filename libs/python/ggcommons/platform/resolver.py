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
still holds (the IPC lock).

**Phase 1b:** :func:`resolve_identity` now reads the Kubernetes Downward-API env
tier (:data:`ENV_K8S_THING_NAME` then :data:`ENV_K8S_POD_NAME`) ahead of the
generic ``AWS_IOT_THING_NAME`` probe **when** the resolved platform is
``KUBERNETES``.

**Phase 1c (logging slice):** the :class:`PlatformProfile` gains a
:attr:`~PlatformProfile.logging_format` default; the KUBERNETES profile defaults it
to :data:`LOGGING_FORMAT_JSON` (the stdout-JSON sink), while GREENGRASS/HOST leave
it ``None`` (the library console/text default). The effective logging format follows
the same one-line precedence — explicit ``logging.<lang>_format`` config ▸ this
platform-profile default ▸ library default — applied by the logging configurator
(see :func:`profile_logging_format`).

**Phase 1c (health slice):** the :class:`PlatformProfile` gains a
:attr:`~PlatformProfile.health_enabled` default; the KUBERNETES profile defaults it to
``True`` (the HTTP health server starts by default), while GREENGRASS/HOST leave it
``False`` (opt-in via ``health.enabled``). The effective enable follows the same
precedence — explicit ``health.enabled`` config ▸ this platform-profile default ▸ off —
applied where the health server is started (see :func:`profile_health_enabled`).

**Phase 1c (prometheus slice):** the :class:`PlatformProfile` gains a
:attr:`~PlatformProfile.metric_target` default; the KUBERNETES profile defaults it to
:data:`METRIC_TARGET_PROMETHEUS` (the pull-based in-process registry exposed as OpenMetrics
text at an HTTP ``/metrics`` endpoint), while GREENGRASS/HOST leave it ``None`` (the library
default ``log`` target). The effective metric target follows the same one-line precedence —
explicit ``metricEmission.target`` config ▸ this platform-profile default ▸ library default
``log`` — applied by :class:`~ggcommons.metrics.metric_emitter.MetricEmitter` when it selects
the target (see :func:`profile_metric_target`).
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
#: Kubernetes Downward-API identity (Phase 1b): the chart maps the ``ggcommons.io/thing-name`` pod
#: annotation (or an explicit value) into this env var. Highest of the KUBERNETES identity tier.
ENV_K8S_THING_NAME = "GGCOMMONS_THING_NAME"
#: Kubernetes Downward-API pod name (Phase 1b): ``metadata.name`` via a Downward-API ``fieldRef``.
#: The fallback identity on KUBERNETES when ``GGCOMMONS_THING_NAME`` is absent.
ENV_K8S_POD_NAME = "POD_NAME"
#: Kubernetes Downward-API pod namespace (``metadata.namespace`` via ``fieldRef``); a Phase-1c
#: logging *correlation* field (not an identity probe). Same env var wired by the chart in 1b.
ENV_K8S_POD_NAMESPACE = "POD_NAMESPACE"
#: Kubernetes Downward-API node name (``spec.nodeName`` via ``fieldRef``); a Phase-1c logging
#: *correlation* field. Same env var wired by the chart in 1b.
ENV_K8S_NODE_NAME = "NODE_NAME"
#: Projected service-account token path: the primary, definitive Kubernetes signal.
K8S_SA_TOKEN_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/token"

#: The library-default identity when no thing name is available (matches today's behavior).
DEFAULT_IDENTITY = "NOT_GREENGRASS"

#: The case-insensitive selector value (FR-LOG-4) that selects the stdout-JSON logging sink via the
#: per-language ``logging.<lang>_format`` token. Consistent across all four languages.
LOGGING_FORMAT_JSON = "json"

#: The pull-based metric target (FR-MET-1): an in-process registry exposed as OpenMetrics/Prometheus
#: text over HTTP. The KUBERNETES profile defaults ``metricEmission.target`` to this value. Consistent
#: across all four languages.
METRIC_TARGET_PROMETHEUS = "prometheus"

#: The offline software-KEK vault key provider (FR-CRED-3): the KEK is a base64-encoded 32-byte raw
#: key read from an env var (typically a mounted Kubernetes Secret). The KUBERNETES profile defaults
#: the credentials vault ``keyProvider.type`` to this value (FR-CRED-6). Consistent across all four
#: languages.
CREDENTIALS_KEY_PROVIDER_ENV = "env"


@dataclass(frozen=True)
class PlatformProfile:
    """A platform profile: the table of per-subsystem *defaults* for a platform (DESIGN-core sec 3).

    Pure data; the resolver consults it only for settings the caller did not set
    explicitly. Phase 0 carries the two defaultable settings the resolver actually
    injects — the default messaging ``transport`` and the default ``config_source``.
    Phase 1c appends :attr:`logging_format` (the platform's default logging-format
    token, e.g. ``"json"`` on KUBERNETES, or ``None`` to keep the library default).
    Later phases append more fields (additive; no resolver change).

    Args:
        transport: the platform's default messaging transport.
        config_source: the platform's default ``-c/--config`` source token.
        logging_format: the platform's default ``logging.<lang>_format`` value, or
            ``None`` to fall through to the library console/text default. Consumed by
            the logging configurator, not the resolver (logging is configured after
            config load); see :func:`profile_logging_format`.
        health_enabled: the platform's default for the HTTP health server (Phase 1c
            health slice). ``True`` on KUBERNETES (the server starts by default with no
            config), ``False`` elsewhere. The middle tier of the FR-RT-3 precedence —
            explicit ``health.enabled`` config ▸ this default ▸ off — applied where the
            health server is started; see :func:`profile_health_enabled`.
        metric_target: the platform's default ``metricEmission.target`` (Phase 1c prometheus
            slice). :data:`METRIC_TARGET_PROMETHEUS` on KUBERNETES (the pull-based registry),
            ``None`` elsewhere (fall through to the library default ``log``). The middle tier of
            the FR-RT-3 precedence — explicit ``metricEmission.target`` config ▸ this default ▸
            ``log`` — applied by ``MetricEmitter`` when selecting the target; see
            :func:`profile_metric_target`.
        credentials_key_provider: the platform's default credentials-vault ``keyProvider.type``
            (Phase 1d env-KeyProvider slice). :data:`CREDENTIALS_KEY_PROVIDER_ENV` (``env``) on
            KUBERNETES — the offline software-KEK from a mounted Secret — ``None`` elsewhere (fall
            through to the library default ``file``). The middle tier of the FR-CRED-6 / FR-RT-3
            precedence — explicit ``credentials.vault.keyProvider.type`` config ▸ this default ▸
            ``file`` — applied at the credentials init site (it does NOT enable credentials, only
            changes the default provider type when a ``credentials`` section is present); see
            :func:`profile_credentials_key_provider`.
    """

    transport: Transport
    config_source: str
    logging_format: Optional[str] = None
    health_enabled: bool = False
    metric_target: Optional[str] = None
    credentials_key_provider: Optional[str] = None


#: The platform-profile table (DESIGN-core sec 3). GREENGRASS and HOST deliberately default the
#: config source to ``GG_CONFIG`` to preserve current behavior. KUBERNETES (Phase 1a) defaults to the
#: ``MQTT`` transport and the k8s-native ``CONFIGMAP`` config source.
#:
#: Phase 1c models the KUBERNETES profile's default ``logging_format`` (``json``, the stdout-JSON
#: sink) and ``metric_target`` (``prometheus``, the pull-based registry). GREENGRASS/HOST keep
#: ``None`` for both (the library console/text default + the ``log`` metric target), so their
#: behavior is unchanged. Phase 1d models the KUBERNETES profile's default ``credentials_key_provider``
#: (``env``, the offline software-KEK from a mounted Secret); GREENGRASS/HOST keep ``None`` (the
#: library default ``file``). TODO (later Phase 1d): the KUBERNETES streaming default (PVC buffer) is
#: not yet modeled here.
PROFILES: Mapping[Platform, PlatformProfile] = {
    Platform.GREENGRASS: PlatformProfile(Transport.IPC, "GG_CONFIG"),
    Platform.HOST: PlatformProfile(Transport.MQTT, "GG_CONFIG"),
    Platform.KUBERNETES: PlatformProfile(
        Transport.MQTT,
        "CONFIGMAP",
        LOGGING_FORMAT_JSON,
        health_enabled=True,
        metric_target=METRIC_TARGET_PROMETHEUS,
        credentials_key_provider=CREDENTIALS_KEY_PROVIDER_ENV,
    ),
}


def profile_logging_format(platform: Optional[Platform]) -> Optional[str]:
    """Return the platform-profile default logging-format token, or ``None`` (FR-RT-3 / FR-LOG-1).

    The logging configurator uses this as the **middle** tier of the logging-format precedence —
    explicit ``logging.<lang>_format`` config ▸ this platform-profile default ▸ library default —
    when the component config does not specify a format. It is a pure lookup (no I/O, no
    ``ConfigManager`` dependency), so the resolved platform alone selects the default; the KUBERNETES
    profile yields :data:`LOGGING_FORMAT_JSON`, every other platform yields ``None``.

    Args:
        platform: the resolved platform, or ``None`` (e.g. a caller that bypassed the resolver).

    Returns:
        The profile's default logging-format token, or ``None`` to keep the library default.
    """
    if platform is None:
        return None
    profile = PROFILES.get(platform)
    return None if profile is None else profile.logging_format


def profile_health_enabled(platform: Optional[Platform]) -> bool:
    """Return the platform-profile default for the HTTP health server (FR-HB-1 / FR-RT-3).

    This is the **middle** tier of the health-enable precedence — explicit ``health.enabled`` config ▸
    this platform-profile default ▸ off — used where the health server is started (see
    ``GGCommons._init_health``). It is a pure lookup (no I/O, no ``ConfigManager`` dependency), so the
    resolved platform alone selects the default: KUBERNETES yields ``True`` (the server starts by
    default with no config), every other platform yields ``False``.

    Args:
        platform: the resolved platform, or ``None`` (e.g. a caller that bypassed the resolver).

    Returns:
        ``True`` if the platform defaults the health server on, else ``False``.
    """
    if platform is None:
        return False
    profile = PROFILES.get(platform)
    return False if profile is None else profile.health_enabled


def profile_metric_target(platform: Optional[Platform]) -> Optional[str]:
    """Return the platform-profile default metric target, or ``None`` (FR-MET-4 / FR-RT-3).

    This is the **middle** tier of the metric-target precedence — explicit ``metricEmission.target``
    config ▸ this platform-profile default ▸ library default ``log`` — consulted by
    :class:`~ggcommons.metrics.metric_emitter.MetricEmitter` when it selects the target. It is a pure
    lookup (no I/O, no ``ConfigManager`` dependency), so the resolved platform alone selects the
    default: KUBERNETES yields :data:`METRIC_TARGET_PROMETHEUS`, every other platform yields ``None``
    (the caller falls through to ``log``).

    Args:
        platform: the resolved platform, or ``None`` (e.g. a caller that bypassed the resolver).

    Returns:
        The profile's default metric target token, or ``None`` to keep the library default.
    """
    if platform is None:
        return None
    profile = PROFILES.get(platform)
    return None if profile is None else profile.metric_target


def profile_credentials_key_provider(platform: Optional[Platform]) -> Optional[str]:
    """Return the platform-profile default credentials key-provider type, or ``None`` (FR-CRED-6 / FR-RT-3).

    This is the **middle** tier of the credentials key-provider precedence — explicit
    ``credentials.vault.keyProvider.type`` config ▸ this platform-profile default ▸ library default
    ``file`` — consulted at the credentials init site (``GGCommons._init_credentials``) when a
    ``credentials`` section is present. It is a pure lookup (no I/O, no ``ConfigManager`` dependency),
    so the resolved platform alone selects the default: KUBERNETES yields
    :data:`CREDENTIALS_KEY_PROVIDER_ENV` (``env``, the offline software-KEK), every other platform
    yields ``None`` (the caller falls through to ``file``).

    Note this does **not** enable credentials — the subsystem stays opt-in, gated by the presence of a
    ``credentials`` config section. It only changes the **default provider type** when credentials is
    configured without an explicit ``keyProvider.type``.

    Args:
        platform: the resolved platform, or ``None`` (e.g. a caller that bypassed the resolver).

    Returns:
        The profile's default key-provider type token, or ``None`` to keep the library default.
    """
    if platform is None:
        return None
    profile = PROFILES.get(platform)
    return None if profile is None else profile.credentials_key_provider


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
    """Resolve the IoT Thing name / identity (DESIGN-core sec 6.2; FR-RT-7 / FR-CFG-6).

    Order of precedence:

    1. explicit ``-t/--thing`` (highest, unchanged);
    2. **if** ``platform == KUBERNETES``: the Downward-API env vars, in order —
       :data:`ENV_K8S_THING_NAME` (``GGCOMMONS_THING_NAME``, the
       ``ggcommons.io/thing-name`` pod annotation mapped by the chart), then
       :data:`ENV_K8S_POD_NAME` (``POD_NAME``, ``metadata.name`` via ``fieldRef``);
    3. ``AWS_IOT_THING_NAME`` (GREENGRASS / generic platform-supplied, unchanged for non-k8s);
    4. the library fallback :data:`DEFAULT_IDENTITY`.

    The KUBERNETES env tier (2) takes precedence over the generic
    ``AWS_IOT_THING_NAME`` probe (3) **only** when ``platform == KUBERNETES``; on
    every other platform behavior is unchanged. The returned value is the raw
    string — template-variable sanitization is still applied downstream when it is
    interpolated as ``{ThingName}`` (see ``ConfigManager.resolve_template``).

    Args:
        thing: the explicit thing name, or ``None``.
        platform: the resolved platform (now consulted for the KUBERNETES tier).
        env: the process environment.

    Returns:
        The resolved identity, never ``None``.
    """
    if thing is not None:
        return thing
    # KUBERNETES Downward-API identity tier (FR-RT-7): GGCOMMONS_THING_NAME, then POD_NAME.
    # Empty values are treated as absent so an unset Downward-API field falls through.
    if platform == Platform.KUBERNETES and env is not None:
        for key in (ENV_K8S_THING_NAME, ENV_K8S_POD_NAME):
            value = env.get(key)
            if value:
                return value
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
