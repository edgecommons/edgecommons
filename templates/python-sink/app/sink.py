"""<<COMPONENTNAME>> -- a sink component.

A **sink** is the last thing standing between data and its destination. It consumes work, delivers it
outward, and only then lets go of the source.

.. code-block:: text

    consume --> deliver (idempotent, stable key) --> verify --> confirm --> report
                        ^                                                     |
                        +---------- retry with full jitter <------------------+

The ordering is the archetype, and every step earns its place:

* **Deliver idempotently, to a stable key.** A redelivery overwrites; it does not duplicate. A sink
  that cannot retry without duplicating cannot retry at all.
* **Verify before you confirm.** Trusting that `deliver` returned and releasing the source without
  checking what actually landed is how you end up having deleted the only copy.
* **Classify the failure.** Retrying a permanent error burns the budget; giving up on a transient one
  loses data a second attempt would have delivered. See :class:`~app.dest.DeliverError`.
* **Report every transition.** A sink that fails quietly is indistinguishable from one that is idle.
  Started / completed / failed / exhausted all go out on the UNS event surface.
* **Report each destination's health.** A sink's destinations *are* its instances:
  :meth:`~<<COMPONENTNAME>>.instance_connectivity` reports one entry per configured destination —
  the same sample the ``state`` keepalive pushes and the built-in ``status`` verb returns.

Where the work comes from
-------------------------
This scaffold's source is a **subscription**: it consumes messages off the bus and delivers each one.
That is the common case. If your source is a watched directory or a polled API, replace the subscribe
call in :meth:`run` -- everything downstream of ``_deliver_with_retry`` is unchanged, which is the
point of the seam.
"""
import json
import logging
import queue
import threading
import time

from edgecommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from edgecommons.facades import Severity
from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
from edgecommons.metrics.metric_builder import MetricBuilder

from app.dest import DeliverError, DestinationHealth, Item, key_for, parse_sink

logger = logging.getLogger("<<COMPONENTNAME>>")

#: The metric this component emits each interval.
METRIC_NAME = "sinkDeliveries"
#: How often the counters below are flushed as a metric, in seconds.
METRIC_INTERVAL_SECS = 60


class Stats:
    """Counters, reported as a metric each interval.

    ``exhausted`` is the number that matters: it is data that did not arrive.
    """

    MEASURES = ("received", "delivered", "retried", "exhausted", "dropped")

    def __init__(self):
        self._lock = threading.Lock()
        self._counts = {m: 0 for m in self.MEASURES}

    def incr(self, measure: str, n: int = 1) -> None:
        with self._lock:
            self._counts[measure] += n

    def swap(self) -> dict:
        with self._lock:
            snapshot = {k: float(v) for k, v in self._counts.items()}
            self._counts = {m: 0 for m in self.MEASURES}
        return snapshot


