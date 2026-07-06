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

Library-internal: obtain via ``gg.instance(id).app()`` or the ``main`` convenience
``gg.app()``.

Mirrors Java's ``AppFacade`` (``com.mbreissi.edgecommons.facades.AppFacade``).
"""
import logging
from typing import Any, Dict, Optional

from edgecommons.facades.channel import Channel
from edgecommons.facades.util import sanitize_channel_path
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.qos import Qos
from edgecommons.uns import UnsClass

logger = logging.getLogger("AppFacade")


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
        if not instance_id:
            raise ValueError("instance_id must not be None/empty")
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
        if not name:
            raise ValueError("app publish requires a non-empty header name")
        if not channel:
            raise ValueError("app publish requires a non-empty channel")
        self._reject_stream(routing)
        topic = self._uns.topic(UnsClass.APP, sanitize_channel_path(channel))
        msg = (
            MessageBuilder.create(name, self.APP_MESSAGE_VERSION)
            .with_config(self._config_manager)
            .with_instance(self._instance_id)
            .with_payload(body)
            .build()
        )
        if routing is not None and routing.kind is Channel.Kind.NORTHBOUND:
            try:
                self._messaging.publish_northbound(topic, msg, Qos.AT_LEAST_ONCE)
            except Exception as e:  # noqa: BLE001 - a northbound outage must not propagate
                logger.warning("Northbound app publish on '%s' failed (local readiness"
                               " unaffected): %s", topic, e)
        else:
            self._messaging.publish(topic, msg)

    @staticmethod
    def _reject_stream(channel: Optional[Channel]) -> None:
        if channel is not None and channel.kind is Channel.Kind.STREAM:
            raise ValueError(
                "app() does not support the stream channel - use data() for streamed"
                " telemetry"
            )
