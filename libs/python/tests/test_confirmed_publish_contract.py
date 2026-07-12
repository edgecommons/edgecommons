"""The transport-independent strict-confirmation contract on the ``MessagingProvider``
base class, plus its error value types.

These pin the invariants a *provider* must honour before it is allowed to claim
delivery evidence -- the Python mirror of the Java canonical's
``MessagingProviderDeadlineTest`` confirmation cases:

* a confirmed publish requires an **explicit QoS 1** -- never implied, never silently
  downgraded from a weaker level the caller passed;
* a confirmed publish carries **exact bytes** -- a ``str``/``bytearray``/``None`` body is
  refused rather than coerced;
* zero / negative / non-finite deadlines are **rejected**, never truncated into an
  unbounded wait;
* a provider that cannot *prove* acknowledgement must **raise**, not fall back to the
  best-effort ``publish()`` (that would turn queue submission into delivery evidence).

The seam is a no-op provider subclass, so nothing here touches a broker.
"""
import math

import pytest

from edgecommons.messaging.errors import (
    PublishConfirmationError,
    PublishConfirmationReason,
)
from edgecommons.messaging.messaging_provider import MessagingProvider
from edgecommons.messaging.qos import Qos


class _NoopProvider(MessagingProvider):
    """Minimal concrete provider: implements the abstract surface, overrides nothing
    strict -- so the base-class defaults are what the tests observe."""

    def __init__(self):
        super().__init__()
        self.best_effort_publishes = []

    def disconnect(self):
        pass

    def connected(self) -> bool:
        return True

    def publish(self, topic, msg):
        self.best_effort_publishes.append((topic, msg))

    def publish_raw(self, topic, msg):
        self.best_effort_publishes.append((topic, msg))

    def publish_northbound(self, topic, msg, qos):
        self.best_effort_publishes.append((topic, msg, qos))

    def publish_northbound_raw(self, topic, msg, qos):
        self.best_effort_publishes.append((topic, msg, qos))

    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None):
        pass

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
        pass

    def reply_northbound(self, request_msg, response_msg):
        pass

    def cancel_request(self, iou):
        pass

    def cancel_request_northbound(self, iou):
        pass

    def get_native_client(self):
        return None


ENVELOPE = b'{"header": {"name": "N"}, "body": {}}'


class TestUnprovenTransportRefusesToConfirm:
    """A provider that cannot prove acknowledgement must raise -- never degrade to a
    best-effort publish and call it confirmed."""

    def test_confirmed_publish_is_not_silently_downgraded_to_best_effort(self):
        provider = _NoopProvider()
        with pytest.raises(NotImplementedError):
            provider.publish_confirmed("t/a", ENVELOPE, Qos.AT_LEAST_ONCE, 1.0)
        assert provider.best_effort_publishes == [], (
            "an unsupported confirmed publish must not fall through to publish()"
        )

    def test_confirmed_northbound_publish_is_not_silently_downgraded(self):
        provider = _NoopProvider()
        with pytest.raises(NotImplementedError):
            provider.publish_northbound_confirmed(
                "t/a", ENVELOPE, Qos.AT_LEAST_ONCE, 1.0
            )
        assert provider.best_effort_publishes == []

    def test_acknowledged_subscribe_has_no_best_effort_fallback(self):
        provider = _NoopProvider()
        with pytest.raises(NotImplementedError):
            provider.subscribe_acknowledged("t/+", lambda topic, msg: None)


class TestConfirmationRequiresQos1:
    """QoS 1 is the whole basis of the acknowledgement; a weaker level cannot be
    accepted, silently raised, or inferred."""

    @pytest.mark.parametrize("qos", [Qos.AT_MOST_ONCE, Qos.EXACTLY_ONCE])
    def test_a_non_qos1_level_is_rejected(self, qos):
        with pytest.raises(ValueError, match="QoS 1"):
            MessagingProvider._validated_confirmation_timeout(ENVELOPE, qos, 1.0)

    def test_qos1_is_accepted_and_the_deadline_is_returned_as_a_float(self):
        timeout = MessagingProvider._validated_confirmation_timeout(
            ENVELOPE, Qos.AT_LEAST_ONCE, 3
        )
        assert timeout == 3.0
        assert isinstance(timeout, float)

    def test_a_raw_mqtt_level_is_not_accepted_in_place_of_the_enum(self):
        # Passing the wire level 1 must not be mistaken for Qos.AT_LEAST_ONCE.
        with pytest.raises(ValueError, match="QoS 1"):
            MessagingProvider._validated_confirmation_timeout(ENVELOPE, 1, 1.0)


