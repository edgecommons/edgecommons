"""What this sink reports about its destinations, plus the library-facing wiring's testable seams.

`test_dest.py` covers the pure destination/retry/health logic; this covers what the app wires into
the library: the instance-connectivity provider, the sink parsing in `__init__`, the inbound
handler's key/queue/drop, the deliver→verify→retry state machine and its event ladder, the
exhaustion path, the metric flush, and stop. The app is handed the framework facade, so recording
stand-ins for it are enough — no broker, no transport, no threads (the two infinite worker loops,
`run()`/`_run_sink()`, are the live-runtime seam, validated by the HOST/GREENGRASS smoke). Run them
with `pytest`.
"""
import queue

import pytest

from app.dest import (
    BACKOFF,
    CONNECTING,
    FAILED,
    ONLINE,
    DeliverError,
    Delivered,
    Item,
    RetryPolicy,
    SinkConfig,
)
from app.<<COMPONENTNAME>> import METRIC_NAME, Stats, <<COMPONENTNAME>>


# --- the framework stand-ins: they record what the app emits ------------------------------------


class FakeHeader:
    def __init__(self, uuid="uuid-1"):
        self.uuid = uuid


class FakeMsg:
    def __init__(self, body=None, uuid="uuid-1"):
        self._header = FakeHeader(uuid)
        self._body = body if body is not None else {"v": 1}

    def get_header(self):
        return self._header

    def get_body(self):
        return self._body


class RecordingEvents:
    def __init__(self, sink):
        self._sink = sink

    def emit(self, type, message, context, severity=None):
        self._sink.events.append((type, context))

    def raise_alarm(self, type, message, context, severity=None):
        self._sink.alarms.append((type, context))


class RecordingInstance:
    def __init__(self, sink):
        self._sink = sink

    def events(self):
        return RecordingEvents(self._sink)


class FakeGg:
    """Records the app's registrations, events, alarms and metrics; getters are otherwise inert."""

    DEFAULT_SINKS = {
        "archive": {
            "id": "archive",
            "subscribe": "ecv1/+/+/+/data/#",
            "destination": {"type": "local", "path": "/var/lib/out"},
            # A tiny backoff keeps the retry-path test instant (the delay is random in [0, ~1ms)).
            "retry": {"baseDelayMs": 1, "maxDelayMs": 1},
        },
        "audit": {
            "id": "audit",
            "subscribe": "ecv1/+/+/+/evt/#",
            "destination": {"type": "local", "path": "/var/lib/audit"},
        },
    }

    def __init__(self, sinks=None, unsubscribe_raises=False, metric_raises=False):
        self._sinks = sinks if sinks is not None else self.DEFAULT_SINKS
        self._unsubscribe_raises = unsubscribe_raises
        self._metric_raises = metric_raises
        self.connectivity_provider = None
        self.events = []
        self.alarms = []
        self.metrics = []
        self.unsubscribed = []
        self.ready = False

    # config manager
    def get_config_manager(self):
        return self

    def add_config_change_listener(self, listener):
        pass

    def get_global_config(self):
        return {}

    def get_instance_ids(self):
        return list(self._sinks)

    def get_instance_config(self, instance_id):
        return self._sinks[instance_id]

    # messaging
    def get_messaging(self):
        return self

    def unsubscribe(self, subscribe):
        if self._unsubscribe_raises:
            raise RuntimeError("already gone")
        self.unsubscribed.append(subscribe)

    # metrics
    def get_metrics(self):
        return self

    def define_metric(self, metric):
        pass

    def emit_metric(self, name, values):
        if self._metric_raises:
            raise RuntimeError("metrics sink down")
        self.metrics.append((name, values))

    # instance facade + lifecycle
    def instance(self, instance_id):
        return RecordingInstance(self)

    def set_instance_connectivity_provider(self, provider):
        self.connectivity_provider = provider

    def set_ready(self, ready):
        self.ready = ready


