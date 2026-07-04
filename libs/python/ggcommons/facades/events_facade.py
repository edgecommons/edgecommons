"""The ``events()`` publish facade -- operator events & alarms on the ``evt`` class
(DESIGN-class-facades §2.2, D8). It is the facade that **stops the §1.2 ``evt`` drift**:
it makes the ``evt/{severity}/{type}`` channel and the body shape non-negotiable by
**deriving the channel from the body's own ``severity`` + ``type``**, so the topic and
body can never disagree. ``evt`` is non-reserved -- this publishes through the ordinary
guarded ``MessagingClient.publish(...)``.

Body (``header.name`` = :data:`EventsFacade.EVT_MESSAGE_NAME`, version
:data:`EventsFacade.EVT_MESSAGE_VERSION`)::

    {"severity":  "critical|warning|info|debug",  # REQUIRED (channel token 1)
     "type":      <REQUIRED>,                      # the event type (channel token 2, sanitized)
     "message":   <str>?,                           # optional operator text
     "timestamp": <iso>,                            # DEFAULTED to now
     "context":   {}?,                              # optional structured data
     "alarm":     <bool>?,  "active": <bool>?}      # present only for raise_alarm/clear_alarm

Channel: ``evt/{severity.wire()}/{sanitize(type)}`` (2 tokens). Routing:
:attr:`~ggcommons.facades.channel.Channel.LOCAL` (default) or
:attr:`~ggcommons.facades.channel.Channel.NORTHBOUND` via :meth:`EventsFacade.via` --
alarms often go straight to the cloud control plane. A ``stream`` route is **rejected**
(events are low-rate control-plane, not bulk telemetry).

Python-idiom note: Java overloads ``emit``/``raiseAlarm``/``clearAlarm`` on arity;
Python instead collapses each into one method with keyword defaults (``severity``
defaults to ``INFO`` on :meth:`emit`, ``CRITICAL`` on :meth:`raise_alarm`/
:meth:`clear_alarm` when omitted) -- same behavior, no overload set.

Library-internal: obtain via ``gg.instance(id).events()`` or the ``main`` convenience
``gg.events()``.

Mirrors Java's ``EventsFacade`` (``com.mbreissi.ggcommons.facades.EventsFacade``).
"""
import logging
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Optional

from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.facades.channel import Channel
from ggcommons.facades.severity import Severity
from ggcommons.facades.util import format_instant
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.uns import UnsClass

logger = logging.getLogger("EventsFacade")


