"""Regression test for the STANDALONE reply-subscription leak (parity gap #7).

A served request/reply must tear down its one-shot ``edgecommons/reply-<uuid>`` subscription, the
same way the IPC path and ``_cancel_request`` do. Before the fix, the standalone reply path popped
the pending future but never unsubscribed, orphaning a broker subscription on every request.
Drives ``_process_message`` directly so no broker is required.
"""
import threading
import types
from unittest.mock import MagicMock

from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.providers.standalone_provider import (
    StandaloneProvider,
    _BrokerChannel,
)
from edgecommons.utils.iou import Iou


def _fake_mqtt_message(topic: str, body: dict):
    m = types.SimpleNamespace()
    m.topic = topic
    m.payload = MessageBuilder.create("Delivered", "1.0").with_payload(body).build().to_bytes()
    m.qos = 0
    return m


def test_standalone_reply_unsubscribes_reply_topic():
    # Build a provider without connecting (bypass __init__'s broker connect).
    prov = StandaloneProvider.__new__(StandaloneProvider)
    prov._lock = threading.RLock()
    prov._response_ious = {}

    channel = _BrokerChannel("local")
    channel.client = MagicMock()
    reply_topic = "edgecommons/reply-abc123"

    # Reproduce the state _request leaves behind: a pending iou + a live reply subscription.
    iou = Iou(reply_topic)
    prov._response_ious[reply_topic] = iou
    channel.subscriptions[reply_topic] = {"callback": None, "semaphore": None}

    prov._process_message(_fake_mqtt_message(reply_topic, {"ok": True}), channel)

    # The reply resolved the pending future...
    done, _ = iou.get(0.5)
    assert done is True
    # ...and the one-shot subscription was torn down (no leak).
    assert reply_topic not in channel.subscriptions
    channel.client.unsubscribe.assert_called_once_with(reply_topic)
    assert reply_topic not in prov._response_ious
