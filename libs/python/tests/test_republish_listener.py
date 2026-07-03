"""Deterministic unit tests for RepublishListener (DESIGN-uns §9.3/§9.4, the late-join
lever) via the injected delayer/clock/jitter seams — no sleeping, no real scheduler.
Mirrors ``libs/java/.../uns/RepublishListenerTest.java``.

Covers:
- ``start()`` subscribes both own-device ``_bcast`` republish topics on the primary
  connection (exact rootless topics, built with the ``_bcast`` pseudo-component);
- ``republish-state`` re-runs the state action; ``republish-cfg`` the cfg action (verb
  separation);
- the jitter window (``RepublishListener.JITTER_WINDOW_MS`` ms) is passed to the
  injected jitter source and the returned delay is what gets scheduled;
- a broadcast while a re-announce is pending — or within the
  ``RepublishListener.COOLDOWN_MS`` ms cooldown of the last accepted trigger —
  coalesces (no amplification); the verbs rate-limit independently;
- foreign/malformed payloads (wrong header name, raw no-header envelope, None) are
  ignored and never raise;
- ``close()`` unsubscribes both topics, drops pending re-announces and is idempotent;
  a missing resolved identity disables the listener.
"""
import threading

import pytest

from ggcommons.messaging.identity import HierEntry, MessageIdentity
from ggcommons.messaging.message import Message
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.republish_listener import RepublishListener, _ThreadingDelayer

# The default fake identity's device is 'test-thing' (single 'device' level).
STATE_TOPIC = "ecv1/test-thing/_bcast/main/cmd/republish-state"
CFG_TOPIC = "ecv1/test-thing/_bcast/main/cmd/republish-cfg"


class FakeConfig:
    """Minimal ConfigManager stand-in exposing only get_component_identity()."""

    def __init__(self, identity=True):
        self._identity = (
            MessageIdentity([HierEntry("device", "test-thing")], "comp")
            if identity
            else None
        )

    def get_component_identity(self):
        return self._identity

    def set_component_identity(self, identity):
        self._identity = identity


class FakeMessaging:
    """Records subscribe/unsubscribe calls and can simulate an inbound message."""

    def __init__(self):
        self._callbacks = {}

    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None):
        self._callbacks[topic] = callback

    def unsubscribe(self, topic):
        self._callbacks.pop(topic, None)

    def subscribed_topics(self):
        return set(self._callbacks.keys())

    def simulate_message(self, topic, message):
        callback = self._callbacks.get(topic)
        if callback is not None:
            callback(topic, message)


class RecordingDelayer:
    """Records (task, delay_millis) pairs; run_all() runs+clears them synchronously —
    the test's "the jitter delay elapsed" step."""

    def __init__(self):
        self.tasks = []
        self.delays = []

    def __call__(self, task, delay_millis):
        self.tasks.append(task)
        self.delays.append(delay_millis)

    def run_all(self):
        to_run = list(self.tasks)
        self.tasks.clear()
        self.delays.clear()
        for task in to_run:
            task()


class Clock:
    def __init__(self):
        self.now_ms = 0

    def __call__(self):
        return self.now_ms


class FixedJitter:
    def __init__(self, value=0):
        self.value = value
        self.window_seen = None

    def __call__(self, window_ms):
        self.window_seen = window_ms
        return self.value


class Counter:
    def __init__(self):
        self.value = 0

    def increment(self):
        self.value += 1


def _broadcast(verb):
    return MessageBuilder.create(verb, "1.0").with_payload({}).build()


class Harness:
    def __init__(self):
        self.config = FakeConfig()
        self.messaging = FakeMessaging()
        self.delayer = RecordingDelayer()
        self.clock = Clock()
        self.jitter = FixedJitter(0)
        self.state_count = Counter()
        self.cfg_count = Counter()
        self.listener = RepublishListener(
            self.config,
            self.messaging,
            self.state_count.increment,
            self.cfg_count.increment,
            delayer=self.delayer,
            clock_millis=self.clock,
            jitter=self.jitter,
        )


@pytest.fixture
def h():
    return Harness()


def test_start_subscribes_both_own_device_bcast_topics(h):
    h.listener.start()
    assert h.messaging.subscribed_topics() == {STATE_TOPIC, CFG_TOPIC}, (
        "start() must subscribe exactly the two own-device _bcast republish topics"
    )


