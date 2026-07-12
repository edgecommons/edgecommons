"""Adversarial coverage for explicit and deferred command outcomes."""

import math
import threading
import time

import pytest

import edgecommons.command_inbox as command_module
from edgecommons.command_inbox import (
    CommandException,
    CommandInbox,
    CommandOutcome,
    Deferred,
    DeferredReplyState,
    ImmediateError,
    ImmediateSuccess,
    SettlementResult,
)
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient


INBOX_FILTER = "ecv1/test-thing/TestComponent/main/cmd/#"
REPLY_TO = "edgecommons/reply-command-outcome"


class _Config:
    def __init__(self):
        self.identity = MessageIdentity(
            [HierEntry("device", "test-thing")], "TestComponent"
        )

    def get_component_identity(self):
        return self.identity

    def is_topic_include_root(self):
        return False

    def get_tag_config(self):
        return None


class _Messaging:
    def __init__(self):
        self.callbacks = {}
        self.published = []
        self.confirmed_attempts = 0
        self.confirmed_failures = 0
        self.validated = []

    def subscribe_acknowledged(
        self,
        topic,
        callback,
        max_concurrency=None,
        max_messages=None,
        timeout_secs=10.0,
    ):
        self.callbacks[topic] = callback

    def unsubscribe(self, topic):
        self.callbacks.pop(topic, None)

    def validate_reply_target(self, request):
        self.validated.append(request)

    def reply(self, request, reply):
        reply.set_correlation_id(request.get_header().correlation_id)
        self.published.append((request.get_header().reply_to, reply))

    def reply_confirmed(self, request, reply, timeout_secs=5.0):
        self.confirmed_attempts += 1
        if self.confirmed_failures:
            self.confirmed_failures -= 1
            raise RuntimeError("simulated confirmation failure")
        self.reply(request, reply)

    def deliver(self, topic, message):
        for filter_, callback in list(self.callbacks.items()):
            if MessagingClient.topic_matches_sub(filter_, topic):
                callback(topic, message)


def _request(verb, *, reply_to=REPLY_TO):
    request = MessageBuilder.create(verb, "1.0").with_payload({}).build()
    if reply_to is not None:
        request.make_request(reply_to)
    return request


def _topic(verb):
    return f"ecv1/test-thing/TestComponent/main/cmd/{verb}"


def _inbox():
    messaging = _Messaging()
    inbox = CommandInbox(_Config(), messaging, lambda: 1, lambda: True, lambda: {})
    inbox.start()
    assert set(messaging.callbacks) == {INBOX_FILTER}
    return inbox, messaging


def _wait_until(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.005)
    assert predicate()


@pytest.mark.parametrize(
    ("outcome", "expected"),
    [
        (CommandOutcome.success(), {"ok": True, "result": {}}),
        (CommandOutcome.success({"accepted": True}), {"ok": True, "result": {"accepted": True}}),
        (CommandOutcome.error("CAMERA_BUSY"), {"ok": False, "error": {"code": "CAMERA_BUSY", "message": ""}}),
        (CommandOutcome.error("CAMERA_BUSY", "retry later"), {"ok": False, "error": {"code": "CAMERA_BUSY", "message": "retry later"}}),
    ],
)
def test_immediate_outcomes_use_standard_reply_shapes(outcome, expected):
    inbox, messaging = _inbox()
    try:
        inbox.register_outcome("capture", lambda request: outcome)
        messaging.deliver(_topic("capture"), _request("capture"))
        assert messaging.published[0][1].get_body() == expected
    finally:
        inbox.close()


def test_outcome_registration_is_mutually_exclusive_with_legacy_handlers():
    inbox, _ = _inbox()
    try:
        inbox.register_outcome("capture", lambda request: CommandOutcome.success())
        assert "capture" in inbox.verbs()
        with pytest.raises(ValueError, match="already registered"):
            inbox.register("capture", lambda request: None)
        inbox.unregister("capture")
        inbox.register("capture", lambda request: None)
        with pytest.raises(ValueError, match="already registered"):
            inbox.register_outcome("capture", lambda request: CommandOutcome.success())
        inbox.unregister("capture")
        assert "capture" not in inbox.verbs()

        with pytest.raises(ValueError, match="must not be None"):
            inbox.register_outcome(None, lambda request: CommandOutcome.success())
        with pytest.raises(ValueError, match="must not be None"):
            inbox.register_outcome("capture", None)
    finally:
        inbox.close()


@pytest.mark.parametrize(
    "handler",
    [
        lambda request: ImmediateSuccess("not-a-dict"),
        lambda request: object(),
    ],
)
def test_invalid_outcomes_are_handler_errors(handler):
    inbox, messaging = _inbox()
    try:
        inbox.register_outcome("capture", handler)
        messaging.deliver(_topic("capture"), _request("capture"))
        body = messaging.published[0][1].get_body()
        assert body["ok"] is False
        assert body["error"]["code"] == CommandInbox.ERR_HANDLER_ERROR
        assert "invalid command outcome" in body["error"]["message"]
    finally:
        inbox.close()


