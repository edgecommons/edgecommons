"""
Pull-based ``prometheus`` metric target (Phase 1c prometheus slice, FR-MET-1/2/3).

Unlike the other (push) targets — ``log``/``messaging``/``cloudwatch``/``cloudwatchcomponent``, which
each *push* an EMF datum somewhere on emit — the ``prometheus`` target maintains an **in-process
registry** of latest-value gauges and *serves* it as OpenMetrics/Prometheus text over HTTP for a
scraper to **pull**. This is the *inverted lifecycle* (FR-MET-2):

* :meth:`emit_metric` and :meth:`emit_metric_now` both only **update the registry** (set the gauge
  for the emitted label-set); they never deliver anywhere. The base-class :meth:`emit_metric`
  default already routes to :meth:`emit_metric_now`, so the "batched" and "immediate" paths are
  identical here — there is no batching, no flush, and no network call on the emit path (so a
  metric emit can never block on the cloud).
* the *flush* concept is a **no-op w.r.t. delivery** — delivery happens when a scraper performs a
  ``GET`` against the ``/metrics`` endpoint, not when the component flushes.
* :meth:`close` **stops the HTTP listener** (no leaked port/thread).

The HTTP exposition binds ``0.0.0.0`` on the configured ``port`` (default 9090) and serves the
configured ``path`` (default ``/metrics``) using the official ``prometheus_client`` exposition
(``generate_latest`` + ``CONTENT_TYPE_LATEST``), which sets a valid, non-blank ``Content-Type``
(``text/plain; version=0.0.4; charset=utf-8``) — Prometheus 3.x rejects a blank type. Any other path
returns ``404``. Built on the standard-library :class:`http.server.ThreadingHTTPServer` running on a
daemon thread (no web framework, matching the health server), so the only added dependency is the
client library itself.

**Dimension -> label mapping (FR-MET-3, LOCKED for four-way parity).** For each measure in an emitted
metric a gauge is registered/updated:

* gauge **name** = :func:`_sanitize_metric_name` of ``lowercase("{namespace}_{measureName}")`` —
  every char not matching ``[a-z0-9_]`` becomes ``_``, and a leading digit is prefixed with ``_``
  (Prometheus metric-name rules); ``namespace`` defaults to ``ggcommons``.
* **labels** = the metric's dimensions (:meth:`Metric.get_dimensions`, which already include
  ``category`` (= the metric name), ``coreName``, ``component`` plus any custom dimensions). Each
  label *name* is sanitized to ``[a-zA-Z_][a-zA-Z0-9_]*`` via :func:`_sanitize_label_name`; the
  label *value* is used as-is.
* the gauge for that label-set is **set** to the measure's float value on each emit (latest-value
  gauge semantics — a scrape reads the current value).

Mirrors the canonical Java ``Prometheus`` target and the Rust/TS equivalents.
"""

import re
from threading import Lock, Thread
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget

# Prometheus metric-name chars: lowercase a-z, digits, underscore (the name is lowercased first).
_METRIC_NAME_INVALID = re.compile(r"[^a-z0-9_]")
# Prometheus label-name chars: letters, digits, underscore (first char must not be a digit).
_LABEL_NAME_INVALID = re.compile(r"[^a-zA-Z0-9_]")


def _sanitize_metric_name(name: str) -> str:
    """Sanitize a string into a valid Prometheus metric name (FR-MET-3).

    Lowercases, replaces every char not matching ``[a-z0-9_]`` with ``_``, and prefixes a leading
    digit with ``_`` so the result matches ``[a-zA-Z_:][a-zA-Z0-9_:]*`` (we never emit ``:``, which
    is reserved for recording rules).
    """
    sanitized = _METRIC_NAME_INVALID.sub("_", (name or "").lower())
    if sanitized and sanitized[0].isdigit():
        sanitized = "_" + sanitized
    return sanitized


def _sanitize_label_name(name: str) -> str:
    """Sanitize a string into a valid Prometheus label name (FR-MET-3).

    Replaces every char not matching ``[a-zA-Z0-9_]`` with ``_`` and prefixes a leading digit with
    ``_`` so the result matches ``[a-zA-Z_][a-zA-Z0-9_]*``.
    """
    sanitized = _LABEL_NAME_INVALID.sub("_", name or "")
    if sanitized and sanitized[0].isdigit():
        sanitized = "_" + sanitized
    return sanitized