class <<COMPONENTNAME>>(ConfigurationChangeListener):
    """One sink per ``component.instances[]`` entry; one worker thread per sink."""

    def __init__(self, gg):
        self._gg = gg
        self._cm = gg.get_config_manager()
        self._cm.add_config_change_listener(self)
        self._messaging = gg.get_messaging()
        self._metrics = gg.get_metrics()
        self._stats = Stats()
        self._stop = threading.Event()
        self._threads = []

        self._metrics.define_metric(
            MetricBuilder.create(METRIC_NAME)
            .with_config(self._cm)
            .add_measure("received", "Count", 60)
            .add_measure("delivered", "Count", 60)
            .add_measure("retried", "Count", 60)
            .add_measure("exhausted", "Count", 60)
            .add_measure("dropped", "Count", 60)
            .build()
        )

        defaults = (self._cm.get_global_config() or {}).get("defaults", {})
        self._sinks = []
        for instance_id in self._cm.get_instance_ids():
            try:
                self._sinks.append(parse_sink(self._cm.get_instance_config(instance_id) or {}, defaults))
            except ValueError as e:
                logger.warning("skipping malformed sink `%s`: %s", instance_id, e)
        if not self._sinks:
            raise ValueError("no valid sinks in component.instances[]")

        # A sink's destinations ARE its instances. One health per configured destination, built
        # before a single message arrives, so a destination that is configured and unreachable is
        # reported (connected=false / CONNECTING) rather than absent.
        self.health = {s.id: DestinationHealth(s.id, s.destination.get("type")) for s in self._sinks}

        # --- instance connectivity: ONE provider, TWO surfaces. Whatever it returns is pushed into
        # the `state` keepalive's instances[] on every tick AND returned by the built-in `status`
        # verb when a console asks — so whoever watches and whoever asks can never get different
        # answers.
        gg.set_instance_connectivity_provider(self.instance_connectivity)

    def instance_connectivity(self) -> list:
        """One entry per configured destination.

        ``connected`` is the **normalized** flag — true only once a delivery has been *verified*, so
        a console renders a health dot without knowing what this sink delivers to. ``state`` is this
        sink's own vocabulary, and it is what separates ``BACKOFF`` (still trying) from ``FAILED``
        (gave up; data did not arrive) — both are ``connected=False``, and they are not the same
        thing. ``attributes`` is the open bag for domain data, so what only this sink understands
        never destabilizes the fields everyone reads.
        """
        return [
            InstanceConnectivity.of(h.sink_id, h.connected, h.detail)
            .with_state(h.state)
            .with_attributes({"destination": h.kind})
            for h in self.health.values()
        ]

    def on_configuration_change(self, configuration) -> bool:
        logger.info("configuration changed")
        return True

    def run(self):  # pragma: no cover - live-runtime seam: subscribes over a real transport, spawns the per-sink worker threads, and blocks on the metric-flush loop until shutdown; exercised by the HOST/GREENGRASS smoke, not offline unit tests (edgecommons AGENTS.md validation matrix). The testable pieces it calls (_handler, _deliver_with_retry, _emit_metrics) have their own tests.
        """Subscribe every sink, start its worker, then flush metrics until shutdown."""
        for sink in self._sinks:
            destination = sink.build_destination()
            q = queue.Queue(maxsize=sink.max_queue)

            # max_concurrency=1: this sink's messages are dispatched in order onto its own queue.
            self._messaging.subscribe(sink.subscribe, self._handler(sink, q), 1, sink.max_queue)
            logger.info("[%s] subscribed: %s -> %s", sink.id, sink.subscribe, destination.kind())

            t = threading.Thread(
                target=self._run_sink, args=(sink, q, destination), name=f"sink-{sink.id}", daemon=True
            )
            t.start()
            self._threads.append(t)

        self._gg.set_ready(True)
        while not self._stop.wait(METRIC_INTERVAL_SECS):
            self._emit_metrics()
        self._emit_metrics()

    def stop(self):
        """Stop the sinks and drop their subscriptions. Idempotent."""
        if self._stop.is_set():
            return
        self._stop.set()
        for sink in self._sinks:
            try:
                self._messaging.unsubscribe(sink.subscribe)
            except Exception as e:  # noqa: BLE001 - shutdown must not raise
                logger.debug("[%s] unsubscribe failed: %s", sink.id, e)
        for t in self._threads:
            t.join(timeout=5)

    # --- the inbound seam ------------------------------------------------------------------------

    def _handler(self, sink, q: "queue.Queue"):
        def on_message(topic, msg):
            self._stats.incr("received")
            item = Item(
                # A stable, deterministic key: the same message always lands in the same place, so a
                # redelivery overwrites.
                key=key_for(sink.id, topic, msg.get_header().uuid),
                data=json.dumps(msg.get_body()).encode("utf-8"),
            )
            try:
                # put_nowait, never put: a full queue must DROP and be COUNTED, not block the
                # transport's dispatch thread.
                q.put_nowait(item)
            except queue.Full:
                self._stats.incr("dropped")
                logger.warning("[%s] queue full; dropped %s", sink.id, item.key)

        return on_message

    # --- one sink's thread -----------------------------------------------------------------------

    def _run_sink(self, sink, q: "queue.Queue", destination):  # pragma: no cover - live-runtime seam: the per-sink worker's infinite queue-drain loop, driven by real inbound traffic; exercised by the HOST/GREENGRASS smoke, not offline unit tests. The delivery/retry state machine it delegates to (_deliver_with_retry) is unit-tested directly.
        while not self._stop.is_set():
            try:
                item = q.get(timeout=1.0)
            except queue.Empty:
                continue
            self._deliver_with_retry(sink, item, destination)
        logger.info("[%s] sink stopped", sink.id)

    def _deliver_with_retry(self, sink, item: Item, destination):
        """Deliver one item, retrying transient failures until the time budget is spent.

        The event ladder is the sink's contract with whoever is watching: **started**, then either
        **completed**, or **failed** (with ``willRetry``), and finally **exhausted** if the budget
        runs out. An operator must be able to tell "still trying" from "gave up", and gave-up must be
        loud.
        """
        events = self._gg.instance(sink.id).events()
        health = self.health[sink.id]
        started = time.monotonic()
        attempt = 0

        events.emit(
            "delivery-started",
            None,
            {"sink": sink.id, "key": item.key, "kind": destination.kind()},
            severity=Severity.INFO,
        )

        while True:
            try:
                # deliver, then VERIFY. Only a verified delivery is a delivery.
                delivered = destination.deliver(item)
                destination.verify(item, delivered)
            except DeliverError as e:
                elapsed_ms = (time.monotonic() - started) * 1000.0

                # Permanent: it will fail identically forever. Retrying is a waste of the budget and
                # of the log; give up now and say so.
                if not e.transient:
                    self._exhausted(sink, item, attempt, f"{sink.id} will never deliver {item.key}", e, events)
                    return

                if sink.retry.budget_spent(elapsed_ms):
                    self._exhausted(sink, item, attempt, f"{sink.id} gave up on {item.key}", e, events)
                    return

                backoff_ms = sink.retry.delay_ms(attempt)
                # Still trying — BACKOFF, not FAILED. Both are connected=false; only one of them
                # means data was lost.
                health.retrying(str(e))
                self._stats.incr("retried")
                logger.warning(
                    "[%s] transient failure on %s (attempt %d, retrying in %dms): %s",
                    sink.id, item.key, attempt + 1, backoff_ms, e,
                )
                events.emit(
                    "delivery-failed",
                    str(e),
                    {
                        "sink": sink.id, "key": item.key, "attempt": attempt + 1,
                        "willRetry": True, "nextAttemptInMs": backoff_ms,
                    },
                    severity=Severity.WARNING,
                )
                # Waiting on the stop event, not sleeping: a shutdown must not be held hostage by a
                # 15-minute backoff.
                if self._stop.wait(backoff_ms / 1000.0):
                    logger.info("[%s] shutting down mid-retry; %s not delivered", sink.id, item.key)
                    return
                attempt += 1
                continue

            # Verified, and only now: ONLINE is reported on the same proof the source is released
            # on. A destination is not healthy because `deliver` returned.
            health.delivered(item.key)
            self._stats.incr("delivered")
            events.emit(
                "delivery-completed",
                None,
                {
                    "sink": sink.id, "key": item.key, "attempts": attempt + 1,
                    "elapsedMs": int((time.monotonic() - started) * 1000.0),
                },
                severity=Severity.INFO,
            )
            # The source is released HERE -- after verification, never before.
            return

    def _exhausted(self, sink, item: Item, attempt: int, message: str, error: Exception, events):
        # Gave up: permanent, or the budget is spent. FAILED, not BACKOFF — a retry still in flight
        # and data that did not arrive must not look alike on the console.
        self.health[sink.id].failed(str(error))
        self._stats.incr("exhausted")
        logger.error("[%s] %s: %s", sink.id, message, error)
        # Critical, and an alarm rather than a one-shot event: this is data that did not arrive, and
        # an operator has to see it.
        events.raise_alarm(
            "delivery-exhausted",
            message,
            {"sink": sink.id, "key": item.key, "attempts": attempt + 1, "reason": str(error)},
            severity=Severity.CRITICAL,
        )

    def _emit_metrics(self):
        try:
            self._metrics.emit_metric(METRIC_NAME, self._stats.swap())
        except Exception as e:  # noqa: BLE001
            logger.warning("metric emit failed: %s", e)
