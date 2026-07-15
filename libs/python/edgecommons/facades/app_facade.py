"""The ``app()`` publish facade -- free-form inter-component pub/sub on the ``app`` class
(DESIGN-class-facades §2.3, D3). ``app`` is the intentionally-open class, so the
facade's value is **not** body enforcement (there is no contract to enforce) -- it is
removing the raw three-line ritual and guaranteeing topic + identity correctness: a
**named** header, the developer body **verbatim**, minted onto ``app/{channel}`` with
the envelope identity stamped. ``app`` is non-reserved -- this publishes through the
ordinary guarded ``MessagingClient.publish(...)``.

Routing: :attr:`~edgecommons.facades.channel.Channel.LOCAL` (default) or
:attr:`~edgecommons.facades.channel.Channel.NORTHBOUND`; a ``stream`` route is
**rejected** (same reasoning as ``events()``).

Library-internal: obtain via ``gg.instance(id).app()`` or the component-scope
convenience ``gg.app()`` (no instance token, D-U28).

Mirrors Java's ``AppFacade`` (``com.mbreissi.edgecommons.facades.AppFacade``).
"""
import logging
from dataclasses import dataclass
from typing import Any, Dict, Optional, Union

from edgecommons.facades.channel import Channel
from edgecommons.facades.util import sanitize_channel_path
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.message import Message
from edgecommons.messaging.qos import Qos
from edgecommons.uns import UnsClass

logger = logging.getLogger("AppFacade")


@dataclass(frozen=True, init=False)
class PreparedAppMessage:
    """One prepared application publication with stable envelope bytes.

    ``encoded_bytes`` is captured once together with the UUID and timestamp.  A
    durable outbox can therefore retry identical bytes instead of reconstructing a
    logically equivalent but differently identified envelope.
    """

    topic: str
    encoded_bytes: bytes

    def __init__(self, topic: str, message: Message, encoded_bytes: bytes):
        if not topic:
            raise ValueError("topic must not be empty")
        if message is None:
            raise ValueError("message must not be None")
        if not isinstance(encoded_bytes, bytes):
            raise TypeError("encoded_bytes must be bytes")
        if encoded_bytes != message.to_bytes():
            raise ValueError("encoded_bytes must be the exact serialization of message")
        object.__setattr__(self, "topic", topic)
        object.__setattr__(self, "encoded_bytes", bytes(encoded_bytes))

    @property
    def message(self) -> Message:
        """Returns a fresh parsed view so callers cannot mutate the prepared envelope."""
        return Message.from_bytes(self.encoded_bytes)


