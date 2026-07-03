"""Unit tests for the framework-owned ``request()`` deadline (UNS-CANONICAL-DESIGN
§5, D-U5/D-U23): the Iou settle CAS, the deadline timer arming/cleanup, and the
Iou.get() raise contract. Uses short deadlines + events (no sleeps beyond the armed
timers, no broker)."""
import threading
import time

import pytest

from ggcommons.messaging.errors import RequestTimeoutError
from ggcommons.messaging.messaging_provider import MessagingProvider
from ggcommons.messaging.providers.standalone_provider import StandaloneProvider, _BrokerChannel
from ggcommons.messaging.message import Message, MessageHeader
from ggcommons.utils.iou import Iou


class TestIouSettle:
    def test_try_settle_single_winner(self):
        iou = Iou("t")
        assert iou.try_settle() is True
        assert iou.try_settle() is False  # losers no-op
        assert iou.is_settled() is True

    def test_set_error_raises_on_get(self):
        iou = Iou("t")
        iou.set_error(RequestTimeoutError("expired"))
        with pytest.raises(RequestTimeoutError):
            iou.get(0.1)

    def test_get_timeout_returns_not_done(self):
        iou = Iou("t")
        done, result = iou.get(0.01)
        assert done is False and result is iou

    def test_get_result_after_set(self):
        iou = Iou("t")
        iou.set_result({"ok": 1})
        assert iou.get(0.1) == (True, {"ok": 1})

    def test_settle_cancels_attached_timer(self):
        iou = Iou("t")
        fired = threading.Event()
        timer = threading.Timer(0.05, fired.set)
        timer.daemon = True
        timer.start()
        iou._set_deadline_timer(timer)
        assert iou.try_settle() is True
        time.sleep(0.15)
        assert not fired.is_set(), "settle winner must cancel the deadline timer"

    def test_timer_attached_after_settle_is_canceled(self):
        iou = Iou("t")
        iou.try_settle()
        fired = threading.Event()
        timer = threading.Timer(0.05, fired.set)
        timer.daemon = True
        timer.start()
        iou._set_deadline_timer(timer)  # reply beat the arming call
        time.sleep(0.15)
        assert not fired.is_set()


class _DummyProvider(MessagingProvider):
    """Concrete provider exposing only the base-class deadline machinery."""

    def disconnect(self): ...
    def connected(self): return True
    def publish(self, topic, msg): ...
    def publish_raw(self, topic, msg): ...
    def publish_to_iot_core(self, topic, msg, qos): ...
    def publish_to_iot_core_raw(self, topic, msg, qos): ...
    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None): ...
    def subscribe_to_iot_core(self, topic, callback, qos, max_concurrency=None, max_messages=None): ...
    def unsubscribe(self, topic): ...
    def unsubscribe_from_iot_core(self, topic): ...
    def request(self, topic, msg, timeout_secs=None): ...
    def request_from_iot_core(self, topic, msg, timeout_secs=None): ...
    def reply(self, request_msg, response_msg): ...
    def reply_to_iot_core(self, request_msg, response_msg): ...
    def cancel_request(self, iou): ...
    def cancel_request_from_iot_core(self, iou): ...
    def get_native_client(self): ...


class TestProviderDeadlineMachinery:
    def test_builtin_default_is_30s(self):
        assert _DummyProvider().get_default_request_timeout() == 30.0

    def test_set_default_and_disable(self):
        p = _DummyProvider()
        p.set_default_request_timeout(12)
        assert p.get_default_request_timeout() == 12.0
        p.set_default_request_timeout(0)
        assert p.get_default_request_timeout() == 0.0
        p.set_default_request_timeout(None)
        assert p.get_default_request_timeout() == 0.0

    def test_effective_timeout_resolution(self):
        p = _DummyProvider()
        assert p._effective_request_timeout(None) == 30.0   # default
        assert p._effective_request_timeout(5) == 5.0       # per-call wins
        assert p._effective_request_timeout(0) is None      # 0 = disabled per call
        p.set_default_request_timeout(0)
        assert p._effective_request_timeout(None) is None   # disabled default

    def test_deadline_fires_cleanup_and_error_without_get(self):
        # The deadline must fire (cleanup + exceptional completion) even if the
        # caller never get()'s the Iou — the reply-subscription leak fix.
        p = _DummyProvider()
        iou = Iou("reply/t")
        cleaned = threading.Event()
        p._arm_request_deadline(iou, 0.05, cleaned.set)
        assert cleaned.wait(2.0), "deadline cleanup must run"
        with pytest.raises(RequestTimeoutError):
            iou.get(1.0)

    def test_deadline_noop_when_settled_first(self):
        p = _DummyProvider()
        iou = Iou("reply/t")
        cleaned = threading.Event()
        p._arm_request_deadline(iou, 0.05, cleaned.set)
        assert iou.try_settle() is True  # reply won the race
        iou.set_result("reply")
        time.sleep(0.15)
        assert not cleaned.is_set(), "a settled request must not run deadline cleanup"
        assert iou.get(0.1) == (True, "reply")

    def test_disabled_deadline_never_arms(self):
        p = _DummyProvider()
        iou = Iou("reply/t")
        p._arm_request_deadline(iou, None, lambda: pytest.fail("must not run"))
        assert iou._deadline_timer is None

    def test_cleanup_failure_still_completes_exceptionally(self):
        p = _DummyProvider()
        iou = Iou("reply/t")

        def boom():
            raise RuntimeError("cleanup failed")

        p._arm_request_deadline(iou, 0.05, boom)
        with pytest.raises(RequestTimeoutError):
            iou.get(2.0)


