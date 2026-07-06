"""Unit tests for heartbeat/metric messaging-destination routing (no broker).

The heartbeat ``state`` keepalive (UNS-CANONICAL-DESIGN §4.3) routes on
``heartbeat.destination``: ``"local"`` (default) -> the local/IPC transport,
``"northbound"`` -> the northbound transport; anything else falls back to local. The
publish goes through the privileged ``_publish_reserved*`` seam (the ``state`` class
is reserved).
"""
import pytest

from edgecommons.config.heartbeat_config import HeartbeatConfiguration
from edgecommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.metrics.targets.messaging import _is_local_destination


class _FakeMessaging:
    def __init__(self):
        self.local = []
        self.iot = []

    def _publish_reserved(self, topic, message):
        self.local.append((topic, message))

    def _publish_reserved_northbound(self, topic, message, qos):
        self.iot.append((topic, message))


class _FakeConfig:
    def __init__(self, destination=None):
        hb = {"intervalSecs": 3600}
        if destination is not None:
            hb["destination"] = destination
        self._hb = HeartbeatConfiguration(hb)

    def get_heartbeat_config(self):
        return self._hb

    def get_component_identity(self):
        return MessageIdentity([HierEntry("device", "thing")], "comp")

    def is_topic_include_root(self):
        return False

    def get_thing_name(self):
        return "thing"

    def get_component_name(self):
        return "comp"

    def get_tag_config(self):
        return None


def _heartbeat(fake_messaging, destination=None):
    # Bypass __init__ (which starts the loop thread); _publish_state only needs the
    # messaging + config handles.
    hb = object.__new__(EnhancedHeartbeat)
    hb._messaging_service = fake_messaging
    hb._config_service = _FakeConfig(destination)
    hb._warned_no_identity = False
    hb._start_monotonic = 0.0
    return hb


def test_heartbeat_local_destination_publishes_locally():
    fm = _FakeMessaging()
    _heartbeat(fm, "local")._publish_state("RUNNING", include_uptime=True)
    assert len(fm.local) == 1 and len(fm.iot) == 0
    topic, message = fm.local[0]
    assert topic == "ecv1/thing/comp/main/state"
    assert message.get_body()["status"] == "RUNNING"
    assert "uptimeSecs" in message.get_body()


def test_heartbeat_northbound_destination_publishes_to_iot_core_api():
    fm = _FakeMessaging()
    _heartbeat(fm, "northbound")._publish_state("RUNNING", include_uptime=True)
    assert len(fm.iot) == 1 and len(fm.local) == 0


def test_heartbeat_default_destination_is_local():
    fm = _FakeMessaging()
    # No destination -> defaults to "local" -> local transport (D-U14).
    _heartbeat(fm)._publish_state("RUNNING", include_uptime=True)
    assert len(fm.local) == 1 and len(fm.iot) == 0


def test_heartbeat_unrecognized_destination_falls_back_to_local():
    fm = _FakeMessaging()
    # Unlike the removed targets[] shape, an unrecognized destination now falls back
    # to the local transport (parity with the Java heartbeat and the metric target).
    _heartbeat(fm, "bogus")._publish_state("RUNNING", include_uptime=True)
    assert len(fm.local) == 1 and len(fm.iot) == 0


@pytest.mark.parametrize(
    "destination,is_local",
    # Northbound only for northbound; everything else (incl. unrecognized) is local.
    [("ipc", True), ("local", True), ("IPC", True), ("bogus", True),
     ("iot_core", True), ("iotcore", True), ("northbound", False), ("NORTHBOUND", False)],
)
def test_metric_is_local_destination(destination, is_local):
    assert _is_local_destination(destination) is is_local
