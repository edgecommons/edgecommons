"""Adversarial unit coverage for the camera-design P1 messaging primitives."""

import threading
import time
from types import SimpleNamespace
from unittest.mock import MagicMock

import paho.mqtt.client as mqtt
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
from edgecommons.facades.app_facade import AppFacade, PreparedAppMessage
from edgecommons.facades.channel import Channel
from edgecommons.messaging.errors import (
    PublishConfirmationError,
    PublishConfirmationReason,
    ReservedTopicError,
)
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.messaging.messaging_provider import MessagingProvider
from edgecommons.messaging.providers.greengrass.greengrass_ipc import (
    GreengrassIpcProvider,
)
from edgecommons.messaging.providers.standalone_provider import (
    StandaloneProvider,
    _BrokerChannel,
)
from edgecommons.messaging.qos import Qos
from edgecommons.uns import Uns


class _Config:
    def __init__(self):
        self.identity = MessageIdentity(
            [HierEntry("device", "cam-host")], "camera-adapter", "main"
        )

    def get_component_identity(self):
        return self.identity

    def is_topic_include_root(self):
        return False

    def get_tag_config(self):
        return None


class _Messaging:
    def __init__(self, confirmed_failures=0):
        self.callbacks = {}
        self.immediate = []
        self.confirmed = []
        self.confirmed_failures = confirmed_failures
        self.local = []
        self.northbound = []

    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None):
        self.callbacks[topic] = callback

    def subscribe_acknowledged(
        self,
        topic,
        callback,
        max_concurrency=None,
        max_messages=None,
        timeout_secs=10.0,
    ):
        self.subscribe(topic, callback, max_concurrency, max_messages)

    def unsubscribe(self, topic):
        self.callbacks.pop(topic, None)

    def deliver(self, topic, message):
        for filter_, callback in list(self.callbacks.items()):
            if MessagingClient.topic_matches_sub(filter_, topic):
                callback(topic, message)

    @staticmethod
    def validate_reply_target(request):
        header = None if request is None else request.get_header()
        if header is None or not header.reply_to:
            raise ValueError("missing reply target")
        return header.reply_to

    def reply(self, request, reply):
        reply.set_correlation_id(request.get_correlation_id())
        self.immediate.append(reply)

    def reply_confirmed(self, request, reply, timeout_secs):
        reply.set_correlation_id(request.get_correlation_id())
        self.confirmed.append((reply, reply.to_bytes(), timeout_secs))
        if len(self.confirmed) <= self.confirmed_failures:
            raise PublishConfirmationError(
                PublishConfirmationReason.TIMEOUT, "ambiguous timeout"
            )

    def publish(self, topic, message):
        self.local.append((topic, message))

    def publish_northbound(self, topic, message, qos):
        self.northbound.append((topic, message, qos))

    def publish_confirmed(self, topic, encoded, qos, timeout_secs):
        self.confirmed.append((topic, encoded, qos, timeout_secs))

    def publish_northbound_confirmed(self, topic, encoded, qos, timeout_secs):
        self.confirmed.append((topic, encoded, qos, timeout_secs))


def _request(verb="sb/capture", reply_to="edgecommons/reply-camera"):
    request = MessageBuilder.create(verb, "1.0").with_payload({}).build()
    if reply_to is not None:
        request.make_request(reply_to)
    return request


def _topic(verb="sb/capture"):
    return f"ecv1/cam-host/camera-adapter/main/cmd/{verb}"


def _inbox(messaging=None):
    messaging = messaging or _Messaging()
    inbox = CommandInbox(_Config(), messaging, lambda: 1, lambda: True, lambda: {})
    return inbox, messaging


def _wait_until(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.005)
    assert predicate(), "condition did not become true before timeout"


def test_outcome_handlers_are_additive_and_keep_standard_wrappers():
    inbox, messaging = _inbox()
    inbox.register("legacy", lambda _request: {"legacy": True})
    inbox.register_outcome("ok", lambda _request: ImmediateSuccess({"v": 1}))
    inbox.register_outcome("bad", lambda _request: ImmediateError("NO_CAMERA", "gone"))
    inbox.start()

    messaging.deliver(_topic("legacy"), _request("legacy"))
    messaging.deliver(_topic("ok"), _request("ok"))
    messaging.deliver(_topic("bad"), _request("bad"))

    assert messaging.immediate[0].get_body() == {
        "ok": True,
        "result": {"legacy": True},
    }
    assert messaging.immediate[1].get_body() == {"ok": True, "result": {"v": 1}}
    assert messaging.immediate[2].get_body()["error"] == {
        "code": "NO_CAMERA",
        "message": "gone",
    }
    assert {"legacy", "ok", "bad"}.issubset(inbox.verbs())
    inbox.close()


