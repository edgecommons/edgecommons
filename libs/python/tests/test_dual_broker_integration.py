"""Integration test: STANDALONE dual-broker (local MQTT + northbound MQTT together).

Exercises the combined scenario where a component connects to BOTH a local broker
and a northbound broker simultaneously and uses both transports. To run without cloud
infrastructure, the "northbound" endpoint is pointed at the SAME shared EMQX as "local" but over the mutual
-TLS listener (:8883), so the real dual-client / dual-transport code path is
exercised end-to-end. (Because both point at one broker, this validates that both
connections are live and both method sets work; true cross-broker isolation would
require two separate brokers, so distinct topics are used per transport.)

Requires the shared edgecommons-test-infra:
- EDGECOMMONS_TLS_CERTS_DIR pointing at tls-certs/ (ca.crt, client.crt, client.key)
- EMQX up with :1883 (plaintext) and :8883 (mutual TLS).
Skips cleanly if certs/broker are unavailable.
"""
import json
import os
import threading

import pytest

from edgecommons.messaging.messaging_config import MessagingConfiguration
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder

from edgecommons.messaging.qos import Qos

pytestmark = pytest.mark.integration

CERTS = os.environ.get("EDGECOMMONS_TLS_CERTS_DIR") or os.path.join(
    os.path.dirname(__file__), "tls-certs"
)
HAVE_CERTS = os.path.isdir(CERTS) and os.path.exists(os.path.join(CERTS, "ca.crt"))
LOCAL_PORT = int(os.environ.get("EDGECOMMONS_LOCAL_PORT", "1883"))
TLS_PORT = int(os.environ.get("EDGECOMMONS_TLS_PORT", "8883"))


def _dual_config(tmp_path):
    cfg = {
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": "localhost",
                "port": LOCAL_PORT,
                "clientId": "ggc-dual-local",
            },
            "northbound": {
                "host": "localhost",
                "port": TLS_PORT,
                "clientId": "ggc-dual-northbound",
                "credentials": {
                    "caPath": os.path.join(CERTS, "ca.crt"),
                    "certPath": os.path.join(CERTS, "client.crt"),
                    "keyPath": os.path.join(CERTS, "client.key"),
                },
            },
        }
    }
    path = tmp_path / "dual-messaging.json"
    path.write_text(json.dumps(cfg))
    return MessagingConfiguration.load_from_file(str(path))


@pytest.fixture
def provider(tmp_path):
    if not HAVE_CERTS:
        pytest.skip("run edgecommons-test-infra/gen-tls-certs.sh + set EDGECOMMONS_TLS_CERTS_DIR")
    config = _dual_config(tmp_path)
    # Sanity: the config really has both brokers.
    assert config.messaging.local is not None
    assert config.messaging.northbound is not None
    try:
        p = StandaloneProvider(config, "ggc-dual-thing")
    except Exception as e:
        pytest.skip(f"dual broker (local :{LOCAL_PORT} + TLS :{TLS_PORT}) not available ({e})")
    # Both native clients must exist when both sections are configured.
    clients = p.get_native_client()
    assert clients["local"] is not None
    assert clients["northbound"] is not None
    yield p
    p.disconnect()


def _msg(name, payload):
    return MessageBuilder.create(name, "1.0").with_payload(payload).with_tags({}).build()


def test_both_transports_deliver_simultaneously(provider):
    """A single provider connected to both brokers can publish/subscribe on the
    local transport AND the northbound transport at the same time."""
    local_topic = "ggc/dual/local"
    iot_topic = "ggc/dual/iot"
    local_got = threading.Event()
    iot_got = threading.Event()
    box = {}

    provider.subscribe(local_topic, lambda t, m: (box.__setitem__("local", m), local_got.set()))
    provider.subscribe_northbound(
        iot_topic, lambda t, m: (box.__setitem__("iot", m), iot_got.set()), Qos.AT_LEAST_ONCE, 1
    )

    provider.publish(local_topic, _msg("LocalMsg", {"via": "local"}))
    provider.publish_northbound(iot_topic, _msg("IotMsg", {"via": "iot"}), Qos.AT_LEAST_ONCE)

    assert local_got.wait(5), "local transport should deliver"
    assert iot_got.wait(5), "northbound transport should deliver"
    assert box["local"].get_body()["via"] == "local"
    assert box["iot"].get_body()["via"] == "iot"

    provider.unsubscribe(local_topic)
    provider.unsubscribe_northbound(iot_topic)


def test_request_reply_on_both_transports(provider):
    """request/reply works on the local transport and request_northbound /
    reply_northbound works on the northbound transport, with both connected."""
    # Local request/reply
    local_req_topic = "ggc/dual/local/req"
    provider.subscribe(
        local_req_topic,
        lambda t, req: provider.reply(req, _msg("LReply", {"answer": "local"})),
    )
    local_iou = provider.request(local_req_topic, _msg("LReq", {"q": 1}))
    done, reply = local_iou.get(5)
    assert done is True and reply.get_body()["answer"] == "local"

    # Northbound request/reply
    iot_req_topic = "ggc/dual/iot/req"
    provider.subscribe_northbound(
        iot_req_topic,
        lambda t, req: provider.reply_northbound(req, _msg("IReply", {"answer": "iot"})),
        Qos.AT_LEAST_ONCE,
        1,
    )
    iot_iou = provider.request_northbound(iot_req_topic, _msg("IReq", {"q": 2}))
    done2, reply2 = iot_iou.get(5)
    assert done2 is True and reply2.get_body()["answer"] == "iot"

    provider.unsubscribe(local_req_topic)
    provider.unsubscribe_northbound(iot_req_topic)
