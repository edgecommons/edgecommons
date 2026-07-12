"""The ``app()`` facade's prepare / confirmed-publish surface.

``prepare()`` exists so a durable outbox can freeze an envelope -- topic, UUID, timestamp,
correlation -- *before* it tries to send, and then retry the **identical bytes** until the
transport confirms them. The contracts that make that safe:

* preparing publishes nothing;
* a prepared message's bytes are **stable and exact** -- the same object always yields the
  same envelope UUID, and its ``message`` view is a fresh parse that cannot mutate them;
* ``prepare_correlated`` carries an existing conversation's correlation id into the
  envelope, and refuses a missing/blank one rather than minting a fresh one;
* ``publish_confirmed`` hands the transport the prepared bytes verbatim at QoS 1, and --
  unlike the fire-and-forget ``publish_prepared`` -- **propagates** a northbound failure,
  because an outbox must be able to leave the record pending and retry the same UUID.
"""
import pytest

from edgecommons.facades.app_facade import AppFacade, PreparedAppMessage
from edgecommons.facades.channel import Channel
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.qos import Qos
from edgecommons.uns import Uns

IDENTITY = MessageIdentity([HierEntry("device", "gw-01")], "opcua-adapter", "main")
APP_TOPIC = "ecv1/gw-01/opcua-adapter/main/app/order/received"


class _FakeConfigManager:
    def get_component_identity(self):
        return IDENTITY

    def get_global_config(self):
        return {}

    def get_tag_config(self):
        return None


class _RecordingTransport:
    def __init__(self, northbound_confirmed_error=None, northbound_error=None):
        self.local = []               # (topic, msg)
        self.northbound = []          # (topic, msg, qos)
        self.confirmed = []           # (topic, bytes, qos, timeout)
        self.northbound_confirmed = []
        self._northbound_confirmed_error = northbound_confirmed_error
        self._northbound_error = northbound_error

    def publish(self, topic, msg):
        self.local.append((topic, msg))

    def publish_northbound(self, topic, msg, qos):
        if self._northbound_error is not None:
            raise self._northbound_error
        self.northbound.append((topic, msg, qos))

    def publish_confirmed(self, topic, encoded, qos, timeout_secs):
        self.confirmed.append((topic, encoded, qos, timeout_secs))

    def publish_northbound_confirmed(self, topic, encoded, qos, timeout_secs):
        if self._northbound_confirmed_error is not None:
            raise self._northbound_confirmed_error
        self.northbound_confirmed.append((topic, encoded, qos, timeout_secs))


def _facade(transport=None):
    transport = transport if transport is not None else _RecordingTransport()
    return AppFacade(_FakeConfigManager(), "main", Uns(IDENTITY, False), transport), \
        transport


class TestPrepare:
    def test_preparing_builds_the_envelope_without_publishing_anything(self):
        facade, transport = _facade()

        prepared = facade.prepare("OrderReceived", "order/received", {"orderId": "A-42"})

        assert prepared.topic == APP_TOPIC
        assert not transport.local and not transport.confirmed, (
            "prepare() must not touch the transport"
        )

    def test_the_prepared_body_and_header_survive_the_round_trip(self):
        facade, _ = _facade()

        prepared = facade.prepare("OrderReceived", "order/received", {"orderId": "A-42"})

        decoded = Message.from_bytes(prepared.encoded_bytes).to_dict()
        assert decoded["header"]["name"] == "OrderReceived"
        assert decoded["header"]["version"] == AppFacade.APP_MESSAGE_VERSION
        assert decoded["body"] == {"orderId": "A-42"}

    def test_the_channel_is_sanitized_into_the_topic(self):
        facade, _ = _facade()
        assert facade.prepare("Ping", "a+b", {}).topic.endswith("/app/a_b")

    def test_the_bytes_are_stable_so_a_retry_replays_the_same_envelope(self):
        facade, _ = _facade()

        prepared = facade.prepare("OrderReceived", "order/received", {})

        first, second = prepared.encoded_bytes, prepared.encoded_bytes
        assert first == second
        assert prepared.message.get_header().uuid == prepared.message.get_header().uuid, (
            "a retry must replay the SAME envelope UUID, not mint a new one"
        )

    def test_two_prepares_are_distinct_envelopes(self):
        facade, _ = _facade()
        one = facade.prepare("X", "c", {})
        two = facade.prepare("X", "c", {})
        assert Message.from_bytes(one.encoded_bytes).get_header().uuid != \
            Message.from_bytes(two.encoded_bytes).get_header().uuid

    def test_the_message_view_is_a_fresh_parse_that_cannot_corrupt_the_prepared_bytes(
        self,
    ):
        facade, _ = _facade()
        prepared = facade.prepare("X", "c", {})
        frozen = prepared.encoded_bytes

        view = prepared.message
        view.set_correlation_id("tampered")

        assert prepared.encoded_bytes == frozen
        assert prepared.message.get_correlation_id() != "tampered"

    @pytest.mark.parametrize("name,channel", [("", "c"), (None, "c"), ("X", ""), ("X", None)])
    def test_a_missing_name_or_channel_is_rejected(self, name, channel):
        facade, transport = _facade()
        with pytest.raises(ValueError):
            facade.prepare(name, channel, {})
        assert not transport.local