def test_deferred_reply_retries_same_uuid_and_exact_bytes_until_confirmed():
    inbox, messaging = _inbox(_Messaging(confirmed_failures=2))
    captured = {}

    def handler(request):
        token = inbox.defer(request, 1.0)
        captured["token"] = token
        assert token.activate()
        return Deferred(token)

    inbox.register_outcome("sb/capture", handler)
    inbox.start()
    request = _request()
    messaging.deliver(_topic(), request)
    assert not messaging.immediate, "Deferred must suppress the automatic reply"

    assert captured["token"].settle_success({"captureId": "cap-1"}) is SettlementResult.ACCEPTED
    _wait_until(lambda: inbox.deferred_snapshot().settled == 1)

    attempts = messaging.confirmed
    assert len(attempts) == 3
    assert len({attempt[0].get_header().uuid for attempt in attempts}) == 1
    assert len({attempt[1] for attempt in attempts}) == 1
    assert attempts[-1][0].get_header().correlation_id == request.get_correlation_id()
    assert captured["token"].state() is DeferredReplyState.SETTLED
    assert inbox.deferred_snapshot().active == 0
    inbox.close()


def test_only_one_concurrent_settler_can_win():
    inbox, messaging = _inbox()
    token = inbox.defer(_request(), 1.0)
    assert token.activate()
    barrier = threading.Barrier(17)
    results = []

    def settle():
        barrier.wait()
        results.append(token.settle_success({"winner": threading.get_ident()}))

    threads = [threading.Thread(target=settle) for _ in range(16)]
    for thread in threads:
        thread.start()
    barrier.wait()
    for thread in threads:
        thread.join()

    assert results.count(SettlementResult.ACCEPTED) == 1
    assert results.count(SettlementResult.ALREADY_SETTLED) == 15
    _wait_until(lambda: token.state() is DeferredReplyState.SETTLED)
    assert len(messaging.confirmed) == 1
    inbox.close()


def test_provisional_discard_missing_reply_expiration_and_capacity(monkeypatch):
    inbox, _ = _inbox()
    provisional = inbox.defer(_request(), 1.0)
    assert provisional.discard()
    assert provisional.state() is DeferredReplyState.DISCARDED
    assert not provisional.activate()

    with pytest.raises(CommandException) as missing:
        inbox.defer(_request(reply_to=None), 1.0)
    assert missing.value.code == CommandInbox.ERR_REPLY_REQUIRED

    expiring = inbox.defer(_request(), 0.03)
    assert expiring.activate()
    _wait_until(lambda: expiring.state() is DeferredReplyState.EXPIRED)
    assert expiring.settle_error("LATE") is SettlementResult.EXPIRED
    assert inbox.deferred_snapshot().open_expired == 1

    monkeypatch.setattr(command_module, "MAX_DEFERRED_REPLIES", 1)
    held = inbox.defer(_request(), 1.0)
    with pytest.raises(CommandException) as full:
        inbox.defer(_request(), 1.0)
    assert full.value.code == CommandInbox.ERR_DEFERRED_REPLY_CAPACITY
    assert inbox.deferred_snapshot().capacity_rejected == 1
    held.discard()
    inbox.close()


def test_unactivated_or_wrong_request_token_is_rejected_as_handler_error():
    inbox, messaging = _inbox()
    original = _request()
    token = inbox.defer(original, 1.0)
    inbox.register_outcome("sb/capture", lambda _request: Deferred(token))
    inbox.start()

    messaging.deliver(_topic(), original)
    assert token.state() is DeferredReplyState.DISCARDED
    assert messaging.immediate[-1].get_body()["error"]["code"] == "HANDLER_ERROR"
    inbox.close()


def test_close_attempts_component_stopping_for_open_tokens():
    inbox, messaging = _inbox()
    token = inbox.defer(_request(), 1.0)
    assert token.activate()

    inbox.close()

    assert token.state() is DeferredReplyState.CANCELLED_ON_SHUTDOWN
    assert inbox.deferred_snapshot().active == 0
    assert inbox.deferred_snapshot().cancelled_on_shutdown == 1
    assert len(messaging.confirmed) == 1
    body = messaging.confirmed[0][0].get_body()
    assert body["error"]["code"] == CommandInbox.ERR_COMPONENT_STOPPING


