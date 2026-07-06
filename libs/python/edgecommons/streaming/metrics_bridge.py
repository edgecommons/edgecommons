"""
Periodically emits each telemetry stream's stats through the component :class:`MetricEmitter`, so
streaming metrics land in the same configured target (CloudWatch / messaging / log) as heartbeat.
Mirrors the Rust/Java ``StreamMetricsBridge``; one metric per stream, ``stream:<name>``.
"""
from __future__ import annotations

import logging
import threading
from typing import List

from .service import StreamService

logger = logging.getLogger("edgestreamlog")

_DEFAULT_INTERVAL_SECS = 30
_MEASURES = [
    ("backlog", "Count"),
    ("droppedTotal", "Count"),
    ("exportedTotal", "Count"),
    ("retriesTotal", "Count"),
    ("failedTotal", "Count"),
    ("diskBytes", "Bytes"),
    ("oldestUnackedAgeMs", "Milliseconds"),
]


class StreamMetricsBridge:
    """Background poller: stream stats -> MetricEmitter, every ``interval_secs``."""

    def __init__(self, config_manager, streams: StreamService, names: List[str],
                 interval_secs: int = _DEFAULT_INTERVAL_SECS):
        from edgecommons.metrics.metric_builder import MetricBuilder
        from edgecommons.metrics.metric_emitter import MetricEmitter

        self._streams = streams
        self._names = list(names)
        self._interval = interval_secs
        self._stop = threading.Event()
        resolution = 1 if interval_secs < 60 else 60

        for name in self._names:
            builder = MetricBuilder.create(self._metric_name(name)).with_config(config_manager)
            for measure, unit in _MEASURES:
                builder = builder.add_measure(measure, unit, resolution)
            MetricEmitter.define_metric(builder.build())

        self._thread = threading.Thread(target=self._run, name="StreamMetrics", daemon=True)
        self._thread.start()
        logger.info("Stream metrics bridge started for %d stream(s) at %ds interval",
                    len(self._names), interval_secs)

    @staticmethod
    def _metric_name(stream: str) -> str:
        return f"stream:{stream}"

    def _run(self) -> None:
        from edgecommons.metrics.metric_emitter import MetricEmitter

        while not self._stop.wait(self._interval):
            for name in self._names:
                try:
                    s = self._streams.stats(name)
                    MetricEmitter.emit_metric(self._metric_name(name), {
                        "backlog": float(s.backlog),
                        "droppedTotal": float(s.dropped_total),
                        "exportedTotal": float(s.exported_total),
                        "retriesTotal": float(s.retries_total),
                        "failedTotal": float(s.failed_total),
                        "diskBytes": float(s.disk_bytes),
                        "oldestUnackedAgeMs": float(s.oldest_unacked_age_ms),
                    })
                except Exception as exc:  # telemetry-about-telemetry must not crash
                    logger.debug("failed to emit stats for stream %s: %s", name, exc)

    def close(self) -> None:
        self._stop.set()
        self._thread.join(timeout=2)