class TestPrepareCorrelated:
    def _request(self, correlation_id="corr-88"):
        return MessageBuilder.create("Req", "1.0").with_payload({}) \
            .with_correlation_id(correlation_id).build()

    def test_a_request_carries_its_correlation_into_the_prepared_envelope(self):
        facade, _ = _facade()

        prepared = facade.prepare_correlated(
            "OrderAck", "order/ack", {"ok": True}, self._request("corr-88")
        )

        assert Message.from_bytes(prepared.encoded_bytes).get_correlation_id() == "corr-88"

    def test_a_bare_correlation_id_is_accepted(self):
        facade, _ = _facade()

        prepared = facade.prepare_correlated("OrderAck", "order/ack", {}, "corr-99")

        assert Message.from_bytes(prepared.encoded_bytes).get_correlation_id() == "corr-99"

    def test_an_uncorrelated_prepare_still_gets_its_own_correlation(self):
        facade, _ = _facade()
        prepared = facade.prepare("X", "c", {})
        assert Message.from_bytes(prepared.encoded_bytes).get_correlation_id()

    def test_a_blank_correlation_id_is_refused_rather_than_silently_minted(self):
        facade, _ = _facade()
        with pytest.raises(ValueError, match="non-empty correlation id"):
            facade.prepare_correlated("X", "c", {}, "")

    @pytest.mark.parametrize("bad", [None, 42, {"correlation_id": "x"}])
    def test_something_that_is_neither_a_request_nor_an_id_is_refused(self, bad):
        facade, _ = _facade()
        with pytest.raises(ValueError, match="request or correlation id"):
            facade.prepare_correlated("X", "c", {}, bad)


class TestPublishPrepared:
    def test_it_publishes_the_prepared_topic_and_envelope_locally(self):
        facade, transport = _facade()
        prepared = facade.prepare("OrderReceived", "order/received", {"orderId": "A-42"})

        facade.publish_prepared(prepared)

        topic, msg = transport.local[0]
        assert topic == APP_TOPIC
        assert msg.to_bytes() == prepared.encoded_bytes

    def test_northbound_routing_goes_northbound_at_qos_1(self):
        facade, transport = _facade()
        prepared = facade.prepare("CloudEvent", "cloud", {})

        facade.publish_prepared(prepared, Channel.NORTHBOUND)

        assert transport.northbound[0][2] is Qos.AT_LEAST_ONCE
        assert not transport.local

    def test_a_northbound_outage_does_not_propagate_on_the_fire_and_forget_path(self):
        facade, _ = _facade(_RecordingTransport(northbound_error=OSError("cloud down")))
        prepared = facade.prepare("CloudEvent", "cloud", {})

        facade.publish_prepared(prepared, Channel.NORTHBOUND)  # must not raise

    def test_the_stream_channel_is_rejected(self):
        facade, transport = _facade()
        prepared = facade.prepare("X", "c", {})
        with pytest.raises(ValueError, match="stream"):
            facade.publish_prepared(prepared, Channel.stream("hot"))
        assert not transport.local

    @pytest.mark.parametrize("bad", [None, "not-prepared", 42])
    def test_something_that_is_not_a_prepared_message_is_refused(self, bad):
        facade, transport = _facade()
        with pytest.raises(ValueError, match="PreparedAppMessage"):
            facade.publish_prepared(bad)
        assert not transport.local


