"""The dual-MQTT provider's strict-confirmation transport surface, against a fake paho
client: ``publish_confirmed`` / ``publish_northbound_confirmed`` (positive PUBACK
evidence) and ``subscribe_acknowledged`` (positive SUBACK evidence).

The contracts pinned here are the ones that make "confirmed" mean something:

* the wire QoS is **1**, whatever the channel's configured default publish QoS is;
* ``wait_for_publish()`` returning is **not** evidence -- paho returns normally when its
  own timeout elapses, so ``is_published()`` is the acknowledgement test. Deleting that
  check would turn every timeout into a false success, and this suite fails if it does;
* every failure mode is reported as a ``PublishConfirmationError`` with a **reason**, and
  the in-flight permit is **released** on every path (a leak would wedge the outbox);
* a SUBACK that never arrives, or arrives negative, **raises** -- and a broken
  ``unsubscribe`` during that cleanup must not mask the original failure, nor strand the
  caller.

No broker is contacted: ``paho.mqtt.client.Client`` is replaced with an in-process fake
whose PUBACK/SUBACK behavior each test drives directly.
"""
import threading
import time

import paho.mqtt.client as mqtt
import pytest

import edgecommons.messaging.providers.standalone_provider as sp
from edgecommons.messaging.errors import (
    PublishConfirmationError,
    PublishConfirmationReason,
)
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_config import (
    CredentialsConfig,
    LocalMqttConfig,
    MessagingConfigData,
    MessagingConfiguration,
    NorthboundMqttConfig,
)
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider
from edgecommons.messaging.qos import Qos


# --------------------------------------------------------------------------- fakes


class _PublishResult:
    """Stand-in for paho's ``MQTTMessageInfo``."""

    def __init__(self, rc, published, wait_raises=None):
        self.rc = rc
        self._published = published
        self._wait_raises = wait_raises
        self.waited = False

    def wait_for_publish(self, timeout=None):
        self.waited = True
        if self._wait_raises is not None:
            raise self._wait_raises
        # Paho deliberately returns normally when its timeout elapses -- it does NOT
        # raise. is_published() is the only positive acknowledgement.

    def is_published(self):
        return self._published


class FakeClient:
    """In-process stand-in for ``paho.mqtt.client.Client`` with drivable ack behavior."""

    instances = []
    #: When set, disconnect()/loop_stop() blow up -- used to prove a broken teardown
    #: cannot mask the error that triggered it.
    teardown_raises = None

    def __init__(self, callback_api_version=None, client_id=None):
        self.client_id = client_id
        self._connected = True
        self.loop_stopped = False
        self.on_message = None
        self.on_connect = None
        self.on_disconnect = None
        self.on_subscribe = None
        self.tls_context = None
        self.published = []       # (topic, payload, qos)
        self.subscribed = []
        self.unsubscribed = []
        self.results = []         # every _PublishResult handed back
        self._next_mid = 1

        # --- drivable PUBLISH behavior
        self.publish_rc = mqtt.MQTT_ERR_SUCCESS
        self.publish_acknowledged = True     # what is_published() will report
        self.publish_raises = None
        self.publish_wait_raises = None
        self.publish_delay = 0.0             # seconds burned inside publish()

        # --- drivable SUBSCRIBE behavior
        self.auto_suback = True
        self.suback_rc = mqtt.MQTT_ERR_SUCCESS
        self.suback_failure_topics = set()
        self.unsubscribe_raises = None

        FakeClient.instances.append(self)

    # ---- configuration / lifecycle
    def username_pw_set(self, username, password):
        pass

    def tls_set_context(self, ctx):
        self.tls_context = ctx

    def connect_async(self, host, port, keepalive):
        self._connected = True

    def loop_start(self):
        pass

    def loop_stop(self):
        if type(self).teardown_raises is not None:
            raise type(self).teardown_raises
        self.loop_stopped = True

    def disconnect(self):
        if type(self).teardown_raises is not None:
            raise type(self).teardown_raises
        self._connected = False

    def is_connected(self):
        return self._connected

    # ---- pub/sub
    def publish(self, topic, payload, qos=0):
        if self.publish_delay:
            time.sleep(self.publish_delay)
        if self.publish_raises is not None:
            raise self.publish_raises
        self.published.append((topic, payload, qos))
        result = _PublishResult(
            self.publish_rc, self.publish_acknowledged, self.publish_wait_raises
        )
        self.results.append(result)
        return result

    def subscribe(self, topic, qos=0):
        if self.suback_rc != mqtt.MQTT_ERR_SUCCESS:
            return (self.suback_rc, None)
        mid = self._next_mid
        self._next_mid += 1
        self.subscribed.append((topic, qos))
        if self.auto_suback:
            granted = [0x80] if topic in self.suback_failure_topics else [qos]
            threading.Timer(
                0.02,
                lambda: self.on_subscribe and self.on_subscribe(
                    self, None, mid, granted, None
                ),
            ).start()
        return (mqtt.MQTT_ERR_SUCCESS, mid)

    def unsubscribe(self, topic):
        if self.unsubscribe_raises is not None:
            raise self.unsubscribe_raises
        self.unsubscribed.append(topic)

    # ---- test helper
    def deliver(self, topic, message):
        from types import SimpleNamespace

        self.on_message(
            self, None,
            SimpleNamespace(topic=topic, payload=message.to_bytes(), qos=1),
        )


