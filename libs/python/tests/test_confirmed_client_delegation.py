"""The ``MessagingClient`` strict-confirmation facade: what it validates, what it
forwards, and what it must never do.

The Python mirror of the Java canonical's ``MessagingClientDelegationTest`` confirmation
cases. The contracts pinned here:

* a confirmed publish **encodes once** and hands the provider the *exact* bytes -- it
  never re-serializes, and never degrades to the best-effort ``publish()``;
* exact outbox bytes are **parsed through the canonical codec first**, so a corrupt
  record is refused *before* any transport I/O rather than being blasted at a broker;
* the reserved-class UNS guard applies to the confirmed path too;
* a confirmed reply needs a **real reply target** and a **non-null reply**, and inherits
  the request's correlation id before the envelope is frozen;
* an acknowledged subscribe never falls back to the unacknowledged one.

A recording provider is injected into the static client, so no broker is opened. The
client is process-global state, so the fixture restores it (both the provider *and* the
late-bound guard flag) after every test.
"""
import pytest

from edgecommons.messaging.errors import ReservedTopicError
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.messaging.messaging_provider import MessagingProvider
from edgecommons.messaging.qos import Qos

RESERVED_TOPIC = "ecv1/gw-01/adapter/main/state"


class _RecordingProvider(MessagingProvider):
    """Records every call so the tests can assert exactly which seam was used."""

    def __init__(self):
        super().__init__()
        self.confirmed = []          # (topic, bytes, qos, timeout)
        self.confirmed_nb = []       # (topic, bytes, qos, timeout)
        self.best_effort = []        # (topic, msg)
        self.best_effort_nb = []     # (topic, msg, qos)
        self.replies = []            # (request, reply)
        self.subscribes = []         # (topic, cb, max_concurrency, max_messages)
        self.ack_subscribes = []     # (topic, cb, max_concurrency, max_messages, timeout)

    def disconnect(self):
        pass

    def connected(self) -> bool:
        return True

    def publish(self, topic, msg):
        self.best_effort.append((topic, msg))

    def publish_raw(self, topic, msg):
        self.best_effort.append((topic, msg))

    def publish_northbound(self, topic, msg, qos):
        self.best_effort_nb.append((topic, msg, qos))

    def publish_northbound_raw(self, topic, msg, qos):
        self.best_effort_nb.append((topic, msg, qos))

    def publish_confirmed(self, topic, encoded_message, qos, timeout_secs):
        self.confirmed.append((topic, encoded_message, qos, timeout_secs))

    def publish_northbound_confirmed(self, topic, encoded_message, qos, timeout_secs):
        self.confirmed_nb.append((topic, encoded_message, qos, timeout_secs))

    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None):
        self.subscribes.append((topic, callback, max_concurrency, max_messages))

    def subscribe_acknowledged(self, topic, callback, max_concurrency=None,
                               max_messages=None, timeout_secs=10.0):
        self.ack_subscribes.append(
            (topic, callback, max_concurrency, max_messages, timeout_secs)
        )

    def subscribe_northbound(self, topic, callback, qos, max_concurrency=None,
                             max_messages=None):
        pass

    def unsubscribe(self, topic):
        pass

    def unsubscribe_northbound(self, topic):
        pass

    def request(self, topic, msg, timeout_secs=None):
        return None

    def request_northbound(self, topic, msg, timeout_secs=None):
        return None

    def reply(self, request_msg, response_msg):
        self.replies.append((request_msg, response_msg))

    def reply_northbound(self, request_msg, response_msg):
        self.replies.append((request_msg, response_msg))

    def cancel_request(self, iou):
        pass

    def cancel_request_northbound(self, iou):
        pass

    def get_native_client(self):
        return None

    def nothing_happened(self) -> bool:
        return not (self.confirmed or self.confirmed_nb or self.best_effort
                    or self.best_effort_nb or self.replies)


@pytest.fixture
def provider():
    """Injects a recording provider into the process-global client and restores it."""
    previous = MessagingClient._messaging_provider
    previous_guard = MessagingClient._guard_include_root
    recording = _RecordingProvider()
    MessagingClient._messaging_provider = recording
    yield recording
    MessagingClient._messaging_provider = previous
    MessagingClient._guard_include_root = previous_guard


def _envelope(name="Confirmed", body=None):
    return MessageBuilder.create(name, "1.0").with_payload(body or {"k": "v"}).build()


def _request(reply_to="reply/ok"):
    request = _envelope("Req")
    request.make_request(reply_to)
    return request


