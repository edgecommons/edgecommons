"""
Supplementary coverage tests for the pull-based ``prometheus`` metric target.

These exercise the defensive / less-common branches of
``edgecommons/metrics/targets/prometheus.py`` that the main suite
(``tests/test_prometheus_target.py``) does not reach, so the module hits the 90%
line gate under ``-m "not slow and not integration and not aws"``:

* the ``/metrics`` handler returning **500** when the exposition writer raises
  (``do_GET`` except branch);
* ``_respond`` swallowing a ``BrokenPipeError`` from a scraper that hung up early;
* the constructor raising ``RuntimeError`` when ``prometheus-client`` is missing;
* the ``port`` property after :meth:`Prometheus.close` (``_httpd is None`` branch);
* ``_get_or_create_gauge`` warning + returning ``None`` when gauge registration raises;
* :meth:`emit_metric_now` no-label ``gauge.set`` branch (a metric with no dimensions)
  and the except branch when setting a gauge value raises;
* :meth:`on_configuration_change` (ignored, returns ``True``);
* :meth:`close` swallowing an error from the listener shutdown.

Everything is deterministic and cross-platform: any HTTP server is bound to an
ephemeral port (``port: 0``) on ``127.0.0.1`` and always closed; the rest drives
the methods directly with fakes. No broker, no AWS, no real port pinning.
"""

import http.client
import sys

import pytest

from edgecommons.config.metric_config import MetricConfiguration
from edgecommons.metrics.metric_builder import MetricBuilder
from edgecommons.metrics.targets.prometheus import Prometheus, _MetricsRequestHandler


class _FakeConfigManager:
    """Minimal config-manager stand-in (no I/O, no messaging) for target unit tests."""

    def __init__(self, metric_json=None):
        self._metric_config = MetricConfiguration(metric_json)

    def get_metric_config(self):
        return self._metric_config

    def get_thing_name(self):
        return "test-thing"

    def get_component_name(self):
        return "test-component"

    def resolve_template(self, value):
        return value

    def add_config_change_listener(self, listener):
        pass


def _make_target(metric_json=None):
    """Build a Prometheus target bound to an ephemeral port (port 0) for loopback tests."""
    cfg = dict(metric_json or {})
    cfg.setdefault("target", "prometheus")
    cfg.setdefault("targetConfig", {})
    cfg["targetConfig"].setdefault("port", 0)  # ephemeral: never clash with a real 9090
    return Prometheus(_FakeConfigManager(cfg))


def _scrape(target, path=None):
    """Real loopback GET against the exposition; returns (status, content_type, body)."""
    conn = http.client.HTTPConnection("127.0.0.1", target.port, timeout=5)
    try:
        conn.request("GET", path if path is not None else target.path)
        resp = conn.getresponse()
        body = resp.read().decode("utf-8")
        return resp.status, resp.getheader("Content-Type"), body
    finally:
        conn.close()


class _FakeMetric:
    """A metric with a controllable dimension set (lets us hit the no-label gauge path)."""

    def __init__(self, name="M", dimensions=None):
        self._name = name
        self._dimensions = dict(dimensions or {})

    def get_dimensions(self):
        return self._dimensions

    def get_name(self):
        return self._name


# ------------------------------------------------------------ do_GET 500 (lines 93-96)

class TestExpositionFailure:
    def test_metrics_endpoint_returns_500_when_generate_latest_raises(self):
        target = _make_target({"namespace": "g"})
        try:
            def _boom(registry):
                raise RuntimeError("exposition boom")

            # The handler reads srv.generate_latest at request time; swap it for a raiser.
            target._httpd.generate_latest = _boom

            status, _ct, body = _scrape(target)
            assert status == 500
            assert "exposition error" in body
        finally:
            target.close()


# ------------------------------------------------------ _respond broken pipe (lines 110-112)

class TestRespondBrokenPipe:
    def test_respond_swallows_broken_pipe_from_writer(self):
        # Drive _respond directly with a writer that fails mid-write (scraper hung up early).
        handler = object.__new__(_MetricsRequestHandler)
        handler.send_response = lambda status: None
        handler.send_header = lambda key, value: None
        handler.end_headers = lambda: None

        class _BadWriter:
            def write(self, data):
                raise BrokenPipeError("client closed the socket")

        handler.wfile = _BadWriter()
        # Must not raise despite the writer failing (the except/pass swallows it).
        handler._respond(200, b"body", "text/plain; charset=utf-8")