@pytest.fixture(autouse=True)
def _patch_paho(monkeypatch):
    FakeClient.instances = []
    FakeClient.teardown_raises = None
    monkeypatch.setattr(sp.mqtt, "Client", FakeClient)
    yield
    FakeClient.teardown_raises = None


@pytest.fixture(autouse=True)
def _patch_ssl(monkeypatch):
    """Keep the northbound TLS path off the filesystem."""
    import ssl as _ssl
    from types import SimpleNamespace

    monkeypatch.setattr(_ssl, "create_default_context", lambda *a, **k: SimpleNamespace(
        load_verify_locations=lambda *a, **k: None,
        load_cert_chain=lambda *a, **k: None,
        check_hostname=True,
        verify_mode=None,
    ))
    yield


def _local_config():
    cfg = MessagingConfiguration()
    cfg.messaging = MessagingConfigData(
        local=LocalMqttConfig(type="mqtt", host="localhost", port=1883,
                              client_id="local-cid"),
        northbound=None,
    )
    return cfg


def _dual_config():
    cfg = MessagingConfiguration()
    cfg.messaging = MessagingConfigData(
        local=LocalMqttConfig(type="mqtt", host="localhost", port=1883,
                              client_id="local-cid"),
        northbound=NorthboundMqttConfig(
            endpoint="nb.example.com", port=8883, client_id="nb-cid",
            credentials=CredentialsConfig(cert_path="c.pem", key_path="k.pem",
                                          ca_path="a.pem"),
        ),
    )
    return cfg


@pytest.fixture
def provider():
    prov = StandaloneProvider(_local_config(), "thing-1")
    yield prov
    prov.disconnect()


@pytest.fixture
def dual_provider():
    prov = StandaloneProvider(_dual_config(), "thing-1")
    yield prov
    prov.disconnect()


def _local(provider):
    return provider.get_native_client()["local"]


def _northbound(provider):
    return provider.get_native_client()["northbound"]


ENVELOPE = MessageBuilder.create("Confirmed", "1.0").with_payload({"k": "v"}).build()
BYTES = ENVELOPE.to_bytes()


# ===================== publish_confirmed: the happy path =====================