def _standalone_without_broker():
    """A StandaloneProvider with the connect path bypassed (the existing test seam)."""
    prov = StandaloneProvider.__new__(StandaloneProvider)
    prov._lock = threading.RLock()
    prov._response_ious = {}
    return prov


class _RecordingChannel(_BrokerChannel):
    pass


class TestStandaloneRequestDeadline:
    def _provider_with_channel(self):
        prov = _standalone_without_broker()
        channel = _BrokerChannel("local")

        class _Client:
            def __init__(self):
                self.unsubscribed = []
                self.published = []

            def unsubscribe(self, topic):
                self.unsubscribed.append(topic)

            def publish(self, topic, payload, qos=0):
                self.published.append((topic, payload, qos))

                class _R:
                    rc = 0
                return _R()

        channel.client = _Client()
        return prov, channel

    def _pending_request(self, prov, channel, reply_topic):
        iou = Iou(reply_topic)
        prov._response_ious[reply_topic] = iou
        channel.subscriptions[reply_topic] = {"callback": None, "semaphore": None}
        return iou

    def test_deadline_unsubscribes_and_raises(self):
        prov, channel = self._provider_with_channel()
        reply_topic = "ggcommons/reply-deadline"
        iou = self._pending_request(prov, channel, reply_topic)

        def cleanup():
            with prov._lock:
                prov._response_ious.pop(reply_topic, None)
            prov._unsubscribe(channel, reply_topic)

        prov._arm_request_deadline(iou, 0.05, cleanup)
        with pytest.raises(RequestTimeoutError):
            iou.get(2.0)
        assert reply_topic not in prov._response_ious
        assert reply_topic not in channel.subscriptions
        assert channel.client.unsubscribed == [reply_topic]

    def test_reply_settles_and_deadline_noops(self):
        import json as _json
        import types

        prov, channel = self._provider_with_channel()
        reply_topic = "ggcommons/reply-served"
        iou = self._pending_request(prov, channel, reply_topic)
        prov._arm_request_deadline(iou, 5.0, lambda: pytest.fail("deadline must not fire"))

        mqtt_msg = types.SimpleNamespace(
            topic=reply_topic,
            payload=_json.dumps({"body": {"ok": True}}).encode("utf-8"),
            qos=0,
        )
        prov._process_message(mqtt_msg, channel)
        done, reply = iou.get(1.0)
        assert done is True and reply.get_body() == {"ok": True}
        # cleanup happened on the reply path
        assert reply_topic not in prov._response_ious
        assert reply_topic not in channel.subscriptions

    def test_straggler_reply_after_settle_is_dropped(self):
        import json as _json
        import types

        prov, channel = self._provider_with_channel()
        reply_topic = "ggcommons/reply-straggler"
        iou = self._pending_request(prov, channel, reply_topic)
        assert iou.try_settle() is True  # e.g. the deadline already settled it
        iou.set_error(RequestTimeoutError("expired"))

        mqtt_msg = types.SimpleNamespace(
            topic=reply_topic,
            payload=_json.dumps({"body": {"late": True}}).encode("utf-8"),
            qos=0,
        )
        prov._process_message(mqtt_msg, channel)  # no exception, dropped at DEBUG
        with pytest.raises(RequestTimeoutError):
            iou.get(0.1)

    def test_cancel_request_is_idempotent_after_settle(self):
        prov, channel = self._provider_with_channel()
        reply_topic = "ggcommons/reply-cancel"
        iou = self._pending_request(prov, channel, reply_topic)
        prov._cancel_request(channel, iou)
        assert reply_topic not in prov._response_ious
        assert iou.get(0.1) == (True, None)  # canceled request completes with None
        # a second cancel (or a cancel racing the deadline) no-ops
        prov._cancel_request(channel, iou)

    def test_request_arms_deadline_at_send(self, monkeypatch):
        prov, channel = self._provider_with_channel()
        prov._local = channel
        prov._subscription_timeout = 0.1
        # Bypass the SUBACK wait: record the subscription synchronously.
        monkeypatch.setattr(
            StandaloneProvider, "_subscribe",
            lambda self, ch, topic, cb, qos, mc, mm=None: ch.subscriptions.__setitem__(
                topic, {"callback": cb, "semaphore": None}
            ),
        )
        msg = Message()
        msg.header = MessageHeader("N", "1")
        iou = prov._request(channel, "cmd/topic", msg, 0, 0, timeout_secs=0.05)
        with pytest.raises(RequestTimeoutError):
            iou.get(2.0)
        # the deadline cleaned up the pending entry + reply subscription
        assert iou.get_user_data() not in prov._response_ious
        assert iou.get_user_data() not in channel.subscriptions
