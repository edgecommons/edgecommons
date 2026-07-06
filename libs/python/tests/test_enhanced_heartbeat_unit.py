"""Deterministic unit tests for EnhancedHeartbeat publish/lifecycle paths.

Drives _publish_heartbeat / _publish_state / _emit_sys_metric directly with mock
services (long interval so the loop never ticks during a test), so no broker or real
timing is needed. Complements the timing-based test_heartbeat_loop.py.

The heartbeat is the UNS ``state`` keepalive (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20):
``ecv1/{device}/{component}/main/state`` with body
``{"status":"RUNNING","uptimeSecs":n}`` through the privileged ``_publish_reserved*``
seam, plus the measures as the ``sys`` metric through the metric subsystem; a
best-effort ``{"status":"STOPPED"}`` is published once on stop().
"""
from unittest.mock import MagicMock

import pytest

from edgecommons.config.heartbeat_config import HeartbeatConfiguration
from edgecommons.config.metric_config import MetricConfiguration
from edgecommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat
from edgecommons.messaging.identity import HierEntry, MessageIdentity


class FakeConfig:
    def __init__(self, heartbeat_json=None, identity=True):
        self._hb_json = heartbeat_json if heartbeat_json is not None else {
            "intervalSecs": 3600,  # effectively no ticks during the test
            "measures": {"cpu": True, "memory": True},
        }
        self._identity = (
            MessageIdentity([HierEntry("device", "thing-1")], "comp") if identity else None
        )
        self.listeners = []

    def get_heartbeat_config(self):
        return HeartbeatConfiguration(self._hb_json)

    def get_metric_config(self):
        return MetricConfiguration()

    def get_component_identity(self):
        return self._identity

    def is_topic_include_root(self):
        return False

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
    h._stopped_published = False  # re-arm; tests drive publish paths directly
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

    def test_disabled_by_configuration_does_not_start(self):
        cfg = FakeConfig({"enabled": False, "intervalSecs": 3600})
        h = EnhancedHeartbeat(cfg)
        try:
            assert h.is_running() is False
        finally:
            h.stop()

    def test_get_last_heartbeat_time_none(self, hb):
        assert hb.get_last_heartbeat_time() is None