class TestConfirmedPublishHappyPath:
    def test_the_exact_bytes_reach_the_wire_at_qos_1(self, provider):
        provider.publish_confirmed("t/confirmed", BYTES, Qos.AT_LEAST_ONCE, 2.0)

        topic, payload, qos = _local(provider).published[0]
        assert topic == "t/confirmed"
        assert payload == BYTES, "the caller's exact envelope bytes must be published"
        assert qos == 1, "a confirmed publish is always QoS 1 on the wire"

    def test_it_waits_for_the_puback_rather_than_returning_on_submission(self, provider):
        provider.publish_confirmed("t/confirmed", BYTES, Qos.AT_LEAST_ONCE, 2.0)
        result = _local(provider).results[0]
        assert result.waited, "the publish must block on the broker acknowledgement"

    def test_the_northbound_variant_publishes_on_the_northbound_broker_only(
        self, dual_provider
    ):
        dual_provider.publish_northbound_confirmed(
            "t/nb", BYTES, Qos.AT_LEAST_ONCE, 2.0
        )
        assert _northbound(dual_provider).published[0][0] == "t/nb"
        assert _local(dual_provider).published == []


# ===================== publish_confirmed: refusals before I/O =====================


class TestConfirmedPublishRefusesUnusableArguments:
    @pytest.mark.parametrize("qos", [Qos.AT_MOST_ONCE, Qos.EXACTLY_ONCE])
    def test_a_non_qos1_publish_is_rejected_and_nothing_is_sent(self, qos, provider):
        with pytest.raises(ValueError, match="QoS 1"):
            provider.publish_confirmed("t/c", BYTES, qos, 2.0)
        assert _local(provider).published == []

    @pytest.mark.parametrize("timeout", [0, -1, float("inf"), float("nan")])
    def test_an_unusable_deadline_is_rejected_and_nothing_is_sent(
        self, timeout, provider
    ):
        with pytest.raises(ValueError, match="finite and positive"):
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, timeout)
        assert _local(provider).published == [], (
            "a rejected deadline must never become an unbounded blocking publish"
        )


# ===================== publish_confirmed: the failure modes =====================


class TestConfirmedPublishFailureModes:
    def test_an_unobserved_puback_is_a_timeout_not_a_success(self, provider):
        # THE critical one: paho's wait_for_publish() returns normally when its timeout
        # elapses. Only is_published() proves delivery. If that check is ever dropped,
        # every timed-out publish would be reported as delivered -- this test fails first.
        client = _local(provider)
        client.publish_acknowledged = False

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 0.05)

        assert exc.value.reason is PublishConfirmationReason.TIMEOUT
        assert "PUBACK" in str(exc.value)
        assert client.results[0].waited

    def test_a_client_level_rejection_is_a_transport_error(self, provider):
        _local(provider).publish_rc = mqtt.MQTT_ERR_NO_CONN

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 1.0)

        assert exc.value.reason is PublishConfirmationReason.TRANSPORT_ERROR
        assert "rejected by the MQTT client" in str(exc.value)

    def test_a_throwing_publish_is_a_transport_error(self, provider):
        _local(provider).publish_raises = OSError("socket gone")

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 1.0)

        assert exc.value.reason is PublishConfirmationReason.TRANSPORT_ERROR
        assert isinstance(exc.value.__cause__, OSError)

    def test_a_throwing_wait_for_publish_is_a_transport_error(self, provider):
        _local(provider).publish_wait_raises = RuntimeError("not queued")

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 1.0)

        assert exc.value.reason is PublishConfirmationReason.TRANSPORT_ERROR
        assert "awaiting PUBACK" in str(exc.value)

    def test_publishing_after_disconnect_is_a_transport_error_not_a_crash(self, provider):
        provider.disconnect()  # the channel's client is gone

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 1.0)

        assert exc.value.reason is PublishConfirmationReason.TRANSPORT_ERROR

    def test_a_publish_that_burns_the_whole_deadline_never_waits_for_a_puback(
        self, provider
    ):
        # The send itself outlived the caller's deadline: there is no budget left to wait
        # for the PUBACK, so the publish must be reported as unconfirmed rather than
        # blocking past the deadline.
        client = _local(provider)
        client.publish_delay = 0.3

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 0.05)

        assert exc.value.reason is PublishConfirmationReason.TIMEOUT
        assert not client.results[0].waited


