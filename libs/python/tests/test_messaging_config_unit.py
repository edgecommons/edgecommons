"""Unit tests for MessagingConfiguration.load_from_file and validate()."""
import json

import pytest

from edgecommons.messaging.messaging_config import MessagingConfiguration


def _write(tmp_path, data):
    p = tmp_path / "messaging.json"
    p.write_text(json.dumps(data))
    return str(p)


class TestLoadFromFile:
    def test_local_only(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {"local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"}}
        })
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.local.host == "h"
        assert cfg.messaging.local.port == 1883
        assert cfg.messaging.northbound is None

    def test_local_with_credentials(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {
                "local": {
                    "type": "mqtt", "host": "h", "port": 1883, "clientId": "c",
                    "credentials": {
                        "username": "u", "password": "p",
                        "certPath": "cert", "keyPath": "key", "caPath": "ca",
                    },
                }
            }
        })
        cfg = MessagingConfiguration.load_from_file(path)
        creds = cfg.messaging.local.credentials
        assert creds.username == "u" and creds.password == "p"
        assert creds.cert_path == "cert" and creds.key_path == "key" and creds.ca_path == "ca"

    def test_dual_broker(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {
                "local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"},
                "northbound": {
                    "endpoint": "ep", "port": 8883, "clientId": "ic",
                    "credentials": {"certPath": "cp", "keyPath": "kp", "caPath": "ap"},
                },
            }
        })
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.northbound.endpoint == "ep"
        assert cfg.messaging.northbound.credentials.ca_path == "ap"

    def test_northbound_without_credentials_is_valid(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {"northbound": {"endpoint": "ep", "port": 8883, "clientId": "ic"}}
        })
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.northbound.endpoint == "ep"
        assert cfg.messaging.northbound.credentials is None
        assert cfg.validate() is True

    def test_empty_messaging_section(self, tmp_path):
        path = _write(tmp_path, {"messaging": {}})
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.local is None and cfg.messaging.northbound is None

    def test_generic_messaging_lwt_is_rejected(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {
                "local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"},
                "lwt": {"topic": "ecv1/d/uns-bridge/main/state"},
            }
        })
        with pytest.raises(ValueError, match=r"messaging\.lwt is not supported"):
            MessagingConfiguration.load_from_file(path)

    def test_top_level_messaging_qos_is_rejected(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {
                "local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"},
                "qos": {"local": {"publish": 1}},
            }
        })
        with pytest.raises(ValueError, match=r"messaging\.qos is not supported"):
            MessagingConfiguration.load_from_file(path)

    def test_nested_broker_qos_is_loaded(self, tmp_path):
        path = _write(tmp_path, {
            "messaging": {
                "local": {
                    "type": "mqtt", "host": "h", "port": 1883, "clientId": "c",
                    "qos": {"publish": 2, "subscribe": 0},
                },
                "northbound": {
                    "endpoint": "ep", "port": 8883, "clientId": "ic",
                    "qos": {"publish": 2, "subscribe": 0},
                },
            }
        })
        cfg = MessagingConfiguration.load_from_file(path)
        assert cfg.messaging.local.qos.publish == 2
        assert cfg.messaging.local.qos.subscribe == 0
        assert cfg.messaging.northbound.qos.publish == 2
        assert cfg.messaging.northbound.qos.subscribe == 0

    def test_missing_file_raises(self, tmp_path):
        with pytest.raises(FileNotFoundError):
            MessagingConfiguration.load_from_file(str(tmp_path / "nope.json"))

    def test_invalid_json_raises(self, tmp_path):
        p = tmp_path / "bad.json"
        p.write_text("{ not valid")
        with pytest.raises(json.JSONDecodeError):
            MessagingConfiguration.load_from_file(str(p))

    def test_missing_required_key_raises(self, tmp_path):
        # local missing 'host' -> KeyError surfaces
        path = _write(tmp_path, {
            "messaging": {"local": {"type": "mqtt", "port": 1883, "clientId": "c"}}
        })
        with pytest.raises(KeyError):
            MessagingConfiguration.load_from_file(path)


class TestValidate:
    def _cfg(self, tmp_path, data):
        return MessagingConfiguration.load_from_file(_write(tmp_path, data))

    def test_valid_local_only(self, tmp_path):
        cfg = self._cfg(tmp_path, {
            "messaging": {"local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"}}
        })
        assert cfg.validate() is True

    def test_no_messaging_invalid(self):
        cfg = MessagingConfiguration()  # messaging is None
        assert cfg.validate() is False

    def test_no_brokers_invalid(self, tmp_path):
        cfg = self._cfg(tmp_path, {"messaging": {}})
        assert cfg.validate() is False

    def test_valid_dual(self, tmp_path):
        cfg = self._cfg(tmp_path, {
            "messaging": {
                "local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"},
                "northbound": {
                    "endpoint": "ep", "port": 8883, "clientId": "ic",
                    "credentials": {"certPath": "cp", "keyPath": "kp", "caPath": "ap"},
                },
            }
        })
        assert cfg.validate() is True

    def test_northbound_missing_host_invalid(self, tmp_path):
        cfg = self._cfg(tmp_path, {
            "messaging": {
                "northbound": {"endpoint": "ep", "port": 8883, "clientId": "ic"}
            }
        })
        cfg.messaging.northbound.endpoint = None
        assert cfg.validate() is False

    def test_local_missing_host_invalid(self, tmp_path):
        cfg = self._cfg(tmp_path, {
            "messaging": {"local": {"type": "mqtt", "host": "h", "port": 1883, "clientId": "c"}}
        })
        cfg.messaging.local.host = None
        assert cfg.validate() is False
