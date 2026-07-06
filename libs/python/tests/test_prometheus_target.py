"""
Unit tests for the pull-based ``prometheus`` metric target (Phase 1c prometheus slice).

Covers:
* name/label sanitization (FR-MET-3), including hostile/invalid characters;
* :meth:`Prometheus.emit_metric_now` and the batched :meth:`emit_metric` both update the in-process
  registry (FR-MET-2 — no push, the scrape pulls);
* a real loopback ``GET`` against ``/metrics`` returns OpenMetrics text with a valid (non-blank)
  ``Content-Type`` containing the emitted gauge with the right name + labels;
* a non-``/metrics`` path returns 404;
* :meth:`Prometheus.close` stops the listener (the port is released);
* :class:`MetricEmitter` target selection by ``metricEmission.target`` and the KUBERNETES
  platform-profile default (prometheus), with explicit config overriding it and HOST/GREENGRASS
  defaulting to ``log``.
"""

import http.client

import pytest

from edgecommons.config.metric_config import MetricConfiguration
from edgecommons.metrics.metric_builder import MetricBuilder
from edgecommons.metrics.metric_emitter import MetricEmitter
from edgecommons.metrics.targets.prometheus import (
    Prometheus,
    _sanitize_label_name,
    _sanitize_metric_name,
)
from edgecommons.platform import Platform


class _FakeConfigManager:
    """Minimal config-manager stand-in for target/emitter unit tests (no I/O, no messaging)."""

    def __init__(self, metric_json=None, platform=None):
        self._metric_config = MetricConfiguration(metric_json)
        self._platform = platform

    def get_metric_config(self):
        return self._metric_config

    def get_platform(self):
        return self._platform

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
    cfg["targetConfig"].setdefault("port", 0)  # ephemeral: avoid clashing with a real 9090
    return Prometheus(_FakeConfigManager(cfg))


def _scrape(target, path=None):
    """Perform a real loopback GET against the target's exposition; return (status, content_type, body)."""
    conn = http.client.HTTPConnection("127.0.0.1", target.port, timeout=5)
    try:
        conn.request("GET", path if path is not None else target.path)
        resp = conn.getresponse()
        body = resp.read().decode("utf-8")
        return resp.status, resp.getheader("Content-Type"), body
    finally:
        conn.close()


# ---------------------------------------------------------------- config parsing

def test_metric_config_parses_prometheus_port_and_path():
    mc = MetricConfiguration(
        {"target": "prometheus", "targetConfig": {"port": 9123, "path": "/m"}}
    )
    assert mc.get_explicit_target() == "prometheus"
    assert mc.get_prometheus_port() == 9123
    assert mc.get_prometheus_path() == "/m"
    assert mc.to_dict()["targetConfig"] == {"port": 9123, "path": "/m"}


def test_metric_config_prometheus_defaults():
    mc = MetricConfiguration({"target": "prometheus"})
    assert mc.get_prometheus_port() == 9090
    assert mc.get_prometheus_path() == "/metrics"


def test_metric_config_explicit_target_none_when_absent():
    # Absent target: explicit is None (so the platform default can apply), effective target is `log`.
    mc = MetricConfiguration({})
    assert mc.get_explicit_target() is None
    assert mc.get_target() == "log"


def test_metric_config_port_path_parsed_even_without_prometheus_target():
    # On KUBERNETES the effective target can be prometheus via the profile default even though the
    # config's `target` is absent (so still "log" here); the port/path must still be picked up.
    mc = MetricConfiguration({"targetConfig": {"port": 7000, "path": "/x"}})
    assert mc.get_explicit_target() is None
    assert mc.get_prometheus_port() == 7000
    assert mc.get_prometheus_path() == "/x"


# ---------------------------------------------------------------- sanitization (FR-MET-3)

def test_sanitize_metric_name_lowercases_and_replaces_invalid():
    assert _sanitize_metric_name("edgecommons_RequestCount") == "edgecommons_requestcount"
    assert _sanitize_metric_name("My App.metric-name") == "my_app_metric_name"


def test_sanitize_metric_name_prefixes_leading_digit():
    assert _sanitize_metric_name("9lives") == "_9lives"


def test_sanitize_label_name_keeps_case_replaces_invalid():
    # Label names keep case; only invalid chars are replaced.
    assert _sanitize_label_name("coreName") == "coreName"
    assert _sanitize_label_name("my-dim.name") == "my_dim_name"


def test_sanitize_label_name_prefixes_leading_digit():
    assert _sanitize_label_name("1bad") == "_1bad"


# ---------------------------------------------------------------- emit + scrape (FR-MET-1/2/3)