# ===================== publish_confirmed: the in-flight permit =====================


class _StalledPermits:
    """Permits whose acquire() burns more time than the caller's whole deadline."""

    def __init__(self, delay):
        self.delay = delay
        self.released = 0

    def acquire(self, timeout=None):
        time.sleep(self.delay)
        return True

    def release(self):
        self.released += 1


class TestConfirmedPublishInFlightPermit:
    def test_exhausted_capacity_times_out_rather_than_queueing_without_bound(
        self, provider
    ):
        provider._confirmed_publish_permits = threading.BoundedSemaphore(1)
        provider._confirmed_publish_permits.acquire()  # the only permit is taken
        try:
            with pytest.raises(PublishConfirmationError) as exc:
                provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 0.05)
        finally:
            provider._confirmed_publish_permits.release()

        assert exc.value.reason is PublishConfirmationReason.TIMEOUT
        assert "waiting for capacity" in str(exc.value)
        assert _local(provider).published == [], "nothing may be sent without a permit"

    def test_a_deadline_consumed_while_queueing_is_not_spent_on_a_doomed_send(
        self, provider
    ):
        provider._confirmed_publish_permits = _StalledPermits(delay=0.3)

        with pytest.raises(PublishConfirmationError) as exc:
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 0.05)

        assert exc.value.reason is PublishConfirmationReason.TIMEOUT
        assert "timed out before send" in str(exc.value)
        assert _local(provider).published == []
        assert provider._confirmed_publish_permits.released == 1, (
            "the permit must be released even when the deadline expired while queueing"
        )

    def test_every_failure_path_releases_its_permit(self, provider):
        # A permit leak would wedge the outbox after N failures. Drive the bound down to
        # 1 so a single leak is immediately fatal, then fail repeatedly and recover.
        provider._confirmed_publish_permits = threading.BoundedSemaphore(1)
        client = _local(provider)

        client.publish_acknowledged = False
        for _ in range(3):
            with pytest.raises(PublishConfirmationError):
                provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 0.05)

        client.publish_rc = mqtt.MQTT_ERR_NO_CONN
        with pytest.raises(PublishConfirmationError):
            provider.publish_confirmed("t/c", BYTES, Qos.AT_LEAST_ONCE, 0.05)

        # The single permit survived every failure: a confirmed publish still works.
        client.publish_rc = mqtt.MQTT_ERR_SUCCESS
        client.publish_acknowledged = True
        provider.publish_confirmed("t/recovered", BYTES, Qos.AT_LEAST_ONCE, 1.0)
        assert client.published[-1][0] == "t/recovered"

    def test_a_rejected_argument_does_not_consume_a_permit(self, provider):
        provider._confirmed_publish_permits = threading.BoundedSemaphore(1)

        with pytest.raises(ValueError):
            provider.publish_confirmed("t/c", BYTES, Qos.AT_MOST_ONCE, 1.0)

        provider.publish_confirmed("t/after", BYTES, Qos.AT_LEAST_ONCE, 1.0)
        assert _local(provider).published[-1][0] == "t/after"


# ===================== subscribe_acknowledged =====================