class ScriptedDestination:
    """A destination whose deliver()/verify() are scripted: a queue of outcomes, each either None
    (succeed) or a DeliverError to raise. `verify` always passes once `deliver` returned."""

    def __init__(self, outcomes):
        self._outcomes = list(outcomes)
        self.delivered_count = 0

    def kind(self):
        return "scripted"

    def deliver(self, item):
        outcome = self._outcomes.pop(0) if self._outcomes else None
        if isinstance(outcome, DeliverError):
            raise outcome
        self.delivered_count += 1
        return Delivered(len(item.data))

    def verify(self, item, delivered):
        return None


def _item():
    return Item("archive/temp/uuid-1.json", b"{}")


# --- instance connectivity (existing coverage) --------------------------------------------------


def test_every_configured_destination_is_reported_before_anything_is_delivered():
    # A sink's destinations ARE its instances, and one that is configured but not delivering must
    # never be indistinguishable from one that was never configured. ONE provider, TWO surfaces: the
    # library pushes this into every `state` keepalive's instances[] AND returns it from the
    # built-in `status` verb, so a console that subscribes and one that asks cannot disagree.
    gg = FakeGg()

    app = <<COMPONENTNAME>>(gg)

    assert gg.connectivity_provider is not None
    reported = app.instance_connectivity()
    assert [c.instance for c in reported] == ["archive", "audit"]
    assert all(c.connected is False for c in reported), "nothing has been verified yet"
    assert all(c.state == CONNECTING for c in reported)
    assert reported[0].attributes == {"destination": "local"}


def test_a_destinations_condition_reaches_the_wire_element():
    # ONLINE only after a delivery is verified; BACKOFF and FAILED are both connected=false and stay
    # tellable apart -- still trying is not the same as gave up.
    app = <<COMPONENTNAME>>(FakeGg())

    app.health["archive"].delivered("archive/temp/uuid-1.json")
    app.health["audit"].retrying("transient: connection reset")
    archive, audit = app.instance_connectivity()

    assert (archive.connected, archive.state) == (True, ONLINE)
    assert (audit.connected, audit.state) == (False, BACKOFF)

    app.health["audit"].failed("permanent: bad credentials")
    assert app.instance_connectivity()[1].state == FAILED


# --- __init__ route parsing + config-change ------------------------------------------------------


def test_a_malformed_sink_is_skipped_but_the_valid_ones_still_run():
    gg = FakeGg(sinks={
        "good": {"id": "good", "subscribe": "t", "destination": {"type": "local", "path": "/o"}},
        "bad": {"id": "bad", "subscribe": "t", "destination": {"type": "nowhere"}},
    })
    app = <<COMPONENTNAME>>(gg)
    assert [s.id for s in app._sinks] == ["good"]


def test_a_component_with_no_valid_sinks_fails_loudly_rather_than_idling():
    gg = FakeGg(sinks={"bad": {"id": "bad", "subscribe": "t", "destination": {"type": "nowhere"}}})
    with pytest.raises(ValueError, match="no valid sinks"):
        <<COMPONENTNAME>>(gg)


def test_a_config_change_is_accepted_by_the_listener():
    app = <<COMPONENTNAME>>(FakeGg())
    assert app.on_configuration_change({"component": {"global": {}}}) is True


# --- Stats ---------------------------------------------------------------------------------------


def test_stats_count_and_reset_on_swap():
    stats = Stats()
    stats.incr("received", 2)
    stats.incr("exhausted")
    snap = stats.swap()
    assert snap == {"received": 2.0, "delivered": 0.0, "retried": 0.0,
                    "exhausted": 1.0, "dropped": 0.0}
    assert stats.swap()["received"] == 0.0


# --- the inbound handler -------------------------------------------------------------------------


def test_the_handler_keys_the_item_and_drops_on_a_full_queue():
    app = <<COMPONENTNAME>>(FakeGg())
    sink = app._sinks[0]
    q = queue.Queue(maxsize=1)
    on_message = app._handler(sink, q)

    on_message("ecv1/gw/x/main/data/temp", FakeMsg({"v": 1}, uuid="u1"))
    assert q.qsize() == 1
    item = q.get()
    assert item.key == "archive/temp/u1.json", "a stable, deterministic key"

    # Refill, then overflow: the next arrival must DROP and be COUNTED, never block.
    on_message("t", FakeMsg(uuid="u2"))
    on_message("t", FakeMsg(uuid="u3"))
    snap = app._stats.swap()
    assert snap["received"] == 3.0
    assert snap["dropped"] == 1.0