class AppFacade:
    """The ``app()`` publish facade bound to one instance token -- see the module
    docstring."""

    #: The app envelope header version (the header ``name`` is the caller's chosen name).
    APP_MESSAGE_VERSION = "1.0"

    def __init__(self, config_manager, instance_id: str, uns, messaging_client):
        """Library-internal constructor (see the module docstring).

        :param config_manager: the component config manager (envelope identity)
        :param instance_id: the instance token this facade is bound to
        :param uns: the instance-bound :class:`~edgecommons.uns.Uns` topic builder
        :param messaging_client: the (guarded) messaging handle
        """
        if config_manager is None:
            raise ValueError("config_manager must not be None")
        # D-U28: instance_id is None for component scope (no instance token).
        if uns is None:
            raise ValueError("uns must not be None")
        if messaging_client is None:
            raise ValueError("messaging_client must not be None")
        self._config_manager = config_manager
        self._instance_id = instance_id
        self._uns = uns
        self._messaging = messaging_client

    def publish(self, name: str, channel: str, body: Dict[str, Any],
                 routing: Optional[Channel] = None) -> None:
        """Publishes a free-form message on ``app/{channel}``.

        :param name: the envelope header ``name`` (the developer's message name; REQUIRED)
        :param channel: the ``app/{channel}`` tail (each ``/``-token is sanitized; REQUIRED)
        :param body: the developer body, published verbatim
        :param routing: the routing channel, or ``None`` for LOCAL
        :raises ValueError: when ``name``/``channel`` is missing/empty, or ``routing`` is
            a ``stream`` channel
        """
        self.publish_prepared(self.prepare(name, channel, body), routing)

    def prepare(
        self, name: str, channel: str, body: Dict[str, Any]
    ) -> PreparedAppMessage:
        """Constructs an application envelope without publishing it."""
        return self._prepare_internal(name, channel, body, None)

    def prepare_correlated(
        self,
        name: str,
        channel: str,
        body: Dict[str, Any],
        request_or_correlation_id: Union[Message, str],
    ) -> PreparedAppMessage:
        """Prepares an app message carrying an existing conversation correlation."""
        if isinstance(request_or_correlation_id, Message):
            header = request_or_correlation_id.get_header()
            correlation_id = None if header is None else header.correlation_id
        elif isinstance(request_or_correlation_id, str):
            correlation_id = request_or_correlation_id
        else:
            raise ValueError(
                "correlated app message requires a request or correlation id"
            )
        if not correlation_id:
            raise ValueError(
                "correlated app message requires a non-empty correlation id"
            )
        return self._prepare_internal(name, channel, body, correlation_id)

    def _prepare_internal(
        self,
        name: str,
        channel: str,
        body: Dict[str, Any],
        correlation_id: Optional[str],
    ) -> PreparedAppMessage:
        if not name:
            raise ValueError("app publish requires a non-empty header name")
        if not channel:
            raise ValueError("app publish requires a non-empty channel")
        topic = self._uns.topic(UnsClass.APP, sanitize_channel_path(channel))
        builder = (
            MessageBuilder.create(name, self.APP_MESSAGE_VERSION)
            .with_config(self._config_manager)
            .with_instance(self._instance_id)
            .with_payload(body)
        )
        if correlation_id is not None:
            builder.with_correlation_id(correlation_id)
        message = builder.build()
        return PreparedAppMessage(topic, message, message.to_bytes())

    def publish_prepared(
        self,
        prepared: PreparedAppMessage,
        routing: Optional[Channel] = None,
    ) -> None:
        """Publishes a prepared envelope through the existing immediate API."""
        if not isinstance(prepared, PreparedAppMessage):
            raise ValueError("prepared must be a PreparedAppMessage")
        self._reject_stream(routing)
        if routing is not None and routing.kind is Channel.Kind.NORTHBOUND:
            try:
                self._messaging.publish_northbound(
                    prepared.topic, prepared.message, Qos.AT_LEAST_ONCE
                )
            except Exception as e:  # noqa: BLE001 - a northbound outage must not propagate
                logger.warning("Northbound app publish on '%s' failed (local readiness"
                               " unaffected): %s", prepared.topic, e)
        else:
            self._messaging.publish(prepared.topic, prepared.message)

    def publish_confirmed(
        self,
        prepared: PreparedAppMessage,
        timeout_secs: float,
        routing: Optional[Channel] = None,
    ) -> None:
        """Publishes exact prepared bytes and waits for positive acknowledgement.

        Unlike ordinary northbound publication, failures intentionally propagate so
        an outbox can leave its record pending and retry the same UUID.
        """
        if not isinstance(prepared, PreparedAppMessage):
            raise ValueError("prepared must be a PreparedAppMessage")
        self._reject_stream(routing)
        if routing is not None and routing.kind is Channel.Kind.NORTHBOUND:
            self._messaging.publish_northbound_confirmed(
                prepared.topic,
                prepared.encoded_bytes,
                Qos.AT_LEAST_ONCE,
                timeout_secs,
            )
        else:
            self._messaging.publish_confirmed(
                prepared.topic,
                prepared.encoded_bytes,
                Qos.AT_LEAST_ONCE,
                timeout_secs,
            )

    @staticmethod
    def _reject_stream(channel: Optional[Channel]) -> None:
        if channel is not None and channel.kind is Channel.Kind.STREAM:
            raise ValueError(
                "app() does not support the stream channel - use data() for streamed"
                " telemetry"
            )
