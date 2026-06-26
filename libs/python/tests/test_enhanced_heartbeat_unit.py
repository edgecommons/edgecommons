"""Deterministic unit tests for EnhancedHeartbeat publish/lifecycle paths.

Drives _publish_heartbeat / _publish_to_messaging / _publish_to_metrics directly with
mock services (long interval so the loop never ticks during a test), so no broker or
real timing is needed. Complements the timing-based test_heartbeat_loop.py.
"""
from unittest.mock import MagicMock

import pytest

from ggcommons.config.heartbeat_config import HeartbeatConfiguration
from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat


class FakeConfig:
    def __init__(self, heartbeat_json=None):
        self._hb_json = heartbeat_json if heartbeat_json is not None else {
            "intervalSecs": 3600,  # effectively no ticks during the test
            "targets": [{"type": "messaging", "config": {"topic": "hb/{ThingName}", "destination": "ipc"}}],
            "measures": {"cpu": True, "memory": True},
        }
        self.listeners = []

    def get_heartbeat_config(self):
        return HeartbeatConfiguration(self._hb_json)

    def get_metric_config(self):
        return MetricConfiguration()

    def get_thing_name(self):
        return "thing-1"

    def get_component_name(self):
        return "comp"

    def resolve_template(self, t):
        return t.replace("{ThingName}", "thing-1").replace("{ComponentName}", "comp")

    def get_tag_config(self):
        return None

    def add_config_change_listener(self, listener):
        self.listeners.append(listener)

    def remove_config_change_listener(self, listener):
        if listener in self.listeners:
            self.listeners.remove(listener)


@pytest.fixture
def hb():
    h = EnhancedHeartbeat(FakeConfig())
    h.stop()  # kill the loop thread; we call methods directly
    yield h
    h.stop()


class TestConstruction:
    def test_none_config_raises(self):
        with pytest.raises(ValueError):
            EnhancedHeartbeat(None)

    def test_registers_listener_and_runs(self):
        cfg = FakeConfig()
        h = EnhancedHeartbeat(cfg)
        try:
            assert h in cfg.listeners
            assert h.is_running() is True
        finally:
            h.stop()
        assert h.is_running() is False

    def test_get_last_heartbeat_time_none(self, hb):
        assert hb.get_last_heartbeat_time() is None


class TestServiceInjection:
    def test_set_messaging_service(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        assert hb._messaging_service is msg

    def test_set_metric_service_defines_metric(self, hb):
        metric = MagicMock()
        hb.set_metric_service(metric)
        assert hb._metric_service is metric
        # injecting the metric service triggers a (re)definition of the heartbeat metric
        metric.define_metric.assert_called_once()


class TestPublishMessaging:
    def test_no_service_warns_and_returns(self, hb):
        # no messaging service injected -> no exception, nothing published
        hb._publish_to_messaging({"cpu": {"cpu_usage": 1.0}}, {"config": {"destination": "ipc"}})

    def test_ipc_destination_publishes_local(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_to_messaging(
            {"cpu": {"cpu_usage": 1.0}},
            {"config": {"topic": "hb/{ThingName}", "destination": "ipc"}},
        )
        msg.publish.assert_called_once()
        assert msg.publish.call_args[0][0] == "hb/thing-1"

    def test_iot_core_destination_publishes_iot(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_to_messaging(
            {"cpu": {"cpu_usage": 1.0}},
            {"config": {"topic": "hb", "destination": "iot_core"}},
        )
        msg.publish_to_iot_core.assert_called_once()

    def test_unrecognized_destination_skips(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_to_messaging({"cpu": {"cpu_usage": 1.0}}, {"config": {"destination": "weird"}})
        msg.publish.assert_not_called()
        msg.publish_to_iot_core.assert_not_called()


class TestPublishMetrics:
    def test_no_metric_service_returns(self, hb):
        hb._publish_to_metrics({"cpu": {"cpu_usage": 1.0}})  # no exception

    def test_flattens_and_emits(self, hb):
        metric = MagicMock()
        hb.set_metric_service(metric)
        metric.reset_mock()
        hb._publish_to_metrics({"cpu": {"cpu_usage": 1.5}, "memory": {"memory_usage": 10.0}})
        metric.emit_metric_now.assert_called_once()
        name, values = metric.emit_metric_now.call_args[0]
        assert name == "heartbeat"
        assert values == {"cpu_usage": 1.5, "memory_usage": 10.0}

    def test_non_numeric_values_skipped(self, hb):
        metric = MagicMock()
        hb.set_metric_service(metric)
        metric.reset_mock()
        # non-numeric measure is skipped; non-dict category ignored
        hb._publish_to_metrics({"cpu": {"cpu_usage": "NaNstr"}, "scalar": 5})
        metric.emit_metric_now.assert_not_called()


class TestPublishHeartbeat:
    def test_monitor_none_warns(self, hb):
        hb._heartbeat_monitor = None
        hb._publish_heartbeat()  # no exception

    def test_dispatches_to_messaging_and_metric(self):
        cfg = FakeConfig({
            "intervalSecs": 3600,
            "targets": [{"type": "messaging", "config": {"destination": "ipc"}}, {"type": "metric"}],
            "measures": {"cpu": True, "memory": True},
        })
        h = EnhancedHeartbeat(cfg)
        h.stop()
        try:
            msg, metric = MagicMock(), MagicMock()
            h.set_messaging_service(msg)
            h.set_metric_service(metric)
            metric.reset_mock()
            h._publish_heartbeat()
            assert msg.publish.called
            assert metric.emit_metric_now.called
        finally:
            h.stop()

    def test_unknown_target_type_warns(self):
        cfg = FakeConfig({
            "intervalSecs": 3600,
            "targets": [{"type": "carrier-pigeon"}],
            "measures": {"cpu": True},
        })
        h = EnhancedHeartbeat(cfg)
        h.stop()
        try:
            h._publish_heartbeat()  # logs unknown target warning, no exception
        finally:
            h.stop()


class TestLifecycle:
    def test_start_when_already_running_is_noop(self):
        cfg = FakeConfig()
        h = EnhancedHeartbeat(cfg)
        try:
            assert h.is_running()
            h.start()  # already running -> no-op
            assert h.is_running()
        finally:
            h.stop()

    def test_start_after_stop(self):
        cfg = FakeConfig()
        h = EnhancedHeartbeat(cfg)
        h.stop()
        assert not h.is_running()
        h.start()
        assert h.is_running()
        h.stop()

    def test_on_configuration_change_restarts(self, hb):
        assert hb.on_configuration_change({}) is True

    def test_get_heartbeat_config_exception_returns_none(self, hb):
        boom = MagicMock()
        boom.get_heartbeat_config.side_effect = RuntimeError("x")
        hb._config_service = boom
        assert hb._get_heartbeat_config() is None
