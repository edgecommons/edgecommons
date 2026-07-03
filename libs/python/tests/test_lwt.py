"""Unit tests for the MQTT Last-Will-and-Testament hook (UNS-CANONICAL-DESIGN §6):
the messaging-config ``lwt`` parse and the paho ``will_set`` registration on the
LOCAL connection only, retain hard-wired to False. No broker required."""
import json

import pytest

from ggcommons.messaging.messaging_config import LwtConfig, MessagingConfiguration
from ggcommons.messaging.providers.standalone_provider import StandaloneProvider


class _FakeClient:
    def __init__(self):
        self.will = None

    def will_set(self, topic, payload=None, qos=0, retain=False):
        self.will = {"topic": topic, "payload": payload, "qos": qos, "retain": retain}


class TestApplyLwt:
    def test_absent_section_is_noop(self):
        client = _FakeClient()
        StandaloneProvider._apply_lwt(client, None)
        assert client.will is None

    def test_object_payload_serialized_compact(self):
        client = _FakeClient()
        lwt = LwtConfig(topic="ecv1/gw-01/uns-bridge/main/state",
                        payload={"status": "UNREACHABLE"}, qos=1)
        StandaloneProvider._apply_lwt(client, lwt)
        assert client.will["topic"] == "ecv1/gw-01/uns-bridge/main/state"
        assert json.loads(client.will["payload"]) == {"status": "UNREACHABLE"}
        assert client.will["payload"] == b'{"status":"UNREACHABLE"}'  # compact JSON
        assert client.will["qos"] == 1
        assert client.will["retain"] is False  # hard-wired, no retain knob (D9)

    def test_string_payload_verbatim_utf8(self):
        client = _FakeClient()
        StandaloneProvider._apply_lwt(client, LwtConfig(topic="t", payload="gone", qos=0))
        assert client.will["payload"] == b"gone"
        assert client.will["qos"] == 0

    def test_absent_payload_empty_bytes(self):
        client = _FakeClient()
        StandaloneProvider._apply_lwt(client, LwtConfig(topic="t"))
        assert client.will["payload"] == b""

    def test_qos_defaults_to_1(self):
        client = _FakeClient()
        StandaloneProvider._apply_lwt(client, LwtConfig(topic="t", payload="x"))
        assert client.will["qos"] == 1

    def test_qos_float_coerced(self):
        # JSON parses "qos": 1.0 as a float; it must coerce to the integral QoS.
        client = _FakeClient()
        StandaloneProvider._apply_lwt(client, LwtConfig(topic="t", qos=1.0))
        assert client.will["qos"] == 1 and isinstance(client.will["qos"], int)

    def test_missing_topic_rejected(self):
        with pytest.raises(ValueError):
            StandaloneProvider._apply_lwt(_FakeClient(), LwtConfig(topic=None))
        with pytest.raises(ValueError):
            StandaloneProvider._apply_lwt(_FakeClient(), LwtConfig(topic=""))

    @pytest.mark.parametrize("qos", [2, -1, 0.5, "one"])
    def test_bad_qos_rejected(self, qos):
        with pytest.raises(ValueError):
            StandaloneProvider._apply_lwt(_FakeClient(), LwtConfig(topic="t", qos=qos))


class TestLwtConfigParse:
    def test_parsed_from_messaging_section(self, tmp_path):
        cfg_path = tmp_path / "messaging.json"
        cfg_path.write_text(json.dumps({
            "messaging": {
                "local": {"host": "localhost", "port": 1883, "clientId": "c1"},
                "lwt": {
                    "topic": "ecv1/gw-01/comp/main/state",
                    "payload": {"status": "UNREACHABLE"},
                    "qos": 1,
                },
            }
        }))
        config = MessagingConfiguration.load_from_file(str(cfg_path))
        lwt = config.messaging.lwt
        assert lwt is not None
        assert lwt.topic == "ecv1/gw-01/comp/main/state"
        assert lwt.payload == {"status": "UNREACHABLE"}
        assert lwt.qos == 1
        assert config.validate() is True

    def test_absent_lwt_is_none(self, tmp_path):
        cfg_path = tmp_path / "messaging.json"
        cfg_path.write_text(json.dumps({
            "messaging": {"local": {"host": "localhost", "port": 1883, "clientId": "c1"}}
        }))
        config = MessagingConfiguration.load_from_file(str(cfg_path))
        assert config.messaging.lwt is None


class TestLwtWiring:
    def test_local_client_gets_the_will(self, monkeypatch):
        """_create_mqtt_client registers the will on the LOCAL channel only."""
        import ggcommons.messaging.providers.standalone_provider as sp

        class _RecordingMqtt(_FakeClient):
            def __init__(self, *args, **kwargs):
                super().__init__()

            def username_pw_set(self, u, p):
                pass

            def tls_set_context(self, ctx):
                pass

        monkeypatch.setattr(sp.mqtt, "Client", _RecordingMqtt)

        prov = StandaloneProvider.__new__(StandaloneProvider)
        prov._thing_name = "thing-1"

        class _MsgCfg:
            lwt = LwtConfig(topic="ecv1/gw/comp/main/state", payload="down", qos=0)
        prov._messaging_config = _MsgCfg()

        class _BrokerCfg:
            host = "localhost"
            port = 1883
            client_id = "c1"
            credentials = None

        local = sp._BrokerChannel("local")
        client = prov._create_mqtt_client(_BrokerCfg(), local)
        assert client.will == {
            "topic": "ecv1/gw/comp/main/state", "payload": b"down", "qos": 0,
            "retain": False,
        }

        # The IoT Core channel never registers the will. (IoT Core LWT is deferred;
        # the local-broker connection is the §6 scope.)
        class _IotCfg:
            endpoint = "e.example.com"
            port = 8883
            client_id = "c1"

            class credentials:
                ca_path = "ca"
                cert_path = "cert"
                key_path = "key"

        monkeypatch.setattr(StandaloneProvider, "_configure_tls", lambda *a, **k: None)
        iot = sp._BrokerChannel("iotcore")
        iot_client = prov._create_mqtt_client(_IotCfg(), iot)
        assert iot_client.will is None