def test_deferred_registry_uses_one_timer_and_bounded_publish_workers():
    inbox, _ = _inbox()
    tokens = [inbox.defer(_request(), 1.0) for _ in range(64)]

    timer = inbox._deferred_timer_thread
    assert timer is not None and timer.is_alive()
    assert sum(
        thread.name == "edgecommons-deferred-reply-timer"
        for thread in threading.enumerate()
    ) == 1
    assert inbox._deferred_publishers._max_workers == 32

    for token in tokens:
        assert token.discard()
    inbox.close()
    assert not timer.is_alive()


def test_deferred_validation_factories_and_failure_edges(monkeypatch):
    inbox, messaging = _inbox()
    assert CommandOutcome.success().result is None
    assert CommandOutcome.error("X").message == ""
    with pytest.raises(ValueError):
        ImmediateError("")
    with pytest.raises(ValueError):
        Deferred(object())

    with pytest.raises(CommandException):
        inbox.defer(None, 1.0)
    for lifetime in (True, "1"):
        with pytest.raises(TypeError):
            inbox.defer(_request(), lifetime)
    for lifetime in (0, -1, float("inf"), float("nan"), 1860.1):
        with pytest.raises(ValueError):
            inbox.defer(_request(), lifetime)

    no_name = _request()
    no_name.get_header().name = ""
    with pytest.raises(ValueError):
        inbox.defer(no_name, 1.0)
    no_correlation = _request()
    no_correlation.get_header().correlation_id = ""
    with pytest.raises(ValueError):
        inbox.defer(no_correlation, 1.0)

    provisional = inbox.defer(_request(), 0.03)
    assert provisional.settle_success() is SettlementResult.NOT_OPEN
    assert "opaque" in repr(provisional)
    _wait_until(lambda: provisional.state() is DeferredReplyState.EXPIRED)
    assert inbox.deferred_reply_snapshot().expired == 1
    assert inbox.deferred_snapshot() == inbox.deferred_reply_snapshot()

    token = inbox.defer(_request(), 1.0)
    assert token.activate()
    original_builder = inbox._build_deferred_reply
    monkeypatch.setattr(
        inbox,
        "_build_deferred_reply",
        lambda _entry, _body: (_ for _ in ()).throw(RuntimeError("config gone")),
    )
    assert token.settle_success() is SettlementResult.NOT_OPEN
    monkeypatch.setattr(inbox, "_build_deferred_reply", original_builder)
    assert token.settle_error("FAILED", "x") is SettlementResult.ACCEPTED
    _wait_until(lambda: token.state() is DeferredReplyState.SETTLED)

    inbox.close()
    with pytest.raises(CommandException) as stopping:
        inbox.defer(_request(), 1.0)
    assert stopping.value.code == CommandInbox.ERR_COMPONENT_STOPPING

    # Provisioning must roll back capacity if the one shared timer cannot start.
    failed, _ = _inbox()
    original_start = threading.Thread.start

    def fail_timer_start(thread):
        if thread.name == "edgecommons-deferred-reply-timer":
            raise RuntimeError("no thread")
        return original_start(thread)

    monkeypatch.setattr(threading.Thread, "start", fail_timer_start)
    with pytest.raises(RuntimeError, match="no thread"):
        failed.defer(_request(), 1.0)
    assert failed.deferred_reply_snapshot().active == 0
    failed.close()


def test_outcome_handler_failure_and_invalid_result_paths():
    inbox, messaging = _inbox()
    inbox.register_outcome("bad-result", lambda _request: ImmediateSuccess([]))
    inbox.register_outcome("none", lambda _request: None)
    inbox.register_outcome(
        "coded",
        lambda _request: (_ for _ in ()).throw(CommandException("CODED", "known")),
    )
    inbox.register_outcome(
        "boom", lambda _request: (_ for _ in ()).throw(RuntimeError("boom"))
    )
    inbox.register_outcome("notify-error", lambda _request: ImmediateError("NOPE"))
    inbox.start()

    for verb in ("bad-result", "none", "coded", "boom"):
        messaging.deliver(_topic(verb), _request(verb))
    messaging.deliver(_topic("notify-error"), _request("notify-error", reply_to=None))

    codes = [reply.get_body()["error"]["code"] for reply in messaging.immediate]
    assert codes == ["HANDLER_ERROR", "HANDLER_ERROR", "CODED", "HANDLER_ERROR"]
    assert len(messaging.immediate) == 4
    inbox.close()