def test_republish_state_re_emits_the_state_keepalive(h):
    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert h.state_count.value == 0, "the re-announce must wait for the jitter delay"
    h.delayer.run_all()
    assert h.state_count.value == 1, "republish-state must re-run the state action"
    assert h.cfg_count.value == 0, "republish-state must not touch the cfg action"


def test_republish_cfg_re_runs_the_effective_config_publisher(h):
    h.listener.start()
    h.messaging.simulate_message(CFG_TOPIC, _broadcast("republish-cfg"))
    h.delayer.run_all()
    assert h.cfg_count.value == 1, "republish-cfg must re-run the cfg action"
    assert h.state_count.value == 0, "republish-cfg must not touch the state action"


def test_jitter_window_is_applied_to_the_scheduled_delay(h):
    h.jitter.value = 1234
    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert h.jitter.window_seen == RepublishListener.JITTER_WINDOW_MS, (
        "the jitter source must be asked for a delay within the normative window"
    )
    assert h.delayer.delays == [1234], (
        "the scheduled delay must be exactly the jittered value"
    )


def test_broadcasts_coalesce_while_a_reannounce_is_pending(h):
    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert len(h.delayer.tasks) == 1, (
        "a looping broadcast must coalesce to a single pending re-announce"
    )
    h.delayer.run_all()
    assert h.state_count.value == 1


def test_broadcasts_coalesce_within_the_cooldown_and_accept_after_it(h):
    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    h.delayer.run_all()  # fired; cooldown runs from the ACCEPTED trigger at t=0

    h.clock.now_ms = RepublishListener.COOLDOWN_MS - 1
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert not h.delayer.tasks, "a broadcast inside the cooldown must coalesce"
    assert h.state_count.value == 1

    h.clock.now_ms = RepublishListener.COOLDOWN_MS
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert len(h.delayer.tasks) == 1, "the cooldown boundary must accept again"
    h.delayer.run_all()
    assert h.state_count.value == 2


def test_the_verbs_rate_limit_independently(h):
    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    # With a state re-announce pending, a cfg broadcast must still be accepted.
    h.messaging.simulate_message(CFG_TOPIC, _broadcast("republish-cfg"))
    assert len(h.delayer.tasks) == 2, "state and cfg coalesce/cooldown independently"
    h.delayer.run_all()
    assert h.state_count.value == 1
    assert h.cfg_count.value == 1


def test_foreign_and_malformed_payloads_are_ignored(h):
    h.listener.start()
    # Wrong verb name in the header (foreign command on the topic).
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("something-else"))
    # A raw (headerless) envelope - e.g. junk JSON published on the broadcast topic.
    h.messaging.simulate_message(STATE_TOPIC, Message.from_object({}))
    # A None message must not crash the callback either.
    h.messaging.simulate_message(STATE_TOPIC, None)
    assert not h.delayer.tasks, "foreign/malformed payloads must never schedule"
    assert h.state_count.value == 0
    assert h.cfg_count.value == 0


def _boom():
    raise RuntimeError("boom")


def test_a_failing_reannounce_is_swallowed_and_does_not_wedge_the_verb(h):
    failing = RepublishListener(
        h.config,
        h.messaging,
        _boom,
        h.cfg_count.increment,
        delayer=h.delayer,
        clock_millis=h.clock,
        jitter=lambda window: 0,
    )
    failing.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    h.delayer.run_all()  # must not raise
    # After the cooldown the verb accepts again (pending was cleared despite the failure).
    h.clock.now_ms = RepublishListener.COOLDOWN_MS
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert len(h.delayer.tasks) == 1
    failing.close()


def test_close_unsubscribes_both_topics_and_drops_pending_reannounces(h):
    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    h.listener.close()
    assert not h.messaging.subscribed_topics(), (
        "close() must unsubscribe both _bcast topics (unsubscribe-before-exit)"
    )
    h.delayer.run_all()
    assert h.state_count.value == 0, "a pending re-announce must not fire after close()"
    # And a late broadcast (e.g. a stale queued delivery) is ignored.
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert not h.delayer.tasks


def test_close_is_idempotent_and_start_after_close_is_a_noop(h):
    h.listener.start()
    h.listener.close()
    h.listener.close()  # must not raise
    h.listener.start()  # closed -> must not resubscribe
    assert not h.messaging.subscribed_topics()