class TestConfirmationRequiresExactBytes:
    """The confirmed path exists so a durable outbox can retry byte-identical
    envelopes; anything that would need re-encoding is refused."""

    @pytest.mark.parametrize(
        "body",
        [
            '{"header": {}}',          # str -- would need an encoding choice
            bytearray(ENVELOPE),       # mutable -- the bytes could change under us
            memoryview(ENVELOPE),
            None,
            {"header": {}},
        ],
    )
    def test_a_non_bytes_body_is_rejected(self, body):
        with pytest.raises(TypeError, match="must be bytes"):
            MessagingProvider._validated_confirmation_timeout(
                body, Qos.AT_LEAST_ONCE, 1.0
            )

    def test_empty_bytes_are_structurally_acceptable_to_the_transport_layer(self):
        # Envelope *validity* is the client's guard (see the client delegation suite);
        # the provider-level contract is only "exact bytes".
        assert MessagingProvider._validated_confirmation_timeout(
            b"", Qos.AT_LEAST_ONCE, 1.0
        ) == 1.0


class TestConfirmationDeadlineBounds:
    """A confirmed publish blocks. An unusable deadline must fail loudly rather than
    become an unbounded wait."""

    @pytest.mark.parametrize("timeout", [0, 0.0, -1, -0.001, math.inf, -math.inf])
    def test_a_non_positive_or_infinite_deadline_is_rejected(self, timeout):
        with pytest.raises(ValueError, match="finite and positive"):
            MessagingProvider._validated_confirmation_timeout(
                ENVELOPE, Qos.AT_LEAST_ONCE, timeout
            )

    def test_nan_is_rejected(self):
        with pytest.raises(ValueError, match="finite and positive"):
            MessagingProvider._validated_confirmation_timeout(
                ENVELOPE, Qos.AT_LEAST_ONCE, math.nan
            )

    @pytest.mark.parametrize("timeout", [True, False, "5", None, [5]])
    def test_a_non_numeric_deadline_is_rejected(self, timeout):
        # bool is an int subclass -- True must NOT silently mean "1 second".
        with pytest.raises(TypeError, match="must be a number"):
            MessagingProvider._validated_confirmation_timeout(
                ENVELOPE, Qos.AT_LEAST_ONCE, timeout
            )

    def test_a_tiny_positive_deadline_is_honoured(self):
        assert MessagingProvider._validated_confirmation_timeout(
            ENVELOPE, Qos.AT_LEAST_ONCE, 0.001
        ) == 0.001


class TestAcknowledgedSubscribeDeadlineBounds:
    """The acknowledged-subscribe deadline carries the same bounds: lifecycle code must
    never be able to block forever waiting for a SUBACK."""

    @pytest.mark.parametrize("timeout", [0, -1, math.inf, math.nan])
    def test_a_non_positive_or_non_finite_deadline_is_rejected(self, timeout):
        with pytest.raises(ValueError, match="finite and positive"):
            MessagingProvider._validated_subscribe_timeout(timeout)

    @pytest.mark.parametrize("timeout", [True, "10", None])
    def test_a_non_numeric_deadline_is_rejected(self, timeout):
        with pytest.raises(TypeError, match="must be a number"):
            MessagingProvider._validated_subscribe_timeout(timeout)

    def test_a_positive_deadline_is_returned_as_a_float(self):
        timeout = MessagingProvider._validated_subscribe_timeout(10)
        assert timeout == 10.0
        assert isinstance(timeout, float)


class TestPublishConfirmationError:
    """The failure value type callers branch on: a timeout is *never* success, and the
    reason must be a real enum member so callers can dispatch on it."""

    def test_it_carries_a_reason_and_a_message_and_is_catchable_as_a_runtime_error(self):
        err = PublishConfirmationError(
            PublishConfirmationReason.TIMEOUT, "no PUBACK for 'a/b'"
        )
        assert err.reason is PublishConfirmationReason.TIMEOUT
        assert "no PUBACK for 'a/b'" in str(err)
        assert isinstance(err, RuntimeError)

    @pytest.mark.parametrize(
        "reason",
        [
            PublishConfirmationReason.TIMEOUT,
            PublishConfirmationReason.TRANSPORT_ERROR,
            PublishConfirmationReason.INTERRUPTED,
        ],
    )
    def test_every_reason_round_trips(self, reason):
        assert PublishConfirmationError(reason, "x").reason is reason

    @pytest.mark.parametrize("reason", ["TIMEOUT", 1, None])
    def test_a_reason_that_is_not_an_enum_member_is_rejected(self, reason):
        # A stringly-typed reason would silently break every caller that branches on it.
        with pytest.raises(ValueError, match="PublishConfirmationReason"):
            PublishConfirmationError(reason, "x")