class TestConfirmedPublishEncodesOnceAndDelegates:
    def test_a_message_is_encoded_once_and_the_exact_bytes_are_forwarded(self, provider):
        msg = _envelope()
        expected = msg.to_bytes()

        MessagingClient.publish_confirmed("t/confirmed", msg, Qos.AT_LEAST_ONCE, 4.0)

        topic, encoded, qos, timeout = provider.confirmed[0]
        assert topic == "t/confirmed"
        assert encoded == expected, "the provider must receive the message's own bytes"
        assert qos is Qos.AT_LEAST_ONCE
        assert timeout == 4.0
        assert not provider.best_effort, (
            "a confirmed publish must never degrade to the best-effort publish()"
        )

    def test_exact_outbox_bytes_are_forwarded_byte_for_byte(self, provider):
        # The outbox retry path: the SAME bytes (same envelope UUID) must reach the wire,
        # not a logically-equivalent re-encoding with a fresh UUID.
        exact = _envelope().to_bytes()

        MessagingClient.publish_confirmed("t/confirmed", exact, Qos.AT_LEAST_ONCE, 0.25)

        _, encoded, _, _ = provider.confirmed[0]
        assert encoded == exact
        assert Message.from_bytes(encoded).get_header().uuid == \
            Message.from_bytes(exact).get_header().uuid

    def test_northbound_confirmed_publish_forwards_exact_bytes_and_skips_best_effort(
        self, provider
    ):
        msg = _envelope()
        MessagingClient.publish_northbound_confirmed(
            "t/nb", msg, Qos.AT_LEAST_ONCE, 6.0
        )

        topic, encoded, qos, timeout = provider.confirmed_nb[0]
        assert (topic, encoded, qos, timeout) == ("t/nb", msg.to_bytes(),
                                                  Qos.AT_LEAST_ONCE, 6.0)
        assert not provider.best_effort_nb

    def test_the_requested_qos_is_passed_through_for_the_provider_to_enforce(
        self, provider
    ):
        # The client does not silently rewrite a caller's QoS to 1 -- it forwards what it
        # was given so the provider's own guard is the single place that rejects it.
        msg = _envelope()
        MessagingClient.publish_confirmed("t/c", msg, Qos.AT_MOST_ONCE, 1.0)
        assert provider.confirmed[0][2] is Qos.AT_MOST_ONCE


class TestConfirmedPublishRejectsBadInputBeforeAnyTransportIo:
    def test_a_malformed_envelope_is_refused_before_the_provider_is_touched(
        self, provider
    ):
        with pytest.raises(ValueError, match="valid EdgeCommons envelope"):
            MessagingClient.publish_confirmed(
                "t/c", b"\x09\x08\x07", Qos.AT_LEAST_ONCE, 1.0
            )
        assert provider.nothing_happened()

    def test_a_malformed_envelope_is_refused_on_the_northbound_path_too(self, provider):
        with pytest.raises(ValueError, match="valid EdgeCommons envelope"):
            MessagingClient.publish_northbound_confirmed(
                "t/c", b"not-an-envelope", Qos.AT_LEAST_ONCE, 1.0
            )
        assert provider.nothing_happened()

    @pytest.mark.parametrize("bad", ["a string", {"header": {}}, None, 42])
    def test_a_body_that_is_neither_a_message_nor_bytes_is_refused(self, bad, provider):
        with pytest.raises(TypeError, match="Message or exact bytes"):
            MessagingClient.publish_confirmed("t/c", bad, Qos.AT_LEAST_ONCE, 1.0)
        with pytest.raises(TypeError, match="Message or exact bytes"):
            MessagingClient.publish_northbound_confirmed(
                "t/c", bad, Qos.AT_LEAST_ONCE, 1.0
            )
        assert provider.nothing_happened()

    def test_a_reserved_class_topic_is_refused_on_the_confirmed_path(self, provider):
        # The library owns state/metric/cfg/log; strictness must not buy a way around it.
        with pytest.raises(ReservedTopicError):
            MessagingClient.publish_confirmed(
                RESERVED_TOPIC, _envelope(), Qos.AT_LEAST_ONCE, 1.0
            )
        with pytest.raises(ReservedTopicError):
            MessagingClient.publish_northbound_confirmed(
                RESERVED_TOPIC, _envelope(), Qos.AT_LEAST_ONCE, 1.0
            )
        assert provider.nothing_happened()