class _MetricsRequestHandler(BaseHTTPRequestHandler):
    """Serves the OpenMetrics exposition on the configured path; everything else is 404."""

    server_version = "ggcommons-prometheus"
    sys_version = ""
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802 - name mandated by BaseHTTPRequestHandler
        srv = self.server
        path = self.path.split("?", 1)[0]  # ignore any query string
        if path == srv.metrics_path:
            try:
                body = srv.generate_latest(srv.registry)
            except Exception as e:  # noqa: BLE001 - the scrape must never crash the listener
                srv.logger.warning("prometheus exposition failed: %s", e)
                self._respond(500, b"exposition error", "text/plain; charset=utf-8")
                return
            # CONTENT_TYPE_LATEST is a valid, non-blank content type (Prometheus 3.x rejects blank).
            self._respond(200, body, srv.content_type)
        else:
            self._respond(404, b"not found", "text/plain; charset=utf-8")

    def _respond(self, status: int, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            # The scraper closed the socket early; not worth logging at WARNING.
            pass

    def log_message(self, fmt, *args):  # noqa: A003 - override of BaseHTTPRequestHandler
        self.server.logger.debug("prometheus %s %s", self.address_string(), fmt % args)


class _MetricsHTTPServer(ThreadingHTTPServer):
    """A threaded HTTP server carrying the registry, the exposition writer and the resolved path."""

    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, server_address, registry, metrics_path, content_type, generate_latest, logger):
        super().__init__(server_address, _MetricsRequestHandler)
        self.registry = registry
        self.metrics_path = metrics_path
        self.content_type = content_type
        self.generate_latest = generate_latest
        self.logger = logger