def _app_facade(messaging=None):
    config = _Config()
    messaging = messaging or _Messaging()
    return AppFacade(config, "main", Uns(config.identity, False), messaging), messaging


def test_prepared_app_message_captures_exact_bytes_and_correlation():
    facade, messaging = _app_facade()
    request = _request()
    prepared = facade.prepare_correlated(
        "ImageCaptured", "image/captured", {"captureId": "cap-1"}, request
    )

    assert isinstance(prepared, PreparedAppMessage)
    assert prepared.message.to_bytes() == prepared.encoded_bytes
    assert prepared.message.get_correlation_id() == request.get_correlation_id()
    uuid = prepared.message.get_header().uuid
    mutable_view = prepared.message
    mutable_view.set_correlation_id("attempted-mutation")
    assert prepared.message.get_correlation_id() == request.get_correlation_id()
    facade.publish_confirmed(prepared, 2.0)
    facade.publish_confirmed(prepared, 2.0)

    assert [call[1] for call in messaging.confirmed] == [
        prepared.encoded_bytes,
        prepared.encoded_bytes,
    ]
    assert prepared.message.get_header().uuid == uuid
    facade.publish("Plain", "plain", {"x": 1})
    assert messaging.local[-1][1].get_body() == {"x": 1}


def test_confirmed_app_northbound_propagates_and_rejects_stream():
    facade, messaging = _app_facade()
    prepared = facade.prepare("ImageCaptured", "image/captured", {})
    facade.publish_confirmed(prepared, 1.0, Channel.NORTHBOUND)
    assert messaging.confirmed[-1][2] is Qos.AT_LEAST_ONCE
    with pytest.raises(ValueError):
        facade.publish_confirmed(prepared, 1.0, Channel.stream("images"))
    with pytest.raises(ValueError):
        facade.prepare_correlated("X", "x", {}, Message())


def test_prepared_app_value_and_correlation_validation_edges():
    message = MessageBuilder.create("X", "1.0").with_payload({}).build()
    with pytest.raises(ValueError):
        PreparedAppMessage("", message, b"x")
    with pytest.raises(ValueError):
        PreparedAppMessage("topic", None, b"x")
    with pytest.raises(TypeError):
        PreparedAppMessage("topic", message, bytearray(b"x"))
    with pytest.raises(ValueError):
        PreparedAppMessage("topic", message, b"not-the-message")

    facade, _ = _app_facade()
    explicit = facade.prepare_correlated("X", "x", {}, "correlation-1")
    assert explicit.message.get_correlation_id() == "correlation-1"
    with pytest.raises(ValueError):
        facade.prepare_correlated("X", "x", {}, object())
    with pytest.raises(ValueError):
        facade.publish_prepared(None)
    with pytest.raises(ValueError):
        facade.publish_confirmed(None, 1.0)