# --------------------------------------------------- missing client lib (lines 151-152)

class TestMissingClientLibrary:
    def test_constructor_raises_runtimeerror_when_prometheus_client_missing(self, monkeypatch):
        # Setting the module to None in sys.modules makes `import prometheus_client` raise
        # ModuleNotFoundError (an ImportError subclass) -> the constructor remaps it to RuntimeError.
        monkeypatch.setitem(sys.modules, "prometheus_client", None)
        with pytest.raises(RuntimeError, match="prometheus-client"):
            Prometheus(_FakeConfigManager({"target": "prometheus"}))


# --------------------------------------------------- port property after close (line 201)

class TestPortPropertyAfterClose:
    def test_port_returns_configured_port_when_listener_stopped(self):
        target = _make_target({"targetConfig": {"port": 0}})
        target.close()
        # _httpd is now None -> the property falls back to the configured port.
        assert target._httpd is None
        assert target.port == target._port == 0


# --------------------------------------------- gauge registration failure (lines 235-237)

class TestGaugeRegistrationFailure:
    def test_gauge_creation_failure_warns_and_skips(self):
        target = _make_target({"namespace": "g"})
        try:
            def _raise(*args, **kwargs):
                raise ValueError("invalid gauge definition")

            target._Gauge = _raise  # force registration to fail

            metric = (
                MetricBuilder.create("M")
                .with_namespace("g")
                .add_measure("v", "Count", 1)
                .build()
            )
            # Must not crash; the bad gauge is skipped (return None -> continue).
            target.emit_metric_now(metric, {"v": 1.0})

            status, _ct, body = _scrape(target)
            assert status == 200
            assert "g_v{" not in body  # nothing was registered
        finally:
            target.close()


# --------------------------------- no-label set + set failure (lines 281, 282-283)

class TestSetGaugeBranches:
    def test_emit_no_dimension_metric_uses_unlabeled_set(self):
        # A metric with an empty dimension set drives the `else: gauge.set(value)` branch.
        target = _make_target({"namespace": "g"})
        try:
            target.emit_metric_now(_FakeMetric("M", dimensions={}), {"v": 5.0})
            status, _ct, body = _scrape(target)
            assert status == 200
            # Unlabeled series: `g_v 5.0` (no `{...}` labels).
            assert "g_v 5.0" in body
        finally:
            target.close()

    def test_set_failure_is_swallowed(self):
        target = _make_target({"namespace": "g"})
        try:
            class _BadGauge:
                def labels(self, **kwargs):
                    return self

                def set(self, value):
                    raise RuntimeError("set blew up")

            target._get_or_create_gauge = lambda name, label_names: _BadGauge()

            metric = (
                MetricBuilder.create("M")
                .with_namespace("g")
                .add_measure("v", "Count", 1)
                .build()
            )
            # The labeled `gauge.labels(...).set(...)` raises; emit must not crash.
            target.emit_metric_now(metric, {"v": 1.0})
        finally:
            target.close()


# --------------------------------------------- on_configuration_change (lines 290-293)

class TestOnConfigurationChange:
    def test_configuration_change_is_ignored_and_returns_true(self):
        target = _make_target()
        try:
            assert target.on_configuration_change({"port": 9999}) is True
        finally:
            target.close()


# ------------------------------------------------ close swallows shutdown error (301-302)

class TestCloseShutdownError:
    def test_close_swallows_listener_shutdown_error(self):
        target = _make_target()
        # Cleanly stop the real listener first (releases the port, joins the thread).
        target.close()

        class _BadHttpd:
            def shutdown(self):
                raise RuntimeError("shutdown blew up")

            def server_close(self):  # pragma: no cover - not reached (shutdown raises first)
                pass

        # Re-attach a faulty listener and a no-op thread; close() must swallow the error.
        target._httpd = _BadHttpd()
        target._thread = None
        target.close()
        assert target._httpd is None
