"""
Integration test: verify a real *secure* (TLS) connection to the local broker.

Requires the shared ggcommons-test-infra repo:
- generate the TLS test certs there (bash gen-tls-certs.sh) and point at them with
  GGCOMMONS_TLS_CERTS_DIR, and
- start the broker (docker compose up -d) — EMQX TLS listener on :8883 trusting
  tls-certs/ca.crt and presenting tls-certs/server.crt.

Skips cleanly if the certs are missing or the secure connection cannot be established.
"""

import json
import os
import threading

import pytest

from ggcommons.messaging.messaging_config import MessagingConfiguration
from ggcommons.messaging.providers.standalone_provider import StandaloneProvider
from ggcommons.messaging.message import Message
from ggcommons.messaging.message_builder import MessageBuilder

pytestmark = pytest.mark.integration

CERTS = os.environ.get("GGCOMMONS_TLS_CERTS_DIR") or os.path.join(
    os.path.dirname(__file__), "tls-certs"
)
HAVE_CERTS = os.path.isdir(CERTS) and os.path.exists(os.path.join(CERTS, "ca.crt"))
TLS_PORT = int(os.environ.get("GGCOMMONS_TLS_PORT", "8883"))


def _ca():
    return os.path.join(CERTS, "ca.crt")


def _write_config(tmp_path, credentials):
    cfg = {
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": "localhost",
                "port": TLS_PORT,
                "clientId": "ggcommons-tls-test",
                "credentials": credentials,
            }
        }
    }
    path = tmp_path / "tls-messaging.json"
    path.write_text(json.dumps(cfg))
    return MessagingConfiguration.load_from_file(str(path))


def _roundtrip(provider):
    topic = "ggcommons/test/tls/roundtrip"
    received = []
    got = threading.Event()

    def handler(t, msg: Message):
        received.append((t, msg))
        got.set()

    provider.subscribe(topic, handler)
    message = (
        MessageBuilder.create("TlsTest", "1.0")
        .with_payload({"hello": "secure"})
        .with_tags({})
        .build()
    )
    provider.publish(topic, message)

    assert got.wait(timeout=5.0), "did not receive the published message over TLS"
    _, msg = received[0]
    assert msg.get_body()["hello"] == "secure"


@pytest.mark.skipif(not HAVE_CERTS, reason="run tests/gen-tls-certs.sh first")
def test_local_broker_mutual_tls_roundtrip(tmp_path):
    """Mutual TLS (CA + client cert) — works against a server-only or a
    client-cert-required EMQX TLS listener."""
    config = _write_config(
        tmp_path,
        {
            "caPath": _ca(),
            "certPath": os.path.join(CERTS, "client.crt"),
            "keyPath": os.path.join(CERTS, "client.key"),
        },
    )
    try:
        provider = StandaloneProvider(config, "ggcommons-tls-test")
    except Exception as e:
        pytest.skip(f"TLS broker not available on :{TLS_PORT} ({e})")
    try:
        _roundtrip(provider)
    finally:
        provider.disconnect()


@pytest.mark.skipif(not HAVE_CERTS, reason="run tests/gen-tls-certs.sh first")
def test_local_broker_server_only_tls_roundtrip(tmp_path):
    """Server-only TLS (CA only, no client cert). Requires an EMQX TLS listener
    that does not mandate client certs; skipped if the broker rejects it."""
    config = _write_config(tmp_path, {"caPath": _ca()})
    try:
        provider = StandaloneProvider(config, "ggcommons-tls-test-srv")
    except Exception as e:
        pytest.skip(f"server-only TLS not accepted on :{TLS_PORT} ({e})")
    try:
        _roundtrip(provider)
    finally:
        provider.disconnect()
