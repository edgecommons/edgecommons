"""The ``data()`` publish facade -- the telemetry / signal data plane
(DESIGN-class-facades §2.1, D2/D5). It **constructs and validates the
``SouthboundSignalUpdate`` body** (``device``/``signal``/``samples``) so an adapter never
hand-builds it, applies the body defaults, sanitizes the signal path into the UNS
``data`` channel, stamps the envelope identity, and routes to the resolved
:class:`~ggcommons.facades.channel.Channel`. It publishes through the **ordinary,
guarded** ``MessagingClient.publish(...)`` -- ``data`` is non-reserved, so it passes the
guard; the facade adds body-contract enforcement + defaults, **not** privilege.

Body (``header.name`` = :data:`DataFacade.DATA_MESSAGE_NAME`, version
:data:`DataFacade.DATA_MESSAGE_VERSION`)::

    {"device":  {"adapter": <str>, "instance": <str>, "endpoint": <str>}?,  # optional
     "signal":  {"id": <REQUIRED>, "name"?, "address"?},
     "samples": [{"value": <REQUIRED>, "quality", "qualityRaw"?, "sourceTs"?, "serverTs"}]}

Defaulting (DESIGN-class-facades §2.1, pinned by ``uns-test-vectors/data.json``):

1. ``quality`` -> ``GOOD`` when omitted on a sample that carries a value.
2. ``qualityRaw`` -> the synthetic marker ``"unspecified"`` when (and only when) the
   quality was defaulted; else the caller's value verbatim, else absent.
3. ``serverTs`` -> now (ISO-8601 UTC ``...Z``, from the injected clock) when omitted;
   ``sourceTs`` is **never** synthesized (absent when the source has none).
4. The ``samples`` wrapper is enforced for the value-shorthand (a caller never emits a
   bare value).
5. ``signal.id`` is the **only** hard reject -- a publish with no stable id raises
   :class:`ValueError` at the call site.

Channel routing (DESIGN-class-facades §4, D1): per-call
:meth:`~ggcommons.facades.signal_update.SignalUpdateBuilder.via` override -> config
``publish.channel`` (instance -> global) -> :attr:`Channel.LOCAL`. A ``stream:<name>``
route serializes the same envelope and appends it to
``get_streams().stream(name)`` (partition key = ``signal.id``, ts = ``serverTs``); when
streaming is not configured it falls back to a LOCAL publish (readiness / no-streaming ->
local). Northbound / stream transport failures are caught and logged -- they must never
flip local readiness.

Library-internal: obtain the bound instance via ``gg.instance(id).data()`` (or the
``main``-instance convenience ``gg.data()``).

Mirrors Java's ``DataFacade`` (``com.mbreissi.ggcommons.facades.DataFacade``).
"""
import json
import logging
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Optional

from ggcommons.facades.channel import Channel
from ggcommons.facades.quality import Quality
from ggcommons.facades.signal_update import Sample, SignalUpdate, SignalUpdateBuilder
from ggcommons.facades.stream_sink import StreamSink
from ggcommons.facades.util import (
    format_instant,
    parse_iso_to_epoch_millis,
    sanitize_channel_path,
)
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.uns import UnsClass

logger = logging.getLogger("DataFacade")