class TestAcknowledgedSubscribe:
    def test_it_returns_only_after_the_suback_and_delivers_messages(self, provider):
        received = []
        provider.subscribe_acknowledged(
            "cmd/+", lambda topic, msg: received.append((topic, msg)), timeout_secs=2.0
        )

        assert _local(provider).subscribed[0][0] == "cmd/+"
        _local(provider).deliver("cmd/go", ENVELOPE)
        deadline = time.monotonic() + 2.0
        while not received and time.monotonic() < deadline:
            time.sleep(0.01)
        assert received and received[0][0] == "cmd/go"

    def test_a_negative_suback_is_a_failure_not_a_silent_no_op(self, provider):
        client = _local(provider)
        client.suback_failure_topics = {"cmd/denied"}

        with pytest.raises(RuntimeError, match="rejected subscription"):
            provider.subscribe_acknowledged(
                "cmd/denied", lambda t, m: None, timeout_secs=2.0
            )

        assert "cmd/denied" not in provider._local.subscriptions, (
            "a rejected subscription must not be retained as if it were live"
        )
        assert "cmd/denied" in client.unsubscribed, "the broker-side state is cleaned up"

    def test_a_suback_that_never_arrives_times_the_caller_out_and_leaves_no_state(
        self, provider
    ):
        client = _local(provider)
        client.auto_suback = False  # the broker never answers

        with pytest.raises(TimeoutError, match="timed out after"):
            provider.subscribe_acknowledged(
                "cmd/silent", lambda t, m: None, timeout_secs=0.05
            )

        assert provider._local.subscriptions == {}
        assert provider._local.pending_subscriptions == {}
        assert provider._local.mid_to_topic == {}, "the mid->topic map must not leak"
        assert "cmd/silent" in client.unsubscribed

    def test_a_broken_unsubscribe_during_cleanup_still_times_the_caller_out(
        self, provider
    ):
        # The cleanup is best-effort. A failing unsubscribe must not mask the timeout,
        # and must never strand the caller waiting on a SUBACK that is not coming.
        client = _local(provider)
        client.auto_suback = False
        client.unsubscribe_raises = RuntimeError("unsubscribe failed")

        with pytest.raises(TimeoutError, match="timed out after"):
            provider.subscribe_acknowledged(
                "cmd/silent", lambda t, m: None, timeout_secs=0.05
            )

        assert provider._local.subscriptions == {}

    def test_a_broken_unsubscribe_after_a_negative_suback_still_raises_the_rejection(
        self, provider
    ):
        client = _local(provider)
        client.suback_failure_topics = {"cmd/denied"}
        client.unsubscribe_raises = RuntimeError("unsubscribe failed")

        with pytest.raises(RuntimeError, match="rejected subscription"):
            provider.subscribe_acknowledged(
                "cmd/denied", lambda t, m: None, timeout_secs=2.0
            )

        assert provider._local.subscriptions == {}

    def test_a_refused_subscribe_request_leaves_no_pending_state(self, provider):
        _local(provider).suback_rc = mqtt.MQTT_ERR_NO_CONN

        with pytest.raises(RuntimeError, match="Failed to send"):
            provider.subscribe_acknowledged(
                "cmd/x", lambda t, m: None, timeout_secs=1.0
            )

        assert provider._local.subscriptions == {}
        assert provider._local.pending_subscriptions == {}

    @pytest.mark.parametrize("timeout", [0, -1, float("nan")])
    def test_an_unusable_acknowledgement_deadline_is_rejected(self, timeout, provider):
        with pytest.raises(ValueError, match="finite and positive"):
            provider.subscribe_acknowledged(
                "cmd/x", lambda t, m: None, timeout_secs=timeout
            )
        assert _local(provider).subscribed == []

    def test_unsubscribing_an_in_flight_subscription_releases_the_waiter(self, provider):
        # Shutdown races an in-flight subscribe: the waiter must be woken (and told the
        # subscription is not live) rather than blocking for the whole timeout.
        _local(provider).auto_suback = False
        failure = []

        def subscribe():
            try:
                provider.subscribe_acknowledged(
                    "cmd/pending", lambda t, m: None, timeout_secs=30.0
                )
            except Exception as exc:  # noqa: BLE001 - recorded for the assertion
                failure.append(exc)

        worker = threading.Thread(target=subscribe, daemon=True)
        worker.start()
        deadline = time.monotonic() + 2.0
        while "cmd/pending" not in provider._local.pending_subscriptions \
                and time.monotonic() < deadline:
            time.sleep(0.01)

        provider.unsubscribe("cmd/pending")

        worker.join(timeout=5.0)
        assert not worker.is_alive(), "the waiter was stranded on a dead subscription"
        assert failure and isinstance(failure[0], RuntimeError)
        assert provider._local.subscriptions == {}
        assert provider._local.mid_to_topic == {}