def test_start_is_idempotent(h):
    h.listener.start()
    h.listener.start()
    assert h.messaging.subscribed_topics() == {STATE_TOPIC, CFG_TOPIC}
    h.messaging.simulate_message(STATE_TOPIC, _broadcast("republish-state"))
    assert len(h.delayer.tasks) == 1, "a double start must not double-schedule"


def test_missing_identity_disables_the_listener(h):
    h.config.set_component_identity(None)  # the mock/test bring-up case
    h.listener.start()
    assert not h.messaging.subscribed_topics(), (
        "no resolved identity -> no _bcast subscriptions (WARN + disabled)"
    )
    h.listener.close()  # must not raise


def test_start_failure_disables_the_listener_without_raising(h):
    def boom_subscribe(topic, callback, max_concurrency=None, max_messages=None):
        raise RuntimeError("broker unavailable")

    h.messaging.subscribe = boom_subscribe
    h.listener.start()  # must not raise; the listener self-disables
    assert not h.messaging.subscribed_topics()
    # No re-announce is possible since nothing was subscribed.
    h.listener.close()  # must not raise


def test_handle_swallows_an_exception_from_a_malformed_message(h):
    class ExplodingMessage:
        def get_header(self):
            raise RuntimeError("corrupt payload")

    h.listener.start()
    h.messaging.simulate_message(STATE_TOPIC, ExplodingMessage())  # must not raise
    assert not h.delayer.tasks
    assert h.state_count.value == 0


def test_on_broadcast_is_a_noop_when_closed_mid_flight(h):
    # The race the internal `closed` check under the lock guards: a message that
    # slipped through before unsubscribe took effect must still not schedule.
    h.listener.start()
    h.listener.close()
    h.listener._on_broadcast(h.listener._commands[0])
    assert not h.delayer.tasks
    assert h.state_count.value == 0


def test_close_swallows_an_unsubscribe_failure(h):
    def boom_unsubscribe(topic):
        raise RuntimeError("already gone")

    h.listener.start()
    h.messaging.unsubscribe = boom_unsubscribe
    h.listener.close()  # must not raise despite both unsubscribe calls failing


class TestConstructorValidation:
    def test_none_config_raises(self):
        with pytest.raises(ValueError):
            RepublishListener(None, FakeMessaging(), lambda: None, lambda: None)

    def test_none_messaging_raises(self):
        with pytest.raises(ValueError):
            RepublishListener(FakeConfig(), None, lambda: None, lambda: None)

    def test_none_state_action_raises(self):
        with pytest.raises(ValueError):
            RepublishListener(FakeConfig(), FakeMessaging(), None, lambda: None)

    def test_none_cfg_action_raises(self):
        with pytest.raises(ValueError):
            RepublishListener(FakeConfig(), FakeMessaging(), lambda: None, None)


class TestProductionWiring:
    """The production defaults (no injected delayer/clock/jitter): exercised
    synchronously so the tests stay deterministic and sleep-free."""

    def test_default_jitter_is_within_the_normative_window(self):
        listener = RepublishListener(
            FakeConfig(), FakeMessaging(), lambda: None, lambda: None
        )
        try:
            for _ in range(50):
                delay = listener._jitter(RepublishListener.JITTER_WINDOW_MS)
                assert 0 <= delay <= RepublishListener.JITTER_WINDOW_MS
        finally:
            listener.close()

    def test_default_clock_is_monotonic_nondecreasing(self):
        listener = RepublishListener(
            FakeConfig(), FakeMessaging(), lambda: None, lambda: None
        )
        try:
            first = listener._clock_millis()
            second = listener._clock_millis()
            assert second >= first
        finally:
            listener.close()

    def test_threading_delayer_runs_task_and_close_cancels_pending(self):
        delayer = _ThreadingDelayer()
        done = threading.Event()
        delayer(done.set, 0)
        assert done.wait(timeout=2), "the production delayer must run the scheduled task"
        # A never-fired timer is cancelled by close() (best-effort; no exception).
        delayer(lambda: None, 60_000)
        delayer.close()

    def test_production_listener_uses_owned_delayer(self):
        listener = RepublishListener(
            FakeConfig(), FakeMessaging(), lambda: None, lambda: None
        )
        try:
            assert isinstance(listener._delayer, _ThreadingDelayer)
            assert listener._owned_delayer is listener._delayer
        finally:
            listener.close()  # must shut down the owned delayer without raising
