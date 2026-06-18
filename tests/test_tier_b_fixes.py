"""Unit tests for Tier B correctness fixes (no broker / AWS required).

Covers:
- heartbeat_config.to_dict() no longer crashes and round-trips
- HeartbeatConfiguration default targets are not shared across instances
- MessageHeader.from_dict() without a reply_to does not raise UnboundLocalError
- ConfigManager rebuilds the instance map on reload (stale instances removed)
"""
from ggcommons.config.heartbeat_config import HeartbeatConfiguration
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import MessageHeader


def test_heartbeat_to_dict_round_trips():
    src = {
        "intervalSecs": 7,
        "measures": {
            "cpu": True, "memory": False, "disk": True,
            "files": True, "threads": False, "fds": True,
        },
        "targets": [{"type": "metric"}],
    }
    hb = HeartbeatConfiguration(src)
    d = hb.to_dict()  # previously raised AttributeError/TypeError
    assert d["intervalSecs"] == 7
    assert d["measures"] == src["measures"]
    assert d["targets"] == src["targets"]
    # Feeding to_dict() output back in reproduces the same dict.
    assert HeartbeatConfiguration(d).to_dict() == d


def test_heartbeat_default_targets_not_shared():
    a = HeartbeatConfiguration(None)
    b = HeartbeatConfiguration(None)
    a.get_targets().append({"type": "extra"})
    assert len(b.get_targets()) == 1, "mutating one instance must not corrupt the default"


def test_message_header_from_dict_without_reply_to():
    header = MessageHeader.from_dict({"name": "X", "version": "1.0"})
    assert header.reply_to is None
    assert header.name == "X"


def test_config_manager_instances_reset_on_reload():
    cm = ConfigManager("comp", "thing", validate_config=False)
    cm._apply_config(
        {"component": {"global": {}, "instances": [{"id": "a"}, {"id": "b"}]}}
    )
    assert set(cm.get_instance_ids()) == {"a", "b"}
    # Reload that drops instance 'b' must not leave a stale entry behind.
    cm._apply_config({"component": {"global": {}, "instances": [{"id": "a"}]}})
    assert set(cm.get_instance_ids()) == {"a"}
