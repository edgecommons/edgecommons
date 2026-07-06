"""The seam :class:`~edgecommons.facades.data_facade.DataFacade` composes to route a
``stream:<name>`` channel into the telemetry streaming service (DESIGN-class-facades §4:
"the facade *composes* ``StreamService``, it does not replace it"). Production wires it
to ``get_streams().stream(name).append(partition_key, timestamp_ms, payload)``; a plain
callable, so tests inject a recorder and the facade never depends on the streaming
service directly.

When streaming is not configured (``get_streams() is None``), :class:`~edgecommons.EdgeCommons`
passes ``None`` for the sink and the facade falls the stream route back to a LOCAL
publish (readiness / no-streaming -> local) rather than dropping the record.

Mirrors Java's ``StreamSink`` functional interface
(``com.mbreissi.edgecommons.facades.StreamSink``).
"""
from typing import Callable

#: ``(stream_name, partition_key, timestamp_ms, payload) -> None`` -- appends one durable
#: record to a named stream. ``payload`` is the serialized envelope bytes (the exact
#: bytes a bus publish would carry); ``partition_key`` is the routing/ordering key (the
#: signal's stable ``signal.id``); ``timestamp_ms`` is the producer timestamp (epoch
#: millis, from the sample's ``serverTs``).
StreamSink = Callable[[str, str, int, bytes], None]