class TestValidateReplyTarget:
    def test_a_usable_reply_target_is_returned(self, provider):
        assert MessagingClient.validate_reply_target(_request("reply/ok")) == "reply/ok"

    def test_a_request_with_no_reply_to_has_nothing_to_reply_to(self, provider):
        # Fire-and-forget: retaining it as server-side reply state would strand the entry.
        with pytest.raises(ValueError, match="non-empty reply_to"):
            MessagingClient.validate_reply_target(_envelope("Req"))

    def test_a_missing_request_is_refused(self, provider):
        with pytest.raises(ValueError, match="non-empty reply_to"):
            MessagingClient.validate_reply_target(None)

    def test_a_hostile_request_cannot_aim_a_reply_at_a_reserved_class(self, provider):
        with pytest.raises(ReservedTopicError):
            MessagingClient.validate_reply_target(_request(RESERVED_TOPIC))


class TestConfirmedReply:
    def test_it_guards_the_target_stamps_correlation_and_uses_the_strict_path(
        self, provider
    ):
        request = _request("reply/confirmed")
        reply = _envelope("Reply")
        assert reply.get_correlation_id() != request.get_correlation_id()

        MessagingClient.reply_confirmed(request, reply, 2.0)

        assert reply.get_correlation_id() == request.get_correlation_id(), (
            "the reply must inherit the request's correlation before it is encoded"
        )
        topic, encoded, qos, timeout = provider.confirmed[0]
        assert topic == "reply/confirmed"
        assert qos is Qos.AT_LEAST_ONCE, "a confirmed reply is always QoS 1"
        assert timeout == 2.0
        assert Message.from_bytes(encoded).get_correlation_id() == \
            request.get_correlation_id(), "the correlation must be IN the encoded bytes"
        assert not provider.replies, "must not fall back to the best-effort reply()"

    def test_the_northbound_variant_uses_the_strict_northbound_path(self, provider):
        request = _request("reply/nb-confirmed")
        reply = _envelope("Reply")

        MessagingClient.reply_northbound_confirmed(request, reply, 3.0)

        topic, encoded, qos, timeout = provider.confirmed_nb[0]
        assert (topic, qos, timeout) == ("reply/nb-confirmed", Qos.AT_LEAST_ONCE, 3.0)
        assert Message.from_bytes(encoded).get_correlation_id() == \
            request.get_correlation_id()
        assert not provider.replies

    @pytest.mark.parametrize("reply_to", [None, ""])
    def test_a_reply_with_no_target_is_refused(self, reply_to, provider):
        request = _envelope("Req") if reply_to is None else _request("")
        with pytest.raises(ValueError, match="non-empty reply_to"):
            MessagingClient.reply_confirmed(request, _envelope("Reply"), 1.0)
        with pytest.raises(ValueError, match="non-empty reply_to"):
            MessagingClient.reply_northbound_confirmed(
                request, _envelope("Reply"), 1.0
            )
        assert provider.nothing_happened()

    def test_a_null_reply_is_refused(self, provider):
        request = _request()
        with pytest.raises(ValueError, match="reply must not be None"):
            MessagingClient.reply_confirmed(request, None, 1.0)
        with pytest.raises(ValueError, match="reply must not be None"):
            MessagingClient.reply_northbound_confirmed(request, None, 1.0)
        assert provider.nothing_happened()

    def test_a_reserved_reply_target_is_refused_before_the_reply_is_stamped(
        self, provider
    ):
        request = _request(RESERVED_TOPIC)
        reply = _envelope("Reply")
        original_correlation = reply.get_correlation_id()

        with pytest.raises(ReservedTopicError):
            MessagingClient.reply_confirmed(request, reply, 1.0)

        assert reply.get_correlation_id() == original_correlation, (
            "a rejected reply must not be mutated"
        )
        assert provider.nothing_happened()


class TestAcknowledgedSubscribe:
    def test_it_passes_every_argument_through_without_a_best_effort_fallback(
        self, provider
    ):
        def callback(topic, msg):
            pass

        MessagingClient.subscribe_acknowledged("f/+", callback, 2, 32, 3.0)

        assert provider.ack_subscribes == [("f/+", callback, 2, 32, 3.0)]
        assert not provider.subscribes, (
            "an acknowledged subscribe must never fall back to the unacknowledged one"
        )

    def test_it_defaults_to_a_bounded_ten_second_acknowledgement_wait(self, provider):
        MessagingClient.subscribe_acknowledged("f/+", lambda t, m: None)
        assert provider.ack_subscribes[0][4] == 10.0

    def test_it_refuses_to_pretend_when_no_provider_is_wired(self):
        previous = MessagingClient._messaging_provider
        MessagingClient._messaging_provider = None
        try:
            with pytest.raises(RuntimeError, match="not initialized"):
                MessagingClient.subscribe_acknowledged("f/+", lambda t, m: None)
        finally:
            MessagingClient._messaging_provider = previous