class Prometheus(MetricTarget):
    """Pull-based metric target: an in-process gauge registry served as OpenMetrics text over HTTP.

    See the module docstring for the inverted lifecycle (FR-MET-2) and the dimension->label mapping
    (FR-MET-3). Selected by ``metricEmission.target=prometheus`` and the default on KUBERNETES.
    """

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        # Lazy-import the client so the library (and MetricEmitter's target registry) stay importable
        # even where prometheus-client is not installed; only constructing this target requires it.
        try:
            from prometheus_client import (
                CONTENT_TYPE_LATEST,
                CollectorRegistry,
                Gauge,
                generate_latest,
            )
        except ImportError as e:
            raise RuntimeError(
                "The 'prometheus' metric target requires the 'prometheus-client' package, which is "
                "not installed. Install it (it is in this library's install_requires) or select a "
                "different metricEmission.target."
            ) from e

        self._Gauge = Gauge
        self._generate_latest = generate_latest
        self._content_type = CONTENT_TYPE_LATEST
        # A dedicated registry (not the process-global default) so the exposition contains only this
        # component's gauges and nothing leaks across re-inits / tests.
        self._registry = CollectorRegistry()
        # name -> (Gauge, set(label_names)); guards get-or-create against concurrent emits.
        self._gauges = {}
        self._lock = Lock()

        self._namespace = self.metric_config.get_namespace()
        self._port = self.metric_config.get_prometheus_port()
        path = self.metric_config.get_prometheus_path() or "/metrics"
        self._path = path if path.startswith("/") else "/" + path

        self._httpd = None
        self._thread = None
        self._start_server()

    def _start_server(self) -> None:
        """Bind ``0.0.0.0:port`` and serve the exposition on a daemon thread. Raises if the port is
        unavailable (the caller logs and continues; metrics still update the registry)."""
        self._httpd = _MetricsHTTPServer(
            ("0.0.0.0", self._port),
            self._registry,
            self._path,
            self._content_type,
            self._generate_latest,
            self.logger,
        )
        self._thread = Thread(
            target=self._httpd.serve_forever, name="ggcommons-prometheus", daemon=True
        )
        self._thread.start()
        self.logger.info(
            "Prometheus metric target listening on 0.0.0.0:%d%s", self.port, self._path
        )

    @property
    def port(self) -> int:
        """The actual bound port (resolves an ephemeral ``port: 0`` once started; used by tests)."""
        if self._httpd is not None:
            return self._httpd.server_address[1]
        return self._port

    @property
    def path(self) -> str:
        """The resolved exposition path (always leading-slash normalized)."""
        return self._path

    def _get_or_create_gauge(self, name: str, label_names):
        """Return the cached gauge for ``name``, creating it (with these label names) on first use.

        Returns ``None`` (and warns) if the gauge already exists with a *different* label-name set or
        if registration fails — a single bad data point must never crash the component.
        """
        with self._lock:
            entry = self._gauges.get(name)
            if entry is not None:
                gauge, existing_labels = entry
                if existing_labels != set(label_names):
                    self.logger.warning(
                        "prometheus gauge '%s' already registered with labels %s; skipping emit "
                        "with differing labels %s",
                        name,
                        sorted(existing_labels),
                        sorted(label_names),
                    )
                    return None
                return gauge
            try:
                gauge = self._Gauge(
                    name,
                    f"ggcommons metric {name}",
                    labelnames=label_names,
                    registry=self._registry,
                )
            except Exception as e:  # noqa: BLE001 - duplicate/invalid label names etc.
                self.logger.warning("failed to register prometheus gauge '%s': %s", name, e)
                return None
            self._gauges[name] = (gauge, set(label_names))
            return gauge

    def emit_metric_now(self, metric, measure_values):
        """Update the in-process registry for ``metric`` (FR-MET-2: no push, the scrape pulls).

        For each measure, register/update a latest-value gauge named
        ``sanitize(lower("{namespace}_{measure}"))`` labeled by the metric's (sanitized-name)
        dimensions, set to the measure's float value (FR-MET-3).

        The namespace is the CONFIGURED ``metricEmission.namespace`` (captured at construction),
        NOT the per-metric namespace — matching the canonical Java (``metricConfig.getNamespace()``)
        and Rust/TS (config namespace). Using the per-metric namespace here would make gauge names
        diverge across languages when a metric sets its own namespace; the metric's identity is
        already carried by the ``category`` dimension label.
        """
        namespace = self._namespace

        # Build the (ordered) sanitized label names + values once for this metric's dimension set.
        label_names = []
        label_values = {}
        for dim_name, dim_value in metric.get_dimensions().items():
            sanitized = _sanitize_label_name(dim_name)
            label_names.append(sanitized)
            label_values[sanitized] = "" if dim_value is None else str(dim_value)
        label_names = tuple(label_names)

        for measure_name, value in measure_values.items():
            try:
                float_value = float(value)
            except (TypeError, ValueError):
                self.logger.warning(
                    "prometheus: measure '%s' value %r is not numeric; skipping", measure_name, value
                )
                continue
            gauge_name = _sanitize_metric_name(f"{namespace}_{measure_name}")
            gauge = self._get_or_create_gauge(gauge_name, label_names)
            if gauge is None:
                continue
            try:
                if label_names:
                    gauge.labels(**label_values).set(float_value)
                else:
                    gauge.set(float_value)
            except Exception as e:  # noqa: BLE001 - never let one data point crash the component
                self.logger.warning("prometheus: failed to set gauge '%s': %s", gauge_name, e)

        self.logger.debug("Metric '%s' recorded to prometheus registry", metric.get_name())

    def on_configuration_change(self, configuration) -> bool:
        # Port/path/namespace are fixed at open (rebinding the listener / renaming gauges on a hot
        # reload would orphan in-flight scrapes and the existing series). Changes apply on restart.
        self.logger.info(
            "Prometheus metric target: configuration change ignored (port/path/namespace fixed at open)"
        )
        return True

    def close(self) -> None:
        """Stop the HTTP listener and release the port/thread (FR-MET-2). Idempotent and bounded."""
        if self._httpd is not None:
            try:
                self._httpd.shutdown()
                self._httpd.server_close()
            except Exception as e:  # noqa: BLE001 - shutdown must not raise
                self.logger.warning("Error stopping prometheus listener: %s", e)
            finally:
                self._httpd = None
        if self._thread is not None:
            self._thread.join(timeout=5.0)
            self._thread = None
