"""Credentials -> metrics bridge.

Periodically surfaces non-sensitive credential-subsystem :class:`CredentialStats` through the
component :class:`MetricEmitter` (CloudWatch / messaging / log), mirroring the Rust ``bridge.rs``
and the Python ``StreamMetricsBridge``. **Never emits secret values.**
"""
import logging
import threading

logger = logging.getLogger("edgecommons.credentials.bridge")

_DEFAULT_INTERVAL_SECS = 30
_METRIC = "credentials"
_MEASURES = [
    ("secretCount", "Count"),
    ("lastSyncAgeMs", "Milliseconds"),
    ("syncFailures", "Count"),
    ("rotations", "Count"),
]


class CredentialMetricsBridge:
    """Background poller: credential stats -> MetricEmitter, every ``interval_secs``. Never emits
    secret values."""

    def __init__(self, creds, interval_secs: int = _DEFAULT_INTERVAL_SECS):
        from edgecommons.metrics.metric_builder import MetricBuilder
        from edgecommons.metrics.metric_emitter import MetricEmitter

        self._creds = creds
        self._interval = interval_secs
        self._stop = threading.Event()
        resolution = 1 if interval_secs < 60 else 60

        builder = MetricBuilder.create(_METRIC)
        for measure, unit in _MEASURES:
            builder = builder.add_measure(measure, unit, resolution)
        MetricEmitter.define_metric(builder.build())

        self._thread = threading.Thread(target=self._run, name="CredentialMetrics", daemon=True)
        self._thread.start()
        logger.info("Credential metrics bridge started at %ds interval", interval_secs)

    def _run(self) -> None:
        from edgecommons.metrics.metric_emitter import MetricEmitter

        while not self._stop.wait(self._interval):
            try:
                s = self._creds.stats()
                MetricEmitter.emit_metric(_METRIC, {
                    "secretCount": float(s.secret_count),
                    "lastSyncAgeMs": float(s.last_sync_age_ms if s.last_sync_age_ms is not None else 0),
                    "syncFailures": float(s.sync_failures),
                    "rotations": float(s.rotations),
                })
            except Exception as exc:  # telemetry-about-credentials must not crash
                logger.debug("failed to emit credential stats: %s", exc)

    def close(self) -> None:
        self._stop.set()
        self._thread.join(timeout=2)