class TestPublishConfirmed:
    def test_it_hands_the_transport_the_prepared_bytes_verbatim_at_qos_1(self):
        facade, transport = _facade()
        prepared = facade.prepare("OrderReceived", "order/received", {"orderId": "A-42"})

        facade.publish_confirmed(prepared, 5.0)

        topic, encoded, qos, timeout = transport.confirmed[0]
        assert topic == APP_TOPIC
        assert encoded == prepared.encoded_bytes, (
            "the transport must get the prepared bytes, not a re-encoding"
        )
        assert qos is Qos.AT_LEAST_ONCE
        assert timeout == 5.0
        assert not transport.local, "must not degrade to the best-effort publish"

    def test_the_northbound_variant_uses_the_strict_northbound_path(self):
        facade, transport = _facade()
        prepared = facade.prepare("CloudEvent", "cloud", {})

        facade.publish_confirmed(prepared, 2.5, Channel.NORTHBOUND)

        topic, encoded, qos, timeout = transport.northbound_confirmed[0]
        assert encoded == prepared.encoded_bytes
        assert (qos, timeout) == (Qos.AT_LEAST_ONCE, 2.5)
        assert not transport.northbound, "must not degrade to the best-effort publish"

    def test_a_northbound_failure_propagates_so_the_outbox_can_retry(self):
        # The decisive difference from publish_prepared: a swallowed failure here would
        # let an outbox mark an undelivered record as sent.
        facade, _ = _facade(
            _RecordingTransport(northbound_confirmed_error=OSError("cloud down"))
        )
        prepared = facade.prepare("CloudEvent", "cloud", {})

        with pytest.raises(OSError, match="cloud down"):
            facade.publish_confirmed(prepared, 1.0, Channel.NORTHBOUND)

    def test_the_stream_channel_is_rejected(self):
        facade, transport = _facade()
        prepared = facade.prepare("X", "c", {})
        with pytest.raises(ValueError, match="stream"):
            facade.publish_confirmed(prepared, 1.0, Channel.stream("hot"))
        assert not transport.confirmed

    @pytest.mark.parametrize("bad", [None, b"raw-bytes", 42])
    def test_something_that_is_not_a_prepared_message_is_refused(self, bad):
        facade, transport = _facade()
        with pytest.raises(ValueError, match="PreparedAppMessage"):
            facade.publish_confirmed(bad, 1.0)
        assert not transport.confirmed


class TestPreparedAppMessageInvariants:
    def _message(self):
        return MessageBuilder.create("X", "1.0").with_payload({}).build()

    def test_the_bytes_must_be_the_exact_serialization_of_the_message(self):
        message = self._message()
        other = self._message()
        with pytest.raises(ValueError, match="exact serialization"):
            PreparedAppMessage("t/a", message, other.to_bytes())

    def test_an_empty_topic_is_refused(self):
        message = self._message()
        with pytest.raises(ValueError, match="topic must not be empty"):
            PreparedAppMessage("", message, message.to_bytes())

    def test_a_missing_message_is_refused(self):
        with pytest.raises(ValueError, match="message must not be None"):
            PreparedAppMessage("t/a", None, b"")

    @pytest.mark.parametrize("bad", ["a string", bytearray(b"x"), None])
    def test_non_bytes_are_refused(self, bad):
        message = self._message()
        with pytest.raises(TypeError, match="must be bytes"):
            PreparedAppMessage("t/a", message, bad)

    def test_the_bytes_are_copied_so_the_caller_cannot_mutate_them_afterwards(self):
        message = self._message()
        prepared = PreparedAppMessage("t/a", message, message.to_bytes())
        assert prepared.encoded_bytes == message.to_bytes()

    def test_it_is_frozen(self):
        message = self._message()
        prepared = PreparedAppMessage("t/a", message, message.to_bytes())
        with pytest.raises(Exception):
            prepared.topic = "t/other"
