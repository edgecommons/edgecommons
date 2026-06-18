"""
Unit tests for TLS / messaging-config parity (no broker required):
- C3: IoT Core refuses to connect without complete TLS credentials
- local broker TLS is keyed on caPath (server-only) with optional client cert (mutual)
- IoT Core is now optional in the standalone messaging config (parity with Java/Rust)

The TLS-context construction tests need real cert files; they are skipped unless
tests/tls-certs/ has been generated (tests/gen-tls-certs.sh).
"""

import json
import os

import pytest

from ggcommons.messaging.providers.standalone_provider import StandaloneProvider
from ggcommons.messaging.messaging_config import MessagingConfiguration

CERTS = os.path.join(os.path.dirname(__file__), "tls-certs")
HAVE_CERTS = os.path.isdir(CERTS) and os.path.exists(os.path.join(CERTS, "ca.crt"))


class _FakeClient:
    def __init__(self):
        self.ctx = None

    def tls_set_context(self, ctx):
        self.ctx = ctx


class _Creds:
    def __init__(self, ca=None, cert=None, key=None):
        self.ca_path = ca
        self.cert_path = cert
        self.key_path = key


class _Broker:
    def __init__(self, creds):
        self.credentials = creds


def _provider():
    # Build an instance without __init__ (which would try to connect to brokers).
    return object.__new__(StandaloneProvider)


def test_iot_core_refuses_without_complete_credentials():
    prov = _provider()
    client = _FakeClient()
    # Missing cert+key -> must refuse rather than silently connect without TLS (C3).
    with pytest.raises(RuntimeError, match="without complete TLS credentials"):
        prov._configure_tls(client, _Broker(_Creds(ca="/x/ca.crt")), "iotcore")
    assert client.ctx is None


def test_local_without_ca_is_plaintext():
    prov = _provider()
    client = _FakeClient()
    prov._configure_tls(client, _Broker(_Creds()), "local")
    assert client.ctx is None  # no TLS configured


def test_iot_core_optional_local_only_config(tmp_path):
    cfg = {
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": "localhost",
                "port": 8883,
                "clientId": "c",
            }
        }
    }
    path = tmp_path / "m.json"
    path.write_text(json.dumps(cfg))
    mc = MessagingConfiguration.load_from_file(str(path))
    assert mc.messaging.iot_core is None
    assert mc.messaging.local is not None
    assert mc.validate() is True


def test_config_requires_at_least_one_broker(tmp_path):
    path = tmp_path / "empty.json"
    path.write_text(json.dumps({"messaging": {}}))
    mc = MessagingConfiguration.load_from_file(str(path))
    assert mc.validate() is False


@pytest.mark.skipif(not HAVE_CERTS, reason="run tests/gen-tls-certs.sh first")
def test_local_server_only_tls_builds_context():
    prov = _provider()
    client = _FakeClient()
    prov._configure_tls(
        client, _Broker(_Creds(ca=os.path.join(CERTS, "ca.crt"))), "local"
    )
    assert client.ctx is not None  # server-only TLS (CA only, no client cert)


@pytest.mark.skipif(not HAVE_CERTS, reason="run tests/gen-tls-certs.sh first")
def test_local_mutual_tls_builds_context():
    prov = _provider()
    client = _FakeClient()
    prov._configure_tls(
        client,
        _Broker(
            _Creds(
                ca=os.path.join(CERTS, "ca.crt"),
                cert=os.path.join(CERTS, "client.crt"),
                key=os.path.join(CERTS, "client.key"),
            )
        ),
        "local",
    )
    assert client.ctx is not None  # mutual TLS (CA + client cert)


@pytest.mark.skipif(not HAVE_CERTS, reason="run tests/gen-tls-certs.sh first")
def test_iot_core_complete_creds_builds_context():
    prov = _provider()
    client = _FakeClient()
    prov._configure_tls(
        client,
        _Broker(
            _Creds(
                ca=os.path.join(CERTS, "ca.crt"),
                cert=os.path.join(CERTS, "client.crt"),
                key=os.path.join(CERTS, "client.key"),
            )
        ),
        "iotcore",
    )
    assert client.ctx is not None
