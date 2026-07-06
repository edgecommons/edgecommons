"""Unit tests for the Event-driven heartbeat loop (no broker / AWS).

Uses fake config + metric services (heartbeat target type "metric") so the loop's
scheduling, reconfigure, and clean-stop behavior can be exercised deterministically.
Parity with the Java HeartbeatSchedulingTest.
"""
import time

from edgecommons.config.heartbeat_config import HeartbeatConfiguration
from edgecommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat


class _FakeConfig:
    def __init__(self, interval_secs):
        self.interval_secs = interval_secs

    def get_heartbeat_config(self):
        return HeartbeatConfiguration(
            {
                "intervalSecs": self.interval_secs,
                "destination": "local",
                "measures": {"cpu": True, "memory": True},
            }
        )

    def get_thing_name(self):
        return "test-thing"

    def get_component_name(self):
        return "test-component"

    def add_config_change_listener(self, listener):
        pass

    def remove_config_change_listener(self, listener):
        pass


class _FakeMetrics:
    def __init__(self):
        self.calls = []

    def define_metric(self, metric):
        pass

    def emit_metric_now(self, name, measure_values):
        self.calls.append(name)


def test_heartbeat_loop_fires_periodically_and_stops():
    hb = EnhancedHeartbeat(_FakeConfig(interval_secs=1))
    metrics = _FakeMetrics()
    hb.set_metric_service(metrics)
    try:
        time.sleep(3.5)  # ~3 ticks at a 1s interval
        assert len(metrics.calls) >= 2, f"expected periodic ticks, got {len(metrics.calls)}"
        assert hb.is_running()
    finally:
        hb.stop()
    assert not hb.is_running(), "stop() must halt the loop"


def test_heartbeat_reconfigure_restarts_at_new_interval():
    cfg = _FakeConfig(interval_secs=3600)  # effectively no ticks initially
    hb = EnhancedHeartbeat(cfg)
    metrics = _FakeMetrics()
    hb.set_metric_service(metrics)
    try:
        time.sleep(1.0)
        assert len(metrics.calls) == 0  # 3600s interval: nothing yet
        # Reconfigure to a fast interval; on_configuration_change restarts the loop.
        cfg.interval_secs = 1
        hb.on_configuration_change({})
        time.sleep(2.5)
        assert len(metrics.calls) >= 1, "loop should fire after reconfigure to 1s"
    finally:
        hb.stop()
