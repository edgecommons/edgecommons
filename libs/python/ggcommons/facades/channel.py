"""A publish-channel address (DESIGN-class-facades §4, ``DESIGN-channels.md``): the
uniform ``{ local, northbound, stream:<name> }`` routing target the publish facades
resolve on.

- :attr:`Channel.LOCAL` -- the local/IPC bus (``MessagingClient.publish``). The default.
- :attr:`Channel.NORTHBOUND` -- AWS IoT Core (``MessagingClient.publish_to_iot_core``).
- :meth:`Channel.stream` -- the named durable telemetry stream
  (``get_streams().stream(name).append(...)``); **only**
  :class:`~ggcommons.facades.data_facade.DataFacade` honors it -- ``events()``/``app()``
  reject a stream channel (they are low-rate control-plane, not bulk telemetry).

Modeled as a small tagged-union value class (mirroring Java's ``Channel``) rather than a
bare enum because the ``stream`` variant carries a stream name. :meth:`Channel.from_config`
parses the config ``publish.channel`` string (Option C, DESIGN-class-facades §4):
``"local"``, ``"northbound"``/``"iotcore"``/``"iot_core"``, or ``"stream:<name>"``.
"""
from enum import Enum
from typing import Optional


class Channel:
    """A routing channel: ``LOCAL`` / ``NORTHBOUND`` / a named ``stream:<name>``."""

    class Kind(Enum):
        """The routing kind."""

        LOCAL = "local"
        NORTHBOUND = "northbound"
        STREAM = "stream"

    def __init__(self, kind: "Channel.Kind", stream_name: Optional[str] = None):
        self._kind = kind
        self._stream_name = stream_name

    @property
    def kind(self) -> "Channel.Kind":
        """The routing kind."""
        return self._kind

    @property
    def stream_name(self) -> Optional[str]:
        """The stream name (non-``None`` only for a :attr:`Kind.STREAM` channel)."""
        return self._stream_name

    @staticmethod
    def stream(name: str) -> "Channel":
        """The named-durable-stream channel.

        :param name: the stream name (must match a configured ``streaming.streams[].name``)
        :raises ValueError: if ``name`` is ``None``/empty
        """
        if not name:
            raise ValueError("stream channel name must be non-empty")
        return Channel(Channel.Kind.STREAM, name)

    @staticmethod
    def from_config(value: Optional[str]) -> Optional["Channel"]:
        """Parses a config ``publish.channel`` string into a channel (DESIGN-class-facades
        §4, Option C). Recognized: ``"local"`` -> :attr:`LOCAL`; ``"northbound"`` /
        ``"iotcore"`` / ``"iot_core"`` -> :attr:`NORTHBOUND`; ``"stream:<name>"`` ->
        :meth:`stream`. Any other (or ``None``/empty) value yields ``None`` so the caller
        can fall through to its own default.
        """
        if not value:
            return None
        v = value.strip()
        if not v:
            return None
        lower = v.lower()
        if lower == "local":
            return Channel.LOCAL
        if lower in ("northbound", "iotcore", "iot_core"):
            return Channel.NORTHBOUND
        if lower.startswith("stream:"):
            name = v[len("stream:"):]
            return Channel.stream(name) if name else None
        return None

    def __eq__(self, other) -> bool:
        return (
            isinstance(other, Channel)
            and self._kind == other._kind
            and self._stream_name == other._stream_name
        )

    def __hash__(self) -> int:
        return hash((self._kind, self._stream_name))

    def __repr__(self) -> str:
        return f"Channel({self})"

    def __str__(self) -> str:
        """``"local"`` / ``"northbound"`` / ``"stream:<name>"`` -- the config-string form."""
        if self._kind is Channel.Kind.LOCAL:
            return "local"
        if self._kind is Channel.Kind.NORTHBOUND:
            return "northbound"
        return f"stream:{self._stream_name}"


#: The local/IPC bus channel (the default).
Channel.LOCAL = Channel(Channel.Kind.LOCAL)
#: The AWS IoT Core (northbound) channel.
Channel.NORTHBOUND = Channel(Channel.Kind.NORTHBOUND)