# --- the deliver -> verify -> retry state machine + event ladder ---------------------------------


def test_a_verified_delivery_reports_online_and_completes_the_event_ladder():
    app = <<COMPONENTNAME>>(FakeGg())
    sink = app._sinks[0]

    app._deliver_with_retry(sink, _item(), ScriptedDestination([None]))

    assert app.health["archive"].state == ONLINE
    assert app._stats.swap()["delivered"] == 1.0
    assert [e[0] for e in app._gg.events] == ["delivery-started", "delivery-completed"]


def test_a_permanent_failure_exhausts_immediately_and_raises_a_critical_alarm():
    app = <<COMPONENTNAME>>(FakeGg())
    sink = app._sinks[0]
    dest = ScriptedDestination([DeliverError.permanent_failure("bad credentials")])

    app._deliver_with_retry(sink, _item(), dest)

    assert app.health["archive"].state == FAILED
    assert app._stats.swap()["exhausted"] == 1.0
    assert [a[0] for a in app._gg.alarms] == ["delivery-exhausted"]


def test_a_transient_failure_is_retried_then_succeeds():
    app = <<COMPONENTNAME>>(FakeGg())
    sink = app._sinks[0]  # baseDelayMs=1/maxDelayMs=1 -> backoff is ~0ms
    dest = ScriptedDestination([DeliverError.transient_failure("connection reset"), None])

    app._deliver_with_retry(sink, _item(), dest)

    assert app.health["archive"].state == ONLINE
    snap = app._stats.swap()
    assert snap["retried"] == 1.0 and snap["delivered"] == 1.0
    kinds = [e[0] for e in app._gg.events]
    assert kinds == ["delivery-started", "delivery-failed", "delivery-completed"]


def test_a_spent_time_budget_exhausts_a_transient_failure():
    app = <<COMPONENTNAME>>(FakeGg())
    # A zero budget: the first transient failure has already spent it.
    sink = SinkConfig("archive", "t", {"type": "local", "path": "/o"},
                      RetryPolicy(base_delay_ms=1, max_delay_ms=1, give_up_after_ms=0), 8)
    dest = ScriptedDestination([DeliverError.transient_failure("timeout")])

    app._deliver_with_retry(sink, _item(), dest)

    assert app.health["archive"].state == FAILED
    assert app._stats.swap()["exhausted"] == 1.0


def test_a_shutdown_mid_retry_stops_without_delivering():
    app = <<COMPONENTNAME>>(FakeGg())
    sink = app._sinks[0]
    app._stop.set()  # a shutdown is already in flight
    dest = ScriptedDestination([DeliverError.transient_failure("connection reset"), None])

    app._deliver_with_retry(sink, _item(), dest)

    # It backed off, saw the stop, and returned before the (would-be successful) second attempt.
    assert dest.delivered_count == 0
    assert app.health["archive"].state == BACKOFF


# --- metric flush + stop -------------------------------------------------------------------------


def test_emit_metrics_flushes_the_counters_and_survives_a_sink_outage():
    app = <<COMPONENTNAME>>(FakeGg())
    app._stats.incr("delivered", 3)
    app._emit_metrics()
    assert app._gg.metrics[-1][0] == METRIC_NAME
    assert app._gg.metrics[-1][1]["delivered"] == 3.0

    app2 = <<COMPONENTNAME>>(FakeGg(metric_raises=True))
    app2._emit_metrics()  # a metric-sink outage must not propagate


def test_stop_is_idempotent_and_survives_a_failed_unsubscribe():
    gg = FakeGg()
    app = <<COMPONENTNAME>>(gg)
    app.stop()
    assert sorted(gg.unsubscribed) == ["ecv1/+/+/+/data/#", "ecv1/+/+/+/evt/#"]
    app.stop()  # second stop is a no-op
    assert sorted(gg.unsubscribed) == ["ecv1/+/+/+/data/#", "ecv1/+/+/+/evt/#"]

    app2 = <<COMPONENTNAME>>(FakeGg(unsubscribe_raises=True))
    app2.stop()  # shutdown must not raise even if a subscription is already gone