@pytest.mark.parametrize(
    ("exception", "code"),
    [
        (CommandException("CAMERA_OFFLINE", "offline"), "CAMERA_OFFLINE"),
        (RuntimeError("driver failed"), CommandInbox.ERR_HANDLER_ERROR),
    ],
)
def test_outcome_handler_exceptions_keep_coded_vs_generic_mapping(exception, code):
    inbox, messaging = _inbox()

    def fail(request):
        raise exception

    try:
        inbox.register_outcome("capture", fail)
        messaging.deliver(_topic("capture"), _request("capture"))
        assert messaging.published[0][1].get_body()["error"]["code"] == code
        messaging.published.clear()
        messaging.deliver(_topic("capture"), _request("capture", reply_to=None))
        assert messaging.published == []
    finally:
        inbox.close()


def test_outcome_value_objects_reject_invalid_construction():
    with pytest.raises(ValueError, match="non-empty"):
        ImmediateError("")
    assert ImmediateError("FAILED").message == ""
    with pytest.raises(ValueError, match="DeferredReply"):
        Deferred("not-a-token")


def test_deferred_success_is_confirmed_once_and_then_terminal():
    inbox, messaging = _inbox()
    issued = []

    def defer_capture(request):
        token = inbox.defer(request, 1)
        assert token.state() is DeferredReplyState.PROVISIONAL
        assert token.activate() is True
        assert token.activate() is False
        issued.append(token)
        return CommandOutcome.deferred(token)

    try:
        inbox.register_outcome("capture", defer_capture)
        request = _request("capture")
        messaging.deliver(_topic("capture"), request)
        assert messaging.published == []
        token = issued[0]
        assert "opaque" in repr(token)
        assert messaging.validated == [request]
        assert token.settle_success({"imageId": "one"}) is SettlementResult.ACCEPTED
        assert token.settle_error("TOO_LATE") is SettlementResult.ALREADY_SETTLED
        _wait_until(lambda: token.state() is DeferredReplyState.SETTLED)
        assert messaging.confirmed_attempts == 1
        assert messaging.published[0][1].get_body() == {
            "ok": True,
            "result": {"imageId": "one"},
        }
        snapshot = inbox.deferred_snapshot()
        assert snapshot.active == 0
        assert snapshot.provisioned == 1
        assert snapshot.settled == 1
    finally:
        inbox.close()


def test_post_accept_continuation_starts_after_open_token_acceptance():
    inbox, messaging = _inbox()
    started = threading.Event()
    issued = []

    def defer_capture(request):
        token = inbox.defer(request, 1)
        assert token.activate() is True
        issued.append(token)

        def continuation():
            started.set()
            assert token.settle_success({"imageId": "post-accept"}) is SettlementResult.ACCEPTED

        return CommandOutcome.deferred_with_continuation(token, continuation)

    try:
        inbox.register_outcome("capture", defer_capture)
        messaging.deliver(_topic("capture"), _request("capture"))

        assert started.wait(1), "the inbox-owned continuation should start"
        _wait_until(lambda: issued[0].state() is DeferredReplyState.SETTLED)
        assert messaging.published[0][1].get_body() == {
            "ok": True,
            "result": {"imageId": "post-accept"},
        }
    finally:
        inbox.close()


def test_invalid_post_accept_token_never_starts_its_continuation():
    inbox, messaging = _inbox()
    started = threading.Event()

    def invalid_defer(request):
        token = inbox.defer(request, 1)
        # Intentionally leave the token PROVISIONAL.
        return CommandOutcome.deferred_with_continuation(token, started.set)

    try:
        inbox.register_outcome("capture", invalid_defer)
        messaging.deliver(_topic("capture"), _request("capture"))

        assert not started.wait(0.05)
        assert messaging.published[0][1].get_body()["error"]["code"] == CommandInbox.ERR_HANDLER_ERROR
    finally:
        inbox.close()


def test_failed_post_accept_continuation_settles_through_guarded_error_path():
    inbox, messaging = _inbox()
    issued = []

    def defer_capture(request):
        token = inbox.defer(request, 1)
        assert token.activate() is True
        issued.append(token)

        def failure():
            raise RuntimeError("simulated camera worker failure")

        return CommandOutcome.deferred_with_continuation(token, failure)

    try:
        inbox.register_outcome("capture", defer_capture)
        messaging.deliver(_topic("capture"), _request("capture"))

        _wait_until(lambda: issued[0].state() is DeferredReplyState.SETTLED)
        assert messaging.published[0][1].get_body()["error"]["code"] == CommandInbox.ERR_HANDLER_ERROR
    finally:
        inbox.close()