def test_emit_updates_registry_and_metrics_endpoint_serves_openmetrics():
    target = _make_target({"namespace": "edgecommons"})
    try:
        metric = (
            MetricBuilder.create("RequestCount")
            .with_namespace("edgecommons")
            .with_thing_name("dev-1")
            .with_component_name("com.example.App")
            .add_measure("count", "Count", 1)
            .add_dimension("region", "us-east-1")
            .build()
        )
        target.emit_metric_now(metric, {"count": 42.0})

        status, content_type, body = _scrape(target)

        assert status == 200
        # Prometheus 3.x rejects a blank content type; the client lib sets a valid one.
        assert content_type is not None and content_type != ""
        assert "text/plain" in content_type
        # gauge name = sanitize(lower("edgecommons_count"))
        assert "edgecommons_count" in body
        # latest value
        assert "42.0" in body or "42" in body
        # dimensions become labels (default dims + the custom one)
        assert 'category="RequestCount"' in body
        assert 'coreName="dev-1"' in body
        assert 'component="com.example.App"' in body
        assert 'region="us-east-1"' in body
    finally:
        target.close()


def test_gauge_name_uses_config_namespace_not_per_metric_namespace():
    """FR-MET-3 parity: the gauge name uses the CONFIGURED metricEmission.namespace, never the
    per-metric namespace — matching canonical Java/Rust/TS. A metric that sets its own namespace
    must NOT change the gauge name (its identity is carried by the `category` label)."""
    target = _make_target({"namespace": "cfgns"})
    try:
        metric = (
            MetricBuilder.create("M")
            .with_namespace("metricns")  # deliberately different from the config namespace
            .add_measure("v", "Count", 1)
            .build()
        )
        target.emit_metric_now(metric, {"v": 7.0})
        status, _ct, body = _scrape(target)
        assert status == 200
        assert "cfgns_v" in body, "gauge must use the CONFIG namespace (cfgns), not metricns"
        assert "metricns_v" not in body, "per-metric namespace must NOT leak into the gauge name"
    finally:
        target.close()


def test_batched_emit_metric_also_updates_registry():
    """The batched ``emit_metric`` path (base default -> emit_metric_now) updates the registry too."""
    target = _make_target({"namespace": "svc"})
    try:
        metric = (
            MetricBuilder.create("Lat")
            .with_namespace("svc")
            .add_measure("latencyMs", "Milliseconds", 1)
            .build()
        )
        target.emit_metric(metric, {"latencyMs": 12.5})
        _, _, body = _scrape(target)
        assert "svc_latencyms" in body
        assert "12.5" in body
    finally:
        target.close()


def test_latest_value_semantics_overwrites_on_reemit():
    target = _make_target({"namespace": "g"})
    try:
        metric = MetricBuilder.create("M").with_namespace("g").add_measure("v", "Count", 1).build()
        target.emit_metric_now(metric, {"v": 1.0})
        target.emit_metric_now(metric, {"v": 7.0})
        _, _, body = _scrape(target)
        # The series exists once with the latest value; the old value is gone.
        assert "7.0" in body or "7" in body
        # 'g_v' gauge line present with value 7
        gauge_lines = [ln for ln in body.splitlines() if ln.startswith("g_v{")]
        assert gauge_lines, body
        assert gauge_lines[0].endswith(" 7.0")
    finally:
        target.close()


def test_hostile_chars_in_namespace_and_dimensions_are_sanitized():
    target = _make_target({"namespace": "My App/Bad"})
    try:
        metric = (
            MetricBuilder.create("X")
            .with_namespace("My App/Bad")
            .add_measure("9weird-measure!", "Count", 1)
            .add_dimension("bad dim.name", "raw value/keep")
            .build()
        )
        target.emit_metric_now(metric, {"9weird-measure!": 3.0})
        _, _, body = _scrape(target)
        # name = sanitize(lower("My App/Bad_9weird-measure!")) -> leading char ok (m), invalid -> _
        assert "my_app_bad_9weird_measure_" in body
        # label name sanitized; value left as-is
        assert "bad_dim_name=" in body
        assert "raw value/keep" in body
    finally:
        target.close()


def test_non_numeric_measure_is_skipped_not_crashing():
    target = _make_target({"namespace": "g"})
    try:
        metric = MetricBuilder.create("M").with_namespace("g").add_measure("v", "Count", 1).build()
        # A non-numeric value must be skipped, not crash the component.
        target.emit_metric_now(metric, {"v": "not-a-number"})
        status, _, body = _scrape(target)
        assert status == 200
        assert "g_v{" not in body  # nothing recorded for the bad data point
    finally:
        target.close()


def test_unknown_path_returns_404():
    target = _make_target()
    try:
        status, _, _ = _scrape(target, "/not-metrics")
        assert status == 404
    finally:
        target.close()


