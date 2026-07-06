"""Unit tests for the MessagingClient static facade.

Exercises init() transport branching, config load/validate, the connected() readiness
helper, idempotent shutdown, and the delegation methods — all against a mock provider
or a patched StandaloneProvider, so no real broker is needed.
"""
import json
from argparse import Namespace
from unittest.mock import MagicMock

import pytest

import edgecommons.messaging.messaging_client as mc_mod
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.messaging.message import Message
from edgecommons.platform import Transport
from edgecommons.messaging.qos import Qos


@pytest.fixture(autouse=True)
def _reset_provider():
    MessagingClient._messaging_provider = None
    yield
    MessagingClient._messaging_provider = None


def _valid_config_file(tmp_path):
    cfg = {
        "messaging": {
            "local": {"type": "mqtt", "host": "localhost", "port": 1883, "clientId": "cid"}
        }
    }
    p = tmp_path / "messaging.json"
    p.write_text(json.dumps(cfg))
    return str(p)


class TestInit:
    def test_mqtt_without_path_raises(self):
        args = Namespace(transport=Transport.MQTT, identity="t", thing="t")
        with pytest.raises(RuntimeError, match="messaging config file path"):
            MessagingClient.init(args, None)

    def test_invalid_transport_raises(self):
        args = Namespace(transport="bogus", identity="t", thing="t")
        with pytest.raises(RuntimeError, match="Invalid transport"):
            MessagingClient.init(args)

    def test_mqtt_builds_standalone_provider(self, tmp_path, monkeypatch):
        fake_provider = MagicMock()
        captured = {}

        def fake_ctor(config, thing_name):
            captured["config"] = config
            captured["thing"] = thing_name
            return fake_provider

        monkeypatch.setattr(mc_mod, "StandaloneProvider", fake_ctor)
        args = Namespace(transport=Transport.MQTT, identity="ident-thing", thing="raw")
        provider = MessagingClient.init(args, _valid_config_file(tmp_path))
        assert provider is fake_provider
        assert MessagingClient.get_messaging_provider() is fake_provider
        # identity wins over raw thing flag
        assert captured["thing"] == "ident-thing"

    def test_mqtt_falls_back_to_thing_when_no_identity(self, tmp_path, monkeypatch):
        captured = {}
        monkeypatch.setattr(
            mc_mod, "StandaloneProvider",
            lambda config, thing_name: captured.setdefault("thing", thing_name) or MagicMock(),
        )
        args = Namespace(transport=Transport.MQTT, identity=None, thing="raw-thing")
        MessagingClient.init(args, _valid_config_file(tmp_path))
        assert captured["thing"] == "raw-thing"


class TestGetMessagingConfig:
    def test_loads_and_validates(self, tmp_path):
        cfg = MessagingClient._get_messaging_config(_valid_config_file(tmp_path))
        assert cfg.messaging.local is not None

    def test_invalid_config_raises(self, tmp_path):
        # empty messaging -> validate() returns False -> RuntimeError
        p = tmp_path / "bad.json"
        p.write_text(json.dumps({"messaging": {}}))
        with pytest.raises(RuntimeError):
            MessagingClient._get_messaging_config(str(p))

    def test_missing_file_raises(self, tmp_path):
        with pytest.raises(RuntimeError):
            MessagingClient._get_messaging_config(str(tmp_path / "missing.json"))


class TestConnected:
    def test_none_provider_not_connected(self):
        assert MessagingClient.connected() is False

    def test_provider_connected_true(self):
        prov = MagicMock()
        prov.connected.return_value = True
        MessagingClient._messaging_provider = prov
        assert MessagingClient.connected() is True

    def test_provider_exception_returns_false(self):
        prov = MagicMock()
        prov.connected.side_effect = RuntimeError("boom")
        MessagingClient._messaging_provider = prov
        assert MessagingClient.connected() is False


class TestShutdown:
    def test_idempotent(self):
        prov = MagicMock()
        MessagingClient._messaging_provider = prov
        MessagingClient.shutdown()
        prov.disconnect.assert_called_once()
        assert MessagingClient._messaging_provider is None
        # second call is a no-op
        MessagingClient.shutdown()


class TestDelegation:
    def setup_method(self):
        self.prov = MagicMock()
        MessagingClient._messaging_provider = self.prov

    def test_publish(self):
        m = Message()
        MessagingClient.publish("t", m)
        self.prov.publish.assert_called_once_with("t", m)

    def test_publish_raw(self):
        MessagingClient.publish_raw("t", {"a": 1})
        self.prov.publish_raw.assert_called_once_with("t", {"a": 1})

    def test_publish_northbound(self):
        m = Message()
        MessagingClient.publish_northbound("t", m, Qos.AT_LEAST_ONCE)
        self.prov.publish_northbound.assert_called_once_with("t", m, Qos.AT_LEAST_ONCE)

    def test_publish_northbound_raw(self):
        MessagingClient.publish_northbound_raw("t", {"a": 1}, Qos.AT_LEAST_ONCE)
        self.prov.publish_northbound_raw.assert_called_once_with("t", {"a": 1}, Qos.AT_LEAST_ONCE)

    def test_subscribe(self):
        cb = lambda t, m: None
        MessagingClient.subscribe("t", cb, 2, 5)
        self.prov.subscribe.assert_called_once_with("t", cb, 2, 5)

    def test_subscribe_northbound(self):
        cb = lambda t, m: None
        MessagingClient.subscribe_northbound("t", cb, Qos.AT_MOST_ONCE, 1, 3)
        self.prov.subscribe_northbound.assert_called_once_with("t", cb, Qos.AT_MOST_ONCE, 1, 3)

    def test_unsubscribe(self):
        MessagingClient.unsubscribe("t")
        self.prov.unsubscribe.assert_called_once_with("t")

    def test_unsubscribe_northbound(self):
        MessagingClient.unsubscribe_northbound("t")
        self.prov.unsubscribe_northbound.assert_called_once_with("t")

    def test_request(self):
        m = Message()
        MessagingClient.request("t", m)
        self.prov.request.assert_called_once_with("t", m, None)

    def test_request_northbound(self):
        m = Message()
        MessagingClient.request_northbound("t", m)
        self.prov.request_northbound.assert_called_once_with("t", m, None)

    def test_cancel_request(self):
        iou = object()
        MessagingClient.cancel_request(iou)
        self.prov.cancel_request.assert_called_once_with(iou)

    def test_cancel_request_northbound(self):
        iou = object()
        MessagingClient.cancel_request_northbound(iou)
        self.prov.cancel_request_northbound.assert_called_once_with(iou)

    def test_reply(self):
        req, rep = Message(), Message()
        MessagingClient.reply(req, rep)
        self.prov.reply.assert_called_once_with(req, rep)

    def test_reply_northbound(self):
        req, rep = Message(), Message()
        MessagingClient.reply_northbound(req, rep)
        self.prov.reply_northbound.assert_called_once_with(req, rep)

    def test_get_native_client(self):
        self.prov.get_native_client.return_value = {"x": 1}
        assert MessagingClient.get_native_client() == {"x": 1}

    def test_topic_matches_sub(self):
        assert MessagingClient.topic_matches_sub("a/+", "a/b") is True
        assert MessagingClient.topic_matches_sub("a/+", "a/b/c") is False
