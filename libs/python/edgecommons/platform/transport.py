"""
The messaging *transport* — the secondary runtime axis (DESIGN-core sec 2).

Defaults from the resolved :class:`~edgecommons.platform.platform.Platform`
(GREENGRASS -> IPC, HOST -> MQTT) and is independently overridable, but
constrained: :attr:`IPC` is valid only on
:attr:`~edgecommons.platform.platform.Platform.GREENGRASS` (the Nucleus provides
the IPC socket). See :func:`~edgecommons.platform.resolver.validate`.
"""

from enum import Enum


class Transport(str, Enum):
    """Messaging transport (str-valued so it compares to the raw uppercased CLI token)."""

    #: Greengrass Nucleus IPC (domain socket). Requires platform GREENGRASS.
    IPC = "IPC"
    #: Dual-MQTT (local broker + AWS IoT Core). The off-Nucleus transport.
    MQTT = "MQTT"