def test_custom_path_is_served():
    target = _make_target({"namespace": "g", "targetConfig": {"path": "/prom"}})
    try:
        assert target.path == "/prom"
        metric = MetricBuilder.create("M").with_namespace("g").add_measure("v", "Count", 1).build()
        target.emit_metric_now(metric, {"v": 1.0})
        status, _, body = _scrape(target, "/prom")
        assert status == 200
        assert "g_v" in body
        # the default /metrics path is now a 404
        status_default, _, _ = _scrape(target, "/metrics")
        assert status_default == 404
    finally:
        target.close()


def test_close_stops_listener_and_releases_port():
    target = _make_target()
    port = target.port
    # Sanity: the listener answers before close.
    status, _, _ = _scrape(target)
    assert status == 200

    target.close()

    # After close, a new connection to the same port must be refused (listener gone, port released).
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
    with pytest.raises((ConnectionRefusedError, OSError)):
        conn.request("GET", "/metrics")
        conn.getresponse()
    conn.close()


def test_close_is_idempotent():
    target = _make_target()
    target.close()
    target.close()  # must not raise


def test_differing_label_sets_for_same_gauge_name_warns_and_skips():
    """Two metrics producing the same gauge name with different dimension keys: the second is skipped
    (prometheus_client requires a fixed label-name set per series name)."""
    target = _make_target({"namespace": "g"})
    try:
        m1 = (
            MetricBuilder.create("A")
            .with_namespace("g")
            .add_measure("v", "Count", 1)
            .add_dimension("extra", "1")
            .build()
        )
        # Same namespace+measure -> same gauge name "g_v", but different custom dims.
        m2 = MetricBuilder.create("B").with_namespace("g").add_measure("v", "Count", 1).build()
        target.emit_metric_now(m1, {"v": 1.0})
        target.emit_metric_now(m2, {"v": 2.0})  # differing labels -> skipped (no crash)
        status, _, body = _scrape(target)
        assert status == 200
        # The first series is present; the second was skipped, so category="B" never recorded.
        assert 'category="A"' in body
        assert 'category="B"' not in body
    finally:
        target.close()


# ---------------------------------------------------------------- MetricEmitter selection (FR-MET-4)

@pytest.fixture
def reset_metric_emitter():
    """Reset the process-global MetricEmitter around a test so target selection starts clean."""
    MetricEmitter.shutdown()
    yield
    MetricEmitter.shutdown()


def test_resolve_target_explicit_config_wins(reset_metric_emitter):
    cfg = _FakeConfigManager({"target": "prometheus"}, platform=Platform.HOST)
    assert MetricEmitter._resolve_target(cfg) == "prometheus"


def test_resolve_target_kubernetes_default_is_prometheus(reset_metric_emitter):
    # No explicit target + KUBERNETES platform -> profile default prometheus.
    cfg = _FakeConfigManager({}, platform=Platform.KUBERNETES)
    assert MetricEmitter._resolve_target(cfg) == "prometheus"


def test_resolve_target_explicit_log_overrides_kubernetes_default(reset_metric_emitter):
    # Explicit log must beat the KUBERNETES profile default.
    cfg = _FakeConfigManager({"target": "log"}, platform=Platform.KUBERNETES)
    assert MetricEmitter._resolve_target(cfg) == "log"


def test_resolve_target_host_defaults_to_log(reset_metric_emitter):
    cfg = _FakeConfigManager({}, platform=Platform.HOST)
    assert MetricEmitter._resolve_target(cfg) == "log"


def test_resolve_target_greengrass_defaults_to_log(reset_metric_emitter):
    cfg = _FakeConfigManager({}, platform=Platform.GREENGRASS)
    assert MetricEmitter._resolve_target(cfg) == "log"


def test_resolve_target_no_platform_defaults_to_log(reset_metric_emitter):
    cfg = _FakeConfigManager({}, platform=None)
    assert MetricEmitter._resolve_target(cfg) == "log"


def test_metric_emitter_init_selects_prometheus_target(reset_metric_emitter):
    # Selected by explicit config; bound to an ephemeral port so the test never clashes on 9090.
    cfg = _FakeConfigManager(
        {"target": "prometheus", "targetConfig": {"port": 0}}, platform=Platform.HOST
    )
    MetricEmitter.init(cfg)
    try:
        assert isinstance(MetricEmitter.metric_target, Prometheus)
    finally:
        MetricEmitter.shutdown()


def test_metric_emitter_init_kubernetes_default_selects_prometheus(reset_metric_emitter):
    # No explicit target; the KUBERNETES profile default must select prometheus.
    cfg = _FakeConfigManager({"targetConfig": {"port": 0}}, platform=Platform.KUBERNETES)
    MetricEmitter.init(cfg)
    try:
        assert isinstance(MetricEmitter.metric_target, Prometheus)
    finally:
        MetricEmitter.shutdown()
