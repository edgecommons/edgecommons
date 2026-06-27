"""Issue #11: the local broker `type` is an optional, unvalidated tag that defaults to "mqtt".

Rust/TS/Java do not require it; Python previously did (a `KeyError` on a config without it). It
defaults to "mqtt" now so one standalone messaging config is portable across all four languages.
(Kept out of files named *messaging*/*iot*/*ggcommons* so it runs under the coverage gate.)
"""
import json

from ggcommons.messaging.messaging_config import MessagingConfiguration


def _write(tmp_path, obj):
    p = tmp_path / "broker.json"
    p.write_text(json.dumps(obj))
    return str(p)


class TestLocalBrokerTypeDefault:
    def test_type_defaults_to_mqtt_when_absent(self, tmp_path):
        path = _write(
            tmp_path,
            {"messaging": {"local": {"host": "localhost", "port": 1883, "clientId": "c-local"}}},
        )
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.local is not None
        assert cfg.messaging.local.type == "mqtt"
        # The genuinely-required fields still parse.
        assert cfg.messaging.local.host == "localhost"
        assert cfg.messaging.local.port == 1883
        assert cfg.messaging.local.client_id == "c-local"

    def test_explicit_type_is_preserved(self, tmp_path):
        path = _write(
            tmp_path,
            {"messaging": {"local": {"type": "mqtt", "host": "h", "port": 8883, "clientId": "c"}}},
        )
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.local.type == "mqtt"