class TestServiceInjection:
    def test_set_messaging_service(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        assert hb._messaging_service is msg

    def test_set_metric_service_defines_sys_metric(self, hb):
        metric = MagicMock()
        hb.set_metric_service(metric)
        assert hb._metric_service is metric
        # injecting the metric service triggers a (re)definition of the `sys` metric
        metric.define_metric.assert_called_once()
        defined = metric.define_metric.call_args[0][0]
        assert defined.get_name() == "sys"


class TestPublishState:
    def test_no_service_warns_and_returns(self, hb):
        # no messaging service injected -> no exception, nothing published
        hb._publish_state("RUNNING", include_uptime=True)

    def test_no_identity_warns_once_and_skips(self):
        h = EnhancedHeartbeat(FakeConfig(identity=False))
        h.stop()
        try:
            msg = MagicMock()
            h.set_messaging_service(msg)
            h._publish_state("RUNNING", include_uptime=True)
            h._publish_state("RUNNING", include_uptime=True)
            msg._publish_reserved.assert_not_called()
            msg._publish_reserved_northbound.assert_not_called()
        finally:
            h.stop()

    def test_running_state_publishes_uns_topic_with_uptime(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_state("RUNNING", include_uptime=True)
        msg._publish_reserved.assert_called_once()
        topic, message = msg._publish_reserved.call_args[0]
        assert topic == "ecv1/thing-1/comp/main/state"
        assert message.get_header().name == "state"
        assert message.get_header().version == "1.0"
        body = message.get_body()
        assert body["status"] == "RUNNING"
        assert isinstance(body["uptimeSecs"], int)
        # the config-bound builder stamps the component identity (instance main)
        assert message.get_identity().component == "comp"
        assert message.get_identity().instance == "main"

    def test_stopped_state_has_no_uptime(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_state("STOPPED", include_uptime=False)
        _, message = msg._publish_reserved.call_args[0]
        assert message.get_body() == {"status": "STOPPED"}


class TestInstanceConnectivity:
    """The #1c per-instance connectivity surface: a registered provider's result is emitted
    in the RUNNING state body's ``instances`` array, best-effort (a None/empty/raising
    provider omits the section but never suppresses the keepalive)."""

    def _body(self, msg):
        return msg._publish_reserved.call_args[0][1].get_body()

    def test_provider_result_in_state_instances(self, hb):
        from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb.set_instance_connectivity_provider(lambda: [
            InstanceConnectivity.of("filler1", True, "opc.tcp://kep:49320"),
            InstanceConnectivity.of("kep2", False),
        ])
        hb._publish_state("RUNNING", include_uptime=True)
        body = self._body(msg)
        assert body["status"] == "RUNNING"
        assert body["instances"] == [
            {"instance": "filler1", "connected": True, "detail": "opc.tcp://kep:49320"},
            {"instance": "kep2", "connected": False},
        ]

    def test_no_provider_omits_instances(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_state("RUNNING", include_uptime=True)
        assert "instances" not in self._body(msg)

    def test_empty_none_and_cleared_omit_instances(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        for provider in (lambda: [], lambda: None):
            hb.set_instance_connectivity_provider(provider)
            hb._publish_state("RUNNING", include_uptime=True)
            assert "instances" not in self._body(msg)
        hb.set_instance_connectivity_provider(None)  # cleared
        hb._publish_state("RUNNING", include_uptime=True)
        assert "instances" not in self._body(msg)

    def test_raising_provider_never_suppresses_keepalive(self, hb):
        def boom():
            raise RuntimeError("boom")
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb.set_instance_connectivity_provider(boom)
        hb._publish_state("RUNNING", include_uptime=True)
        msg._publish_reserved.assert_called_once()
        body = self._body(msg)
        assert body["status"] == "RUNNING"
        assert "instances" not in body

    def test_stopped_state_omits_instances(self, hb):
        from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb.set_instance_connectivity_provider(lambda: [InstanceConnectivity.of("x", True)])
        hb._publish_state("STOPPED", include_uptime=False)
        assert "instances" not in self._body(msg)

    def test_instance_connectivity_serializes_and_validates(self):
        import pytest
        from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
        assert InstanceConnectivity.of("plc1", True, "tcp://10.0.0.50:502").to_dict() == {
            "instance": "plc1", "connected": True, "detail": "tcp://10.0.0.50:502"}
        assert InstanceConnectivity.of("plc1", False).to_dict() == {
            "instance": "plc1", "connected": False}
        assert "detail" not in InstanceConnectivity("plc1", False, "  ").to_dict()
        with pytest.raises(ValueError):
            InstanceConnectivity("", True)
        with pytest.raises(ValueError):
            InstanceConnectivity("  ", True)


class TestPublishStateNow:
    """The public out-of-band re-emit used by the _bcast republish listener's
    republish-state action (DESIGN-uns §9.3/§9.4): same payload/seam/routing as a
    tick's keepalive, but respects heartbeat.enabled (a disabled state keepalive
    must not be re-enabled by a broadcast)."""

    def test_enabled_publishes_running_with_uptime(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb.publish_state_now()
        msg._publish_reserved.assert_called_once()
        topic, message = msg._publish_reserved.call_args[0]
        assert topic == "ecv1/thing-1/comp/main/state"
        assert message.get_header().name == "state"
        body = message.get_body()
        assert body["status"] == "RUNNING"
        assert isinstance(body["uptimeSecs"], int)

    def test_disabled_by_configuration_does_not_publish(self):
        cfg = FakeConfig({"enabled": False, "intervalSecs": 3600})
        h = EnhancedHeartbeat(cfg)
        try:
            msg = MagicMock()
            h.set_messaging_service(msg)
            h.publish_state_now()
            msg._publish_reserved.assert_not_called()
        finally:
            h.stop()

    def test_failure_is_swallowed(self, hb):
        msg = MagicMock()
        msg._publish_reserved.side_effect = RuntimeError("broker down")
        hb.set_messaging_service(msg)
        hb.publish_state_now()  # no exception


class TestGetUptimeSecs:
    """``get_uptime_secs()`` (DESIGN-uns §9.5): the command inbox's ``ping`` built-in
    verb's uptime source - must agree with the value the RUNNING state keepalive body
    carries as ``uptimeSecs`` (one shared uptime source)."""

    def test_returns_a_non_negative_int(self, hb):
        uptime = hb.get_uptime_secs()
        assert isinstance(uptime, int)
        assert uptime >= 0

    def test_agrees_with_the_state_keepalive_body(self, hb):
        msg = MagicMock()
        hb.set_messaging_service(msg)
        hb._publish_state("RUNNING", include_uptime=True)
        _, message = msg._publish_reserved.call_args[0]
        # Both reads happen back-to-back (monotonic clock) so they must be equal or,
        # at worst, one second apart across a tick boundary.
        assert abs(message.get_body()["uptimeSecs"] - hb.get_uptime_secs()) <= 1


class TestEmitSysMetric:
    def test_no_metric_service_returns(self, hb):
        hb._emit_sys_metric()  # no exception

    def test_flattens_and_emits_sys(self, hb):
        metric = MagicMock()
        hb.set_metric_service(metric)
        metric.reset_mock()
        hb._heartbeat_monitor = MagicMock()
        hb._heartbeat_monitor.get_stats.return_value = {
            "cpu": {"cpu_usage": 1.5}, "memory": {"memory_usage": 10.0},
        }
        hb._emit_sys_metric()
        metric.emit_metric_now.assert_called_once()
        name, values = metric.emit_metric_now.call_args[0]
        assert name == "sys"
        assert values == {"cpu_usage": 1.5, "memory_usage": 10.0}

    def test_non_numeric_values_skipped(self, hb):
        metric = MagicMock()
        hb.set_metric_service(metric)
        metric.reset_mock()
        hb._heartbeat_monitor = MagicMock()
        # non-numeric measure is skipped; non-dict category ignored
        hb._heartbeat_monitor.get_stats.return_value = {
            "cpu": {"cpu_usage": "NaNstr"}, "scalar": 5,
        }
        hb._emit_sys_metric()
        metric.emit_metric_now.assert_not_called()


class TestPublishHeartbeat:
    def test_tick_publishes_state_and_sys_metric(self):
        cfg = FakeConfig({
            "intervalSecs": 3600,
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
            assert msg._publish_reserved.called
            assert metric.emit_metric_now.called
            assert metric.emit_metric_now.call_args[0][0] == "sys"
        finally:
            h.stop()

    def test_state_failure_does_not_suppress_metric(self):
        h = EnhancedHeartbeat(FakeConfig())
        h.stop()
        try:
            msg, metric = MagicMock(), MagicMock()
            msg._publish_reserved.side_effect = RuntimeError("broker down")
            h.set_messaging_service(msg)
            h.set_metric_service(metric)
            metric.reset_mock()
            h._heartbeat_monitor = MagicMock()
            h._heartbeat_monitor.get_stats.return_value = {"cpu": {"cpu_usage": 1.0}}
            h._publish_heartbeat()  # no exception; the sys half still runs
            assert metric.emit_metric_now.called
        finally:
            h.stop()


class TestStoppedState:
    def test_stop_publishes_stopped_once(self):
        cfg = FakeConfig()
        h = EnhancedHeartbeat(cfg)
        msg = MagicMock()
        h.set_messaging_service(msg)
        h.stop()
        stopped_calls = [
            c for c in msg._publish_reserved.call_args_list
            if c[0][1].get_body().get("status") == "STOPPED"
        ]
        assert len(stopped_calls) == 1
        # a second stop() must not publish STOPPED again
        h.stop()
        stopped_calls = [
            c for c in msg._publish_reserved.call_args_list
            if c[0][1].get_body().get("status") == "STOPPED"
        ]
        assert len(stopped_calls) == 1

    def test_stop_when_never_running_does_not_publish(self):
        cfg = FakeConfig({"enabled": False})
        h = EnhancedHeartbeat(cfg)
        msg = MagicMock()
        h.set_messaging_service(msg)
        h.stop()
        msg._publish_reserved.assert_not_called()

    def test_stopped_publish_failure_is_swallowed(self):
        h = EnhancedHeartbeat(FakeConfig())
        msg = MagicMock()
        msg._publish_reserved.side_effect = RuntimeError("gone")
        h.set_messaging_service(msg)
        h.stop()  # no exception


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
