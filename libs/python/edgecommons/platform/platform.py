"""
The deployment *platform* — the primary runtime axis (DESIGN-core sec 2/3).

A platform is a named profile: a table of per-subsystem default
providers/targets/sinks selected by the :class:`PlatformResolver`. It is
orthogonal to :class:`~edgecommons.platform.transport.Transport`; only
messaging-transport is platform-coupled (via the IPC lock,
:func:`~edgecommons.platform.resolver.validate`).

Phase 0 populated :attr:`GREENGRASS` and :attr:`HOST` (a behavior-preserving
re-expression of today's two modes). Phase 1a wires :attr:`KUBERNETES` (transport
``MQTT``, config source ``CONFIGMAP``); a service-account-token pod auto-detects
to it and it resolves cleanly.
"""

from enum import Enum


class Platform(str, Enum):
    """Deployment platform (str-valued so it compares to the raw uppercased CLI token)."""

    #: On an AWS IoT Greengrass v2 Nucleus: IPC transport, Nucleus-managed config/identity.
    GREENGRASS = "GREENGRASS"
    #: A plain host (Docker/bare host without a Nucleus): MQTT transport.
    HOST = "HOST"
    #: Kubernetes (Phase 1a): MQTT transport, ConfigMap-mounted config/identity.
    KUBERNETES = "KUBERNETES"