class DataFacade:
    """The ``data()`` publish facade bound to one instance token -- see the module
    docstring."""

    #: The signal-update envelope header name (``docs/SOUTHBOUND.md`` §2).
    DATA_MESSAGE_NAME = "SouthboundSignalUpdate"
    #: The signal-update envelope header version.
    DATA_MESSAGE_VERSION = "1.0"
    #: The ``qualityRaw`` marker written when ``quality`` was defaulted to ``GOOD``.
    QUALITY_UNSPECIFIED = "unspecified"

    def __init__(self, config_manager, instance_id: str, uns, messaging_client,
                 stream_sink: Optional[StreamSink] = None,
                 clock: Optional[Callable[[], datetime]] = None):
        """Library-internal constructor (see the module docstring).

        :param config_manager: the component config manager (envelope identity +
            ``publish.channel``)
        :param instance_id: the instance token this facade is bound to
        :param uns: the instance-bound :class:`~ggcommons.uns.Uns` topic builder
        :param messaging_client: the (guarded) messaging handle (the ``MessagingClient``
            class, or a test double exposing ``publish``/``publish_to_iot_core``)
        :param stream_sink: the stream seam, or ``None`` when streaming is not configured
        :param clock: a zero-arg callable returning the current timezone-aware
            ``datetime`` for ``serverTs`` defaults (injected for deterministic tests);
            defaults to ``datetime.now(timezone.utc)``
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
        self._stream_sink = stream_sink
        self._clock = clock if clock is not None else (lambda: datetime.now(timezone.utc))
        self._warned_no_stream = False

    def instance_id(self) -> str:
        """The instance token this facade is bound to."""
        return self._instance_id

    # ===================== fluent builder entry point =====================

    def signal(self, signal_id: Optional[str]) -> SignalUpdateBuilder:
        """Starts building a ``SouthboundSignalUpdate`` for a stable ``signal.id`` -- the
        fluent body builder that subsumes the hand-assembled dict. Terminate with
        :meth:`~ggcommons.facades.signal_update.SignalUpdateBuilder.publish`.

        :param signal_id: the stable ``signal.id`` (REQUIRED -- the consumer key)
        """
        return SignalUpdateBuilder(signal_id, facade=self)

    # ===================== value shorthand =====================

    def publish(self, signal_path: str, value: Any,
                quality: Optional[Quality] = None) -> None:
        """The value-shorthand: publish one value for a signal path (the path doubles as
        the stable ``signal.id``). The single value is wrapped into a one-element
        ``samples`` list with ``quality=GOOD`` (unless ``quality`` is given),
        ``qualityRaw="unspecified"`` when defaulted, ``serverTs=now`` -- a caller never
        emits a bare value.

        :param signal_path: the signal path / stable id (e.g. ``"press12/temperature"``)
        :param value: the measured value (REQUIRED)
        :param quality: an explicit quality (so a source that knows the read is
            stale/failed marks it ``BAD``/``UNCERTAIN``), or ``None`` to default to GOOD
        """
        self.signal(signal_path).add_sample(value, quality).signal_path(signal_path).publish()

    # ===================== the raw escape hatch =====================

    def publish_body(self, signal_path: str, body: Dict[str, Any],
                     via: Optional[Channel] = None) -> None:
        """The raw escape hatch (D5): publishes a caller-owned pre-built body verbatim to
        ``data/{signal_path}``, applying **no** body defaulting -- only the topic +
        identity guarantees. For a component with an exotic body the facade should not
        shape.

        :param signal_path: the signal path (sanitized into the channel)
        :param body: the pre-built body, published untouched
        :param via: the channel override, or ``None`` to resolve config -> LOCAL
        """
        if body is None:
            raise ValueError("body must not be None")
        channel = self._channel_token(signal_path)
        topic = self._uns.topic(UnsClass.DATA, channel)
        msg = self._message(body)
        self._route(via, topic, msg, signal_path, self._first_server_ts_millis(body))

    # ===================== the SignalUpdate publish path =====================

    def publish_update(self, update: SignalUpdate) -> None:
        """Publishes a built :class:`~ggcommons.facades.signal_update.SignalUpdate`:
        validates ``signal.id``, constructs the body with the defaulting rules, sanitizes
        the path into the ``data`` channel, stamps the envelope, and routes to the
        resolved channel.

        :raises ValueError: when ``signal.id`` is missing/empty, there are no samples, or
            a sample carries no value
        """
        if not update.signal_id:
            raise ValueError(
                "data publish requires a stable signal.id (the consumer key) - it is the"
                " only non-defaultable field"
            )
        if not update.samples:
            raise ValueError("data publish requires at least one sample")
        body = self.build_body(update)
        channel = self._channel_token(update.effective_signal_path)
        topic = self._uns.topic(UnsClass.DATA, channel)
        msg = self._message(body)
        self._route(update.via, topic, msg, update.signal_id,
                    self._first_server_ts_millis(body))

    # ===================== body construction (THE contract) =====================

    def build_body(self, update: SignalUpdate) -> Dict[str, Any]:
        """Constructs the wire body from a
        :class:`~ggcommons.facades.signal_update.SignalUpdate`, applying the §2.1
        defaulting rules (quality -> ``GOOD`` + ``qualityRaw`` marker, ``serverTs`` -> now,
        the ``samples`` wrapper). Deterministic given the injected clock -- this is the
        exact body the vectors pin.

        :raises ValueError: when a sample carries no value
        """
        signal: Dict[str, Any] = {"id": update.signal_id}
        if update.signal_name is not None:
            signal["name"] = update.signal_name
        if update.signal_address is not None:
            signal["address"] = update.signal_address

        samples = [self._build_sample(sample) for sample in update.samples]

        body: Dict[str, Any] = {}
        if update.device is not None:
            body["device"] = update.device
        body["signal"] = signal
        body["samples"] = samples
        return body

    def _build_sample(self, sample: Sample) -> Dict[str, Any]:
        """Builds one sample with the quality/qualityRaw/serverTs defaulting rules."""
        if sample.value is None:
            raise ValueError(
                "data sample value is required (a quality-only sample is not a sample) -"
                " pass BAD/UNCERTAIN for a failed read"
            )
        out: Dict[str, Any] = {"value": sample.value}

        quality_defaulted = sample.quality is None
        quality = Quality.GOOD if quality_defaulted else sample.quality
        out["quality"] = quality.wire()

        quality_raw = sample.quality_raw
        if quality_raw is None and quality_defaulted:
            quality_raw = self.QUALITY_UNSPECIFIED
        if quality_raw is not None:
            out["qualityRaw"] = quality_raw

        if sample.source_ts is not None:
            out["sourceTs"] = sample.source_ts
        out["serverTs"] = sample.server_ts if sample.server_ts is not None else self._now_iso()
        return out

    # ===================== channel routing =====================

    def resolve_channel(self, via: Optional[Channel]) -> Channel:
        """Resolves the effective channel: per-call ``via`` override -> config
        ``publish.channel`` (instance -> global) -> :attr:`Channel.LOCAL`
        (DESIGN-class-facades §4, D1).
        """
        if via is not None:
            return via
        configured = self._configured_channel()
        return configured if configured is not None else Channel.LOCAL

    def _configured_channel(self) -> Optional[Channel]:
        """Reads the config ``publish.channel`` default (Option C): the bound instance's
        ``publish.channel`` -> the global ``component.global.publish.channel``.
        Best-effort -- any lookup/parse anomaly yields ``None`` (fall through to LOCAL)."""
        try:
            instance_cfg = self._config_manager.get_instance_config(self._instance_id)
        except Exception as e:  # noqa: BLE001 - best-effort, matches the Java facade
            logger.debug("publish.channel lookup (instance) failed (defaulting to LOCAL"
                        " on the instance tier): %s", e)
            instance_cfg = None
        from_instance = _publish_channel_of(instance_cfg)
        if from_instance is not None:
            return from_instance
        try:
            global_cfg = self._config_manager.get_global_config()
        except Exception as e:  # noqa: BLE001
            logger.debug("publish.channel lookup (global) failed (defaulting to LOCAL): %s", e)
            return None
        return _publish_channel_of(global_cfg)

    def _route(self, via: Optional[Channel], topic: str, msg, partition_key: str,
              ts_millis: int) -> None:
        """Routes a built envelope to the resolved channel. LOCAL publishes on the
        guarded bus; NORTHBOUND publishes to IoT Core; a stream route appends the
        serialized envelope to the named stream (falling back to LOCAL when no sink is
        wired). Northbound / stream failures are caught + logged (they must never flip
        local readiness)."""
        channel = self.resolve_channel(via)
        if channel.kind is Channel.Kind.LOCAL:
            self._messaging.publish(topic, msg)
        elif channel.kind is Channel.Kind.NORTHBOUND:
            try:
                from awsiot.greengrasscoreipc.model import QOS
                self._messaging.publish_to_iot_core(topic, msg, QOS.AT_LEAST_ONCE)
            except Exception as e:  # noqa: BLE001 - a northbound outage must not propagate
                logger.warning("Northbound data publish on '%s' failed (local readiness"
                               " unaffected): %s", topic, e)
        else:  # STREAM
            self._append_to_stream(channel.stream_name, topic, msg, partition_key, ts_millis)

    def _append_to_stream(self, stream_name: str, topic: str, msg, partition_key: str,
                         ts_millis: int) -> None:
        """The ``stream:<name>`` route: append the serialized envelope, or fall back to a
        LOCAL publish on the same topic the stream route would have used."""
        if self._stream_sink is None:
            if not self._warned_no_stream:
                self._warned_no_stream = True
                logger.warning(
                    "data channel 'stream:%s' requested but streaming is not configured -"
                    " routing to LOCAL instead (readiness/no-streaming -> local)",
                    stream_name,
                )
            self._messaging.publish(topic, msg)
            return
        try:
            payload = json.dumps(msg.to_dict()).encode("utf-8")
            self._stream_sink(stream_name, partition_key, ts_millis, payload)
        except Exception as e:  # noqa: BLE001 - a stream-append outage must not propagate
            logger.warning("Stream append to 'stream:%s' failed (local readiness"
                           " unaffected): %s", stream_name, e)

    # ===================== helpers =====================

    def _channel_token(self, signal_path: Optional[str]) -> str:
        """The sanitized channel token for a signal path (each ``/``-token -> a UNS
        token)."""
        if not signal_path:
            raise ValueError("data signal path must be non-empty")
        return sanitize_channel_path(signal_path)

    def _message(self, body: Dict[str, Any]):
        """Builds the identity-stamped envelope with the signal-update header."""
        return (
            MessageBuilder.create(self.DATA_MESSAGE_NAME, self.DATA_MESSAGE_VERSION)
            .with_config(self._config_manager)
            .with_instance(self._instance_id)
            .with_payload(body)
            .build()
        )

    def _now_iso(self) -> str:
        """ISO-8601 UTC (``...Z``) "now" from the injected clock."""
        return format_instant(self._clock())

    def _first_server_ts_millis(self, body: Dict[str, Any]) -> int:
        """The first sample's ``serverTs`` as epoch millis (the stream record
        timestamp)."""
        try:
            samples = body.get("samples")
            if samples:
                first = samples[0]
                if "serverTs" in first:
                    return parse_iso_to_epoch_millis(first["serverTs"])
        except Exception:  # noqa: BLE001 - fall through to now() below
            pass
        return int(datetime.now(timezone.utc).timestamp() * 1000)


def _publish_channel_of(section: Optional[Dict[str, Any]]) -> Optional[Channel]:
    """``section["publish"]["channel"]`` as a :class:`Channel`, or ``None`` when
    absent/unparseable."""
    if not isinstance(section, dict):
        return None
    publish = section.get("publish")
    if not isinstance(publish, dict):
        return None
    value = publish.get("channel")
    if not isinstance(value, str):
        return None
    return Channel.from_config(value)