def test_deferred_error_retries_confirmation_then_settles():
    inbox, messaging = _inbox()
    messaging.confirmed_failures = 1
    issued = []

    def defer_capture(request):
        token = inbox.defer(request, 1)
        token.activate()
        issued.append(token)
        return CommandOutcome.deferred(token)

    try:
        inbox.register_outcome("capture", defer_capture)
        messaging.deliver(_topic("capture"), _request("capture"))
        token = issued[0]
        with pytest.raises(ValueError, match="non-empty"):
            token.settle_error("")
        assert token.settle_error("CAMERA_FAILED", "sensor") is SettlementResult.ACCEPTED
        _wait_until(lambda: token.state() is DeferredReplyState.SETTLED)
        assert messaging.confirmed_attempts == 2
        assert messaging.published[0][1].get_body() == {
            "ok": False,
            "error": {"code": "CAMERA_FAILED", "message": "sensor"},
        }
    finally:
        inbox.close()


def test_provisional_deferred_token_can_only_be_discarded_once():
    inbox, _ = _inbox()
    token = inbox.defer(_request("capture"), 1)
    try:
        assert token.settle_success() is SettlementResult.NOT_OPEN
        assert token.discard() is True
        assert token.discard() is False
        assert token.activate() is False
        assert token.state() is DeferredReplyState.DISCARDED
        snapshot = inbox.deferred_reply_snapshot()
        assert snapshot.active == 0
        assert snapshot.discarded == 1
    finally:
        inbox.close()


def test_unactivated_deferred_outcome_is_rejected_and_frees_capacity():
    inbox, messaging = _inbox()
    issued = []

    def invalid_defer(request):
        token = inbox.defer(request, 1)
        issued.append(token)
        return CommandOutcome.deferred(token)

    try:
        inbox.register_outcome("capture", invalid_defer)
        messaging.deliver(_topic("capture"), _request("capture"))
        assert issued[0].state() is DeferredReplyState.DISCARDED
        body = messaging.published[0][1].get_body()
        assert body["error"]["code"] == CommandInbox.ERR_HANDLER_ERROR
        assert inbox.deferred_reply_snapshot().active == 0
    finally:
        inbox.close()


def test_deferred_open_and_provisional_tokens_expire_without_threads_per_token():
    inbox, _ = _inbox()
    provisional = inbox.defer(_request("capture"), 0.03)
    opened = inbox.defer(_request("capture"), 0.03)
    assert opened.activate() is True
    try:
        _wait_until(
            lambda: provisional.state() is DeferredReplyState.EXPIRED
            and opened.state() is DeferredReplyState.EXPIRED
        )
        assert provisional.settle_success() is SettlementResult.EXPIRED
        assert opened.settle_success() is SettlementResult.EXPIRED
        snapshot = inbox.deferred_reply_snapshot()
        assert snapshot.expired == 2
        assert snapshot.open_expired == 1
        assert snapshot.active == 0
    finally:
        inbox.close()


def test_defer_validates_request_lifetime_shutdown_and_capacity(monkeypatch):
    inbox, _ = _inbox()
    notification = _request("capture", reply_to=None)
    try:
        with pytest.raises(CommandException, match="requires a request"):
            inbox.defer(None, 1)
        with pytest.raises(CommandException, match="request/reply"):
            inbox.defer(notification, 1)
        for value in (True, "1"):
            with pytest.raises(TypeError, match="number"):
                inbox.defer(_request("capture"), value)
        for value in (0, -1, math.inf, math.nan):
            with pytest.raises(ValueError, match="finite and positive"):
                inbox.defer(_request("capture"), value)
        with pytest.raises(ValueError, match="31-minute"):
            inbox.defer(
                _request("capture"),
                CommandInbox.MAX_DEFERRED_REPLY_LIFETIME_SECS + 1,
            )

        monkeypatch.setattr(command_module, "MAX_DEFERRED_REPLIES", 1)
        first = inbox.defer(_request("capture"), 1)
        with pytest.raises(CommandException) as rejected:
            inbox.defer(_request("capture"), 1)
        assert rejected.value.code == CommandInbox.ERR_DEFERRED_REPLY_CAPACITY
        assert inbox.deferred_reply_snapshot().capacity_rejected == 1
        assert first.discard() is True
    finally:
        inbox.close()

    with pytest.raises(CommandException) as stopping:
        inbox.defer(_request("capture"), 1)
    assert stopping.value.code == CommandInbox.ERR_COMPONENT_STOPPING


def test_close_cancels_provisional_and_sends_one_stopping_reply_for_open_token():
    inbox, messaging = _inbox()
    provisional = inbox.defer(_request("provisional"), 1)
    opened = inbox.defer(_request("open"), 1)
    opened.activate()

    inbox.close()

    assert provisional.state() is DeferredReplyState.CANCELLED_ON_SHUTDOWN
    assert opened.state() is DeferredReplyState.CANCELLED_ON_SHUTDOWN
    assert provisional.settle_success() is SettlementResult.CANCELLED_ON_SHUTDOWN
    assert opened.settle_success() is SettlementResult.CANCELLED_ON_SHUTDOWN
    assert messaging.published[0][1].get_body()["error"]["code"] == (
        CommandInbox.ERR_COMPONENT_STOPPING
    )
    snapshot = inbox.deferred_reply_snapshot()
    assert snapshot.cancelled_on_shutdown == 2
    assert snapshot.active == 0