def test_messaging_client_confirmed_paths_delegate_exact_bytes_and_guard_replies():
    provider = MagicMock()
    MessagingClient._messaging_provider = provider
    MessagingClient._guard_include_root = False
    message = MessageBuilder.create("x", "1.0").with_payload({}).build()
    encoded = message.to_bytes()

    MessagingClient.publish_confirmed("app/topic", encoded, Qos.AT_LEAST_ONCE, 2.0)
    provider.publish_confirmed.assert_called_once_with(
        "app/topic", encoded, Qos.AT_LEAST_ONCE, 2.0
    )
    MessagingClient.publish_northbound_confirmed(
        "app/north", message, Qos.AT_LEAST_ONCE, 3.0
    )
    provider.publish_northbound_confirmed.assert_called_once_with(
        "app/north", encoded, Qos.AT_LEAST_ONCE, 3.0
    )
    callback = lambda topic, message: None
    MessagingClient.subscribe_acknowledged("cmd/#", callback, 1, 256, 4.0)
    provider.subscribe_acknowledged.assert_called_once_with(
        "cmd/#", callback, 1, 256, 4.0
    )
    request = _request()
    reply = MessageBuilder.create("x", "1.0").with_payload({}).build()
    MessagingClient.reply_confirmed(request, reply, 1.0)
    assert reply.get_correlation_id() == request.get_correlation_id()

    with pytest.raises(ReservedTopicError):
        MessagingClient.publish_confirmed(
            "ecv1/d/c/i/state", encoded, Qos.AT_LEAST_ONCE, 1.0
        )
    hostile = _request(reply_to="ecv1/d/c/i/log")
    with pytest.raises(ReservedTopicError):
        MessagingClient.validate_reply_target(hostile)
    with pytest.raises(ValueError):
        MessagingClient.validate_reply_target(_request(reply_to=None))
    with pytest.raises(TypeError):
        MessagingClient.publish_confirmed(
            "app/topic", object(), Qos.AT_LEAST_ONCE, 1.0
        )
    with pytest.raises(TypeError):
        MessagingClient.publish_northbound_confirmed(
            "app/topic", object(), Qos.AT_LEAST_ONCE, 1.0
        )
    local_calls = provider.publish_confirmed.call_count
    north_calls = provider.publish_northbound_confirmed.call_count
    with pytest.raises(ValueError, match="valid EdgeCommons envelope"):
        MessagingClient.publish_confirmed(
            "app/topic", b"not-an-edgecommons-envelope", Qos.AT_LEAST_ONCE, 1.0
        )
    with pytest.raises(ValueError, match="valid EdgeCommons envelope"):
        MessagingClient.publish_northbound_confirmed(
            "app/topic", b"\xff\xff", Qos.AT_LEAST_ONCE, 1.0
        )
    assert provider.publish_confirmed.call_count == local_calls
    assert provider.publish_northbound_confirmed.call_count == north_calls
    MessagingClient._messaging_provider = None


def test_provider_confirmation_contract_rejects_degradation_and_bad_inputs():
    with pytest.raises(NotImplementedError):
        MessagingProvider.publish_confirmed(
            SimpleNamespace(), "t", b"x", Qos.AT_LEAST_ONCE, 1.0
        )
    assert MessagingProvider._validated_confirmation_timeout(
        b"x", Qos.AT_LEAST_ONCE, 1
    ) == 1.0
    with pytest.raises(ValueError):
        MessagingProvider._validated_confirmation_timeout(
            b"x", Qos.AT_MOST_ONCE, 1
        )
    with pytest.raises(ValueError):
        MessagingProvider._validated_confirmation_timeout(
            b"x", Qos.AT_LEAST_ONCE, 0
        )
    with pytest.raises(TypeError):
        MessagingProvider._validated_confirmation_timeout(
            bytearray(b"x"), Qos.AT_LEAST_ONCE, 1
        )
    with pytest.raises(TypeError):
        MessagingProvider._validated_confirmation_timeout(
            b"x", Qos.AT_LEAST_ONCE, True
        )
    with pytest.raises(ValueError):
        MessagingProvider._validated_confirmation_timeout(
            b"x", Qos.AT_LEAST_ONCE, float("inf")
        )
    with pytest.raises(NotImplementedError, match="acknowledged local subscribe"):
        MessagingProvider.subscribe_acknowledged(
            SimpleNamespace(), "t", lambda topic, message: None, timeout_secs=1
        )
    assert MessagingProvider._validated_subscribe_timeout(1) == 1.0
    with pytest.raises(ValueError):
        MessagingProvider._validated_subscribe_timeout(0)
    with pytest.raises(TypeError):
        MessagingProvider._validated_subscribe_timeout(True)


class _PublishInfo:
    def __init__(self, published=True, rc=mqtt.MQTT_ERR_SUCCESS):
        self.rc = rc
        self.published = published
        self.waited = None

    def wait_for_publish(self, timeout=None):
        self.waited = timeout

    def is_published(self):
        return self.published


def _standalone_with(client):
    provider = object.__new__(StandaloneProvider)
    provider._local = _BrokerChannel("local")
    provider._local.client = client
    provider._confirmed_publish_permits = threading.BoundedSemaphore(1024)
    return provider


