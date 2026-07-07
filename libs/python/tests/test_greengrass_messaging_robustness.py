"""Robustness unit tests for the Greengrass messaging provider (no real Greengrass).

Mirrors a Java fix (commit 6ed774c):
  Fix 1 - the reply handler must not raise on a late/duplicate reply whose Iou has
          already been completed and removed.
  Fix 2 - the subscription worker must suppress (log) an exception thrown by the
          registered callback so a bad message can never kill the worker thread.
"""
import threading

from edgecommons.messaging.providers.greengrass.greengrass_ipc import GreengrassIpcProvider
from edgecommons.messaging.providers.greengrass.ipc_subscription_handler import (
    IpcSubscriptionHandler,
)
from edgecommons.messaging.message import Message


def _provider_without_connect():
    # Bypass __init__ (which opens a real IPC connection); the reply handlers only
    # need the _response_ious map and the unsubscribe maps to be present.
    p = object.__new__(GreengrassIpcProvider)
    p._response_ious = {}
    p._ipc_subscription_operations = {}
    p._ipc_subscription_handlers = {}
    p._northbound_subscription_operations = {}
    p._northbound_subscription_handlers = {}
    return p


# ---------------------------------------------------------------------------
# Fix 1: late/duplicate reply must not raise
# ---------------------------------------------------------------------------

def test_on_reply_received_no_iou_does_not_raise():
    p = _provider_without_connect()
    # No Iou registered for this topic (simulates a late/duplicate reply after the
    # future was already completed + removed). Must not raise.
    p._on_reply_received("reply/absent/topic", Message.from_object({"body": "late"}))
    assert "reply/absent/topic" not in p._response_ious


def test_on_northbound_reply_received_no_iou_does_not_raise():
    p = _provider_without_connect()
    p._on_northbound_reply_received(
        "reply/absent/topic", Message.from_object({"body": "late"})
    )
    assert "reply/absent/topic" not in p._response_ious


def test_duplicate_reply_completes_once_then_is_ignored():
    p = _provider_without_connect()

    class _FakeIou:
        def __init__(self):
            self.results = []
            self.settled = False

        def try_settle(self):
            if self.settled:
                return False
            self.settled = True
            return True

        def set_result(self, r):
            self.results.append(r)

    iou = _FakeIou()
    p._response_ious["reply/topic"] = iou
    # Make unsubscribe a no-op (no real operation registered).
    p.unsubscribe = lambda topic: None

    first = Message.from_object({"body": "first"})
    second = Message.from_object({"body": "duplicate"})

    p._on_reply_received("reply/topic", first)
    # The second (late/duplicate) reply must be ignored without raising.
    p._on_reply_received("reply/topic", second)

    assert len(iou.results) == 1
    assert iou.results[0] is first
    assert "reply/topic" not in p._response_ious


# ---------------------------------------------------------------------------
# Fix 2: a throwing callback must be suppressed by the subscription worker
# ---------------------------------------------------------------------------

def test_subscription_callback_exception_is_suppressed():
    invoked = threading.Event()

    def bad_callback(topic, msg):
        invoked.set()
        raise ValueError("callback boom")

    handler = IpcSubscriptionHandler("some/topic", bad_callback, max_concurrency=1)
    try:
        # Feed a parsed (topic, payload) tuple directly onto the worker queue,
        # the same shape on_stream_event produces.
        handler._queue.put(("some/topic", Message.from_object({"body": "hello"})))
        assert invoked.wait(timeout=5.0), "callback was never invoked"

        # The worker thread must still be alive and processing after the throw.
        followup = threading.Event()

        def good_callback(topic, msg):
            followup.set()

        handler._callback_func = good_callback
        handler._queue.put(("some/topic", Message.from_object({"body": "again"})))
        assert followup.wait(
            timeout=5.0
        ), "worker thread died after a throwing callback"
    finally:
        # Stop the worker thread cleanly.
        handler.close()
        assert not handler._thread.is_alive()


def test_stream_error_requests_close():
    handler = IpcSubscriptionHandler("some/topic", lambda _topic, _msg: None)
    try:
        assert handler.on_stream_error(RuntimeError("ipc closed")) is True
    finally:
        handler.close()


def test_unsubscribe_stops_worker_when_operation_does_not_emit_stream_closed():
    p = _provider_without_connect()
    handler = IpcSubscriptionHandler("some/topic", lambda _topic, _msg: None)

    class _FakeOperation:
        def __init__(self):
            self.closed = False

        def close(self):
            self.closed = True

    operation = _FakeOperation()
    p._ipc_subscription_operations["some/topic"] = operation
    p._ipc_subscription_handlers["some/topic"] = handler

    p.unsubscribe("some/topic")

    assert operation.closed
    assert "some/topic" not in p._ipc_subscription_operations
    assert "some/topic" not in p._ipc_subscription_handlers
    assert not handler._thread.is_alive()