class EventsFacade:
    """The ``events()`` publish facade bound to one instance token -- see the module
    docstring."""

    #: The event envelope header name.
    EVT_MESSAGE_NAME = "evt"
    #: The event envelope header version.
    EVT_MESSAGE_VERSION = "1.0"

    def __init__(self, config_manager, instance_id: str, uns, messaging_client,
                 clock: Optional[Callable[[], datetime]] = None,
                 _override: Optional[Channel] = None):
        """Library-internal constructor (see the module docstring).

        :param config_manager: the component config manager (envelope identity)
        :param instance_id: the instance token this facade is bound to
        :param uns: the instance-bound :class:`~ggcommons.uns.Uns` topic builder
        :param messaging_client: the (guarded) messaging handle
        :param clock: a zero-arg callable returning the current timezone-aware
            ``datetime`` for the ``timestamp`` default (injected for tests)
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
        self._clock = clock if clock is not None else (lambda: datetime.now(timezone.utc))
        self._override = _override  # nullable per-view channel override

    def via(self, channel: Channel) -> "EventsFacade":
        """Returns a channel-bound view for a per-call routing override (LOCAL or
        NORTHBOUND).

        :raises ValueError: when ``channel`` is a ``stream`` channel
        """
        self._reject_stream(channel)
        return EventsFacade(self._config_manager, self._instance_id, self._uns,
                            self._messaging, self._clock, _override=channel)

    # ===================== emit =====================

    def emit(self, event_type: str, message: Optional[str] = None,
             context: Optional[Dict[str, Any]] = None,
             severity: Optional[Severity] = None) -> None:
        """Emits a one-shot event. ``severity`` defaults to :attr:`Severity.INFO` when
        omitted (the message-only convenience DESIGN-class-facades §2.2 describes).

        :param event_type: the event type (channel token 2; REQUIRED)
        :param message: optional operator text
        :param context: optional structured data
        :param severity: the severity (channel token 1); defaults to INFO
        """
        sev = severity if severity is not None else Severity.INFO
        self._publish(sev, event_type, message, context, None, None)

    # ===================== alarms =====================

    def raise_alarm(self, event_type: str, message: Optional[str] = None,
                    context: Optional[Dict[str, Any]] = None,
                    severity: Optional[Severity] = None) -> None:
        """Raises a stateful alarm (``alarm=true, active=true``). ``severity`` defaults
        to :attr:`Severity.CRITICAL` so raises and clears of the same alarm ride the same
        ``evt/critical/{type}`` channel (subsumes OPC UA's ``connection-lost``).

        :param event_type: the alarm type (channel token 2)
        :param message: optional operator text
        :param context: optional structured data
        :param severity: an explicit severity override; defaults to CRITICAL
        """
        sev = severity if severity is not None else Severity.CRITICAL
        self._publish(sev, event_type, message, context, True, True)

    def clear_alarm(self, event_type: str, context: Optional[Dict[str, Any]] = None,
                    severity: Optional[Severity] = None) -> None:
        """Clears a stateful alarm (``alarm=true, active=false``). ``severity`` defaults
        to :attr:`Severity.CRITICAL` so the clear tracks on the same channel as the raise
        (subsumes OPC UA's ``connection-restored``).

        :param event_type: the alarm type (must match the raise's type)
        :param context: optional structured data
        :param severity: an explicit severity override; defaults to CRITICAL
        """
        sev = severity if severity is not None else Severity.CRITICAL
        self._publish(sev, event_type, None, context, True, False)

    # ===================== body construction + routing =====================

    def build_body(self, severity: Severity, event_type: str, message: Optional[str] = None,
                   context: Optional[Dict[str, Any]] = None,
                   alarm: Optional[bool] = None, active: Optional[bool] = None) -> Dict[str, Any]:
        """Constructs the ``evt`` wire body -- the exact body the vectors pin.
        Deterministic given the injected clock.

        :raises ValueError: when ``event_type`` is missing/empty
        """
        if not event_type:
            raise ValueError(
                "evt requires a non-empty type (it is a channel token and the event's kind)"
            )
        body: Dict[str, Any] = {"severity": severity.wire(), "type": event_type}
        if message is not None:
            body["message"] = message
        body["timestamp"] = format_instant(self._clock())
        if context is not None:
            body["context"] = context
        if alarm is not None:
            body["alarm"] = alarm
            body["active"] = active
        return body

    def channel_for(self, severity: Severity, event_type: str) -> str:
        """The ``evt/{severity}/{type}`` channel derived from the body's own severity +
        type.

        :raises ValueError: when ``event_type`` is missing/empty
        """
        if not event_type:
            raise ValueError("evt requires a non-empty type")
        return f"{severity.wire()}/{ConfigManager.sanitize(event_type)}"

    def _publish(self, severity: Severity, event_type: str, message: Optional[str],
                context: Optional[Dict[str, Any]], alarm: Optional[bool],
                active: Optional[bool]) -> None:
        body = self.build_body(severity, event_type, message, context, alarm, active)
        channel = self.channel_for(severity, event_type)
        topic = self._uns.topic(UnsClass.EVT, channel)
        msg = (
            MessageBuilder.create(self.EVT_MESSAGE_NAME, self.EVT_MESSAGE_VERSION)
            .with_config(self._config_manager)
            .with_instance(self._instance_id)
            .with_payload(body)
            .build()
        )
        self._route(topic, msg)

    def _route(self, topic: str, msg) -> None:
        """LOCAL (default) or NORTHBOUND; a stream override is rejected up front by
        :meth:`via`."""
        channel = self._override if self._override is not None else Channel.LOCAL
        if channel.kind is Channel.Kind.NORTHBOUND:
            try:
                from awsiot.greengrasscoreipc.model import QOS
                self._messaging.publish_to_iot_core(topic, msg, QOS.AT_LEAST_ONCE)
            except Exception as e:  # noqa: BLE001 - a northbound outage must not propagate
                logger.warning("Northbound evt publish on '%s' failed (local readiness"
                               " unaffected): %s", topic, e)
        else:
            self._messaging.publish(topic, msg)

    @staticmethod
    def _reject_stream(channel: Optional[Channel]) -> None:
        if channel is not None and channel.kind is Channel.Kind.STREAM:
            raise ValueError(
                "events() does not support the stream channel - events are low-rate"
                " control-plane, not bulk telemetry (use data() for streamed telemetry)"
            )