# ===================== partial construction =====================


class TestPartialConstructionIsTornDown:
    """Construction can fail *after* the local paho loop is already running (local broker
    is up, the northbound TLS broker is not). The caller never receives the object and so
    can never call ``disconnect()`` -- the constructor must therefore clean up after
    itself, or every failed start leaks a live network thread."""

    @pytest.fixture
    def _northbound_tls_fails(self, monkeypatch):
        import ssl as _ssl

        def explode(*args, **kwargs):
            raise OSError("northbound CA unreadable")

        monkeypatch.setattr(_ssl, "create_default_context", explode)
        yield

    def test_a_failed_start_stops_the_loop_it_already_started(self, _northbound_tls_fails):
        with pytest.raises(OSError, match="northbound CA unreadable"):
            StandaloneProvider(_dual_config(), "thing-1")

        local = FakeClient.instances[0]
        assert not local.is_connected(), "the started local client must be disconnected"
        assert local.loop_stopped, "the started paho network loop must be stopped"

    def test_a_broken_teardown_does_not_mask_the_initialization_error(
        self, _northbound_tls_fails
    ):
        # The cleanup is best-effort: whatever it hits, the caller must still see WHY the
        # provider failed to start -- not a secondary error from the cleanup itself.
        FakeClient.teardown_raises = RuntimeError("disconnect blew up")

        with pytest.raises(OSError, match="northbound CA unreadable"):
            StandaloneProvider(_dual_config(), "thing-1")


# ===================== northbound channel routing =====================


class TestNorthboundOperationsUseTheNorthboundBroker:
    """Every northbound verb must land on the northbound client and leave the local one
    untouched -- crossing the channels would leak plant-floor traffic to the cloud (or
    vice versa)."""

    def test_subscribe_and_unsubscribe_northbound_target_the_northbound_broker(
        self, dual_provider
    ):
        dual_provider.subscribe_northbound(
            "nb/+", lambda t, m: None, Qos.AT_LEAST_ONCE
        )
        assert _northbound(dual_provider).subscribed[0][0] == "nb/+"
        assert _local(dual_provider).subscribed == []

        dual_provider.unsubscribe_northbound("nb/+")
        assert _northbound(dual_provider).unsubscribed == ["nb/+"]
        assert _local(dual_provider).unsubscribed == []

    def test_publish_northbound_raw_targets_the_northbound_broker(self, dual_provider):
        dual_provider.publish_northbound_raw("nb/raw", {"k": "v"}, Qos.AT_LEAST_ONCE)
        assert _northbound(dual_provider).published[0][0] == "nb/raw"
        assert _local(dual_provider).published == []

    def test_reply_northbound_targets_the_northbound_broker(self, dual_provider):
        request = MessageBuilder.create("Req", "1.0").with_payload({}).build()
        request.make_request("reply/nb")

        dual_provider.reply_northbound(
            request, MessageBuilder.create("Reply", "1.0").with_payload({}).build()
        )

        assert _northbound(dual_provider).published[0][0] == "reply/nb"
        assert _local(dual_provider).published == []

    def test_a_northbound_request_subscribes_and_publishes_northbound_and_is_cancellable(
        self, dual_provider
    ):
        iou = dual_provider.request_northbound(
            "nb/req",
            MessageBuilder.create("Req", "1.0").with_payload({}).build(),
            timeout_secs=5.0,
        )

        northbound = _northbound(dual_provider)
        assert northbound.published[0][0] == "nb/req"
        reply_topic = northbound.subscribed[0][0]
        assert reply_topic.startswith("edgecommons/reply")
        assert _local(dual_provider).published == []

        dual_provider.cancel_request_northbound(iou)
        assert iou.done(), "cancelling must settle the pending request"
        assert reply_topic in northbound.unsubscribed, (
            "cancelling must release the ephemeral reply subscription"
        )