def test_standalone_confirmation_requires_positive_puback_and_exact_qos1():
    info = _PublishInfo()
    client = MagicMock()
    client.publish.return_value = info
    provider = _standalone_with(client)

    provider.publish_confirmed("camera/out", b"exact", Qos.AT_LEAST_ONCE, 1.0)

    client.publish.assert_called_once_with("camera/out", b"exact", qos=1)
    assert info.waited is not None
    timeout_info = _PublishInfo(published=False)
    client.publish.return_value = timeout_info
    with pytest.raises(PublishConfirmationError) as error:
        provider.publish_confirmed("camera/out", b"exact", Qos.AT_LEAST_ONCE, 0.01)
    assert error.value.reason is PublishConfirmationReason.TIMEOUT

    disconnected = _standalone_with(None)
    with pytest.raises(PublishConfirmationError) as error:
        disconnected.publish_confirmed(
            "camera/out", b"exact", Qos.AT_LEAST_ONCE, 0.01
        )
    assert error.value.reason is PublishConfirmationReason.TRANSPORT_ERROR

    rejected = _PublishInfo(rc=mqtt.MQTT_ERR_NO_CONN)
    client.publish.return_value = rejected
    with pytest.raises(PublishConfirmationError) as error:
        provider.publish_confirmed("camera/out", b"exact", Qos.AT_LEAST_ONCE, 0.1)
    assert error.value.reason is PublishConfirmationReason.TRANSPORT_ERROR


class _Operation:
    def __init__(self, error=None):
        self.error = error
        self.timeout = None
        self.cancelled = False

    def result(self, timeout=None):
        self.timeout = timeout
        if self.error is not None:
            raise self.error
        return object()

    def cancel(self):
        self.cancelled = True


def _greengrass_with(client):
    provider = object.__new__(GreengrassIpcProvider)
    provider._ipc_client = client
    provider._confirmed_publish_permits = threading.BoundedSemaphore(1024)
    return provider


def test_greengrass_confirmation_waits_for_operation_completion_and_cancels_timeout():
    local_operation = _Operation()
    north_operation = _Operation()
    client = MagicMock()
    client.publish_to_topic_async.return_value = local_operation
    client.publish_to_iot_core_async.return_value = north_operation
    provider = _greengrass_with(client)

    provider.publish_confirmed("local", b"exact", Qos.AT_LEAST_ONCE, 1.0)
    provider.publish_northbound_confirmed(
        "north", b"exact", Qos.AT_LEAST_ONCE, 1.0
    )
    assert local_operation.timeout is not None
    assert north_operation.timeout is not None

    timed_out = _Operation(TimeoutError("late"))
    client.publish_to_topic_async.return_value = timed_out
    with pytest.raises(PublishConfirmationError) as error:
        provider.publish_confirmed("local", b"exact", Qos.AT_LEAST_ONCE, 0.1)
    assert error.value.reason is PublishConfirmationReason.TIMEOUT
    assert timed_out.cancelled

    failed = _Operation(RuntimeError("nucleus down"))
    client.publish_to_topic_async.return_value = failed
    with pytest.raises(PublishConfirmationError) as error:
        provider.publish_confirmed("local", b"exact", Qos.AT_LEAST_ONCE, 0.1)
    assert error.value.reason is PublishConfirmationReason.TRANSPORT_ERROR

    provider._ipc_client = None
    with pytest.raises(PublishConfirmationError) as error:
        provider.publish_northbound_confirmed(
            "north", b"exact", Qos.AT_LEAST_ONCE, 0.1
        )
    assert error.value.reason is PublishConfirmationReason.TRANSPORT_ERROR


def _greengrass_subscriber(client):
    provider = object.__new__(GreengrassIpcProvider)
    provider._ipc_client = client
    provider._receive_mode = "RECEIVE_MESSAGES_FROM_OTHERS"
    provider._ipc_subscription_handlers = {}
    provider._ipc_subscription_operations = {}
    return provider


def test_greengrass_acknowledged_subscribe_waits_and_cleans_failed_operation():
    future = MagicMock()
    operation = MagicMock()
    client = MagicMock()
    client.subscribe_to_topic_async.return_value = (future, operation)
    provider = _greengrass_subscriber(client)

    provider.subscribe_acknowledged("camera/cmd/#", lambda topic, message: None, timeout_secs=1.5)
    future.result.assert_called_once_with(timeout=1.5)
    assert provider._ipc_subscription_operations["camera/cmd/#"] is operation
    provider.unsubscribe("camera/cmd/#")
    operation.close.assert_called_once()

    failed_future = MagicMock()
    failed_future.result.side_effect = TimeoutError("late")
    failed_operation = MagicMock()
    client.subscribe_to_topic_async.return_value = (failed_future, failed_operation)
    with pytest.raises(RuntimeError, match="not acknowledged"):
        provider.subscribe_acknowledged(
            "camera/cmd/fail", lambda topic, message: None, timeout_secs=0.01
        )
    failed_operation.close.assert_called_once()
    assert "camera/cmd/fail" not in provider._ipc_subscription_operations
