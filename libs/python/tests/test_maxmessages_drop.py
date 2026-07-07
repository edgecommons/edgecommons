"""Regression test for the per-subscription max_messages queue bound (parity gap #12).

A bounded subscription drops messages on overflow (non-blocking) rather than letting the
dispatch backlog grow unbounded — parity with the Rust (bounded mpsc) / TS (drop-on-overflow)
providers. Drives _process_message directly so no broker is required; deterministic via a
callback that blocks (holding its queue permit) until released.
"""
import threading
import types
from concurrent.futures import ThreadPoolExecutor

from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider, _BrokerChannel


def _fake_mqtt_message(topic: str, body: dict):
    m = types.SimpleNamespace()
    m.topic = topic
    m.payload = MessageBuilder.create("Delivered", "1.0").with_payload(body).build().to_bytes()
    m.qos = 0
    return m


def test_bounded_subscription_drops_on_overflow():
    prov = StandaloneProvider.__new__(StandaloneProvider)
    prov._lock = threading.RLock()
    prov._response_ious = {}
    prov._executor = ThreadPoolExecutor(max_workers=2)

    started = threading.Event()
    release = threading.Event()
    count = [0]

    def cb(_topic, _msg):
        count[0] += 1
        started.set()
        release.wait(2)  # hold the queue permit until released

    channel = _BrokerChannel("local")
    channel.subscriptions["t/1"] = {
        "callback": cb,
        "semaphore": None,
        "max_messages": 1,
        "queue_permits": threading.Semaphore(1),  # capacity of exactly one in-flight/queued message
    }

    try:
        # First message: acquires the single permit, runs cb (which blocks holding the permit).
        prov._process_message(_fake_mqtt_message("t/1", {"n": 1}), channel)
        assert started.wait(2), "first callback should start"

        # Second message: no permit available -> dropped (not dispatched).
        prov._process_message(_fake_mqtt_message("t/1", {"n": 2}), channel)

        # Give any (incorrectly) dispatched second callback a chance to run, then assert it didn't.
        release.set()  # let the first callback finish + release its permit
        prov._executor.shutdown(wait=True)
        assert count[0] == 1, "the overflow message must have been dropped, not dispatched"
    finally:
        release.set()
        prov._executor.shutdown(wait=True)
