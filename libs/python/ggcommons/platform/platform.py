"""
The deployment *platform* — the primary runtime axis (DESIGN-core sec 2/3).

A platform is a named profile: a table of per-subsystem default
providers/targets/sinks selected by the :class:`PlatformResolver`. It is
orthogonal to :class:`~ggcommons.platform.transport.Transport`; only
messaging-transport is platform-coupled (via the IPC lock,
:func:`~ggcommons.platform.resolver.validate`).

Phase 0 populates only :attr:`GREENGRASS` and :attr:`HOST` (a
behavior-preserving re-expression of today's two modes). :attr:`KUBERNETES` is
declared but *not* wired — selecting it fails fast until its profile ships in
Phase 1.
"""

from enum import Enum


class Platform(str, Enum):
    """Deployment platform (str-valued so it compares to the raw uppercased CLI token)."""

    #: On an AWS IoT Greengrass v2 Nucleus: IPC transport, Nucleus-managed config/identity.
    GREENGRASS = "GREENGRASS"
    #: A plain host (Docker/bare host without a Nucleus): MQTT transport.
    HOST = "HOST"
    #: Kubernetes (declared for Phase 0; profile populated in Phase 1).
    KUBERNETES = "KUBERNETES"
