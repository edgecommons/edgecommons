"""Operational metrics — the canonical ``southbound_health`` + the operational-family pattern.

Every southbound adapter emits the shared :data:`HEALTH` metric with **exactly** the SOUTHBOUND.md
§5 measure set. On top of that, this module ships the **operational-family pattern** two protocols
deep as worked examples — :data:`CONNECTION` and :data:`COMMAND` — and shows you where to add your
own.

## What ``<<COMPONENTNAME>>`` emits today

| Metric | Dimensions | What it is |
|---|---|---|
| ``southbound_health`` | ``instance`` | the §5 canonical set (below) — every adapter emits this |
| ``<<COMPONENTNAME>>Connection`` | ``instance`` | the connect/reconnect lifecycle |
| ``<<COMPONENTNAME>>Command`` | ``instance``, ``verb``, ``result`` | the ``sb/*`` command surface |

## The Total/Interval counter convention

Every **counter** is emitted as a measure PAIR: ``<name>Total`` (monotonic since start) and
``<name>Interval`` (since the previous emit of that family; **reset on emit** — see :class:`_Pair`).
**Gauges** (``connectionState``) and interval **sums** (the ``*Ms`` latencies/durations) are single
measures. This is the same convention ``modbus-adapter`` and ``ethernet-ip-adapter`` use, so a fleet
dashboard reads every adapter the same way.

## Dimensions are LOW-CARDINALITY only

``instance``, ``verb`` (the closed :data:`COMMAND_VERBS` set), and ``result``
(``success`` | ``error``) — and nothing else. **Never** dimension by signal name, address, endpoint,
or error text: those are unbounded and would shred a fleet dashboard. (``coreName``/``category``/
``component`` are injected by ``MetricBuilder.build``.)

## Add your protocol's families HERE

``<<COMPONENTNAME>>Connection``/``Command`` are generic — every adapter has them. Your protocol also
has an **inventory** (configured signals), a **poll/subscribe** path, and a **publish** path worth
measuring. Add ``<<COMPONENTNAME>>Inventory`` / ``<<COMPONENTNAME>>Poll`` / ``<<COMPONENTNAME>>Publish``
families next to the two below — see ``modbus-adapter/modbus_adapter/metrics.py`` and
``ethernet-ip-adapter/crates/ethernet-ip-adapter/src/metrics.rs`` for the full worked set (poll
cycles, samples good/bad/uncertain/changed/suppressed, batch flushes, …). Register each new family in
:func:`family_defs` and pre-define it in :meth:`DeviceMetrics.define_all`; the rest of the pattern
(record -> drain -> emit) is copy-shaped from the command family.
"""
import threading
import time
from dataclasses import dataclass
from typing import Any, Dict, List, Tuple

from edgecommons.metrics.metric_builder import MetricBuilder

#: The metric every southbound adapter emits (SOUTHBOUND.md §5).
HEALTH = "southbound_health"
#: The worked operational family for the connect/reconnect lifecycle. Named from the component so a
#: fleet view can tell one adapter's connection health from another's.
CONNECTION = "<<COMPONENTNAME>>Connection"
#: The worked operational family for the ``sb/*`` command surface (``instance``×``verb``×``result``).
COMMAND = "<<COMPONENTNAME>>Command"

#: A ``result`` dimension value: the operation succeeded.
RESULT_SUCCESS = "success"
#: A ``result`` dimension value: the operation failed.
RESULT_ERROR = "error"
_RESULTS = (RESULT_SUCCESS, RESULT_ERROR)

#: The **closed** ``verb`` dimension set for :data:`COMMAND` — every ``sb/*`` verb the command surface
#: registers (``command_service.py``). Closed and low-cardinality on purpose (see the module header).
COMMAND_VERBS = (
    "sb/status", "sb/read", "sb/write", "sb/signals", "sb/browse", "sb/pause", "sb/resume",
    "reconnect", "repoll",
)

#: The **exact** SOUTHBOUND.md §5 measure set of ``southbound_health`` — ``connectionState``,
#: ``publishLatencyMs``, ``pollLatencyMs``, ``readErrors``, ``staleSignals``, plus the §5-optional
#: ``reconnects``. This literal list is the parity anchor the metrics test asserts against; if you
#: change what :meth:`DeviceMetrics._emit_health` emits, this list and :func:`family_defs` must move
#: with it.
HEALTH_MEASURES = (
    "connectionState", "publishLatencyMs", "pollLatencyMs", "readErrors", "staleSignals",
    "reconnects",
)

_UNIT_COUNT = "Count"
_UNIT_MS = "Milliseconds"


# =================================================================================================
# The definition schema — the single source the startup pre-definition and the parity test both read
# =================================================================================================

@dataclass(frozen=True)
class MeasureDef:
    """One measure's name, unit, and storage resolution."""

    name: str
    unit: str
    res: int


@dataclass(frozen=True)
class FamilyDef:
    """One metric family's full definition: its name, dimension keys, and measures."""

    name: str
    dimensions: Tuple[str, ...]
    measures: Tuple[MeasureDef, ...]


def _pair_defs(prefix: str) -> List[MeasureDef]:
    """A ``<prefix>Total`` + ``<prefix>Interval`` counter pair (both ``Count``, resolution 60)."""
    return [MeasureDef(f"{prefix}Total", _UNIT_COUNT, 60), MeasureDef(f"{prefix}Interval", _UNIT_COUNT, 60)]


def family_defs() -> List[FamilyDef]:
    """The **complete** definition set — every family, measure, and dimension key this adapter emits.
    The startup pre-definition (:meth:`DeviceMetrics.define_all`) and the parity test both read it, so
    a dropped or renamed measure fails the test."""
    out: List[FamilyDef] = []

    # southbound_health — the §5 canonical set (dims: instance). All single measures.
    out.append(FamilyDef(
        name=HEALTH,
        dimensions=("instance",),
        measures=(
            MeasureDef("connectionState", _UNIT_COUNT, 1),
            MeasureDef("publishLatencyMs", _UNIT_MS, 1),
            MeasureDef("pollLatencyMs", _UNIT_MS, 1),
            MeasureDef("readErrors", _UNIT_COUNT, 60),
            MeasureDef("staleSignals", _UNIT_COUNT, 60),
            MeasureDef("reconnects", _UNIT_COUNT, 60),
        ),
    ))

    # <<COMPONENTNAME>>Connection — the connect/reconnect lifecycle (dims: instance).
    conn: List[MeasureDef] = [MeasureDef("connectionState", _UNIT_COUNT, 1)]
    conn += _pair_defs("connectAttempts")
    conn += _pair_defs("connectFailures")
    conn += _pair_defs("reconnectAttempts")
    conn += _pair_defs("connectionDrops")
    conn.append(MeasureDef("connectedDurationMs", _UNIT_MS, 60))
    out.append(FamilyDef(name=CONNECTION, dimensions=("instance",), measures=tuple(conn)))

    # <<COMPONENTNAME>>Command — the sb/* surface (dims: instance, verb, result).
    cmd: List[MeasureDef] = []
    cmd += _pair_defs("commandRequests")
    cmd += _pair_defs("commandErrors")
    cmd.append(MeasureDef("commandLatencyMs", _UNIT_MS, 60))
    out.append(FamilyDef(name=COMMAND, dimensions=("instance", "verb", "result"), measures=tuple(cmd)))

    # ADD YOUR PROTOCOL'S FAMILIES HERE (Inventory / Poll / Publish — see the module header).

    return out


def _family_def(name: str) -> FamilyDef:
    for f in family_defs():
        if f.name == name:
            return f
    raise KeyError(f"family_defs covers every family the emitter uses; missing {name!r}")


# =================================================================================================
# Counter state
# =================================================================================================

@dataclass
class _Pair:
    """A ``<name>Total`` (monotonic) + ``<name>Interval`` (reset on emit) counter pair."""

    total: float = 0.0
    interval: float = 0.0

    def add(self, value: float = 1.0) -> None:
        self.total += value
        self.interval += value

    def drain_into(self, out: Dict[str, float], prefix: str) -> None:
        """Write both measures into ``out`` and **reset the interval** — the emit convention."""
        out[f"{prefix}Total"] = self.total
        out[f"{prefix}Interval"] = self.interval
        self.interval = 0.0


class _ConnCounters:
    def __init__(self):
        self.ever_connected = False
        self.connect_attempts = _Pair()
        self.connect_failures = _Pair()
        self.reconnect_attempts = _Pair()
        self.connection_drops = _Pair()
        self.connected_accrued_ms = 0.0
        self.connected_since: Any = None  # monotonic seconds, or None

    def accrue(self, now: float) -> None:
        if self.connected_since is not None:
            self.connected_accrued_ms += max(0.0, now - self.connected_since) * 1000.0
            self.connected_since = now

    def drain(self, now: float, connection_state: float) -> Dict[str, float]:
        self.accrue(now)
        v: Dict[str, float] = {"connectionState": connection_state}
        self.connect_attempts.drain_into(v, "connectAttempts")
        self.connect_failures.drain_into(v, "connectFailures")
        self.reconnect_attempts.drain_into(v, "reconnectAttempts")
        self.connection_drops.drain_into(v, "connectionDrops")
        v["connectedDurationMs"] = self.connected_accrued_ms
        self.connected_accrued_ms = 0.0
        return v


class _CmdCounters:
    def __init__(self):
        self.command_requests = _Pair()
        self.command_errors = _Pair()
        self.command_latency_ms = 0.0

    def drain(self) -> Dict[str, float]:
        v: Dict[str, float] = {}
        self.command_requests.drain_into(v, "commandRequests")
        self.command_errors.drain_into(v, "commandErrors")
        v["commandLatencyMs"] = self.command_latency_ms
        self.command_latency_ms = 0.0
        return v


class DeviceMetrics:
    """A per-device operational-metrics emitter. Owns the counter state for one device's
    ``southbound_health`` plus the two worked families, and emits them on the metrics cadence and on
    connect/disconnect transitions. One per configured instance.

    ``metrics`` is the injected emitter (production: ``gg.get_metrics()`` — the static
    ``MetricEmitter``; tests pass a recorder). It must expose ``define_metric(metric)``,
    ``emit_metric(name, values)`` and ``emit_metric_now(name, values)``.
    """

    def __init__(self, metrics, config_manager, instance: str, health, stale_signal_secs: int = 30):
        self._metrics = metrics
        self._config = config_manager
        self._instance = instance
        self._health = health
        #: A signal with no update for longer than this is counted in ``staleSignals``
        #: (``component.global.healthThresholds.staleSignalSecs``).
        self._stale_after = max(1, int(stale_signal_secs))
        self._lock = threading.Lock()
        self._conn = _ConnCounters()
        # Pre-populate the full (verb, result) command matrix so the dimension set is fixed and
        # discoverable at startup.
        self._command: Dict[Tuple[str, str], _CmdCounters] = {
            (verb, result): _CmdCounters() for verb in COMMAND_VERBS for result in _RESULTS
        }
        #: Per-signal last-update instant (monotonic) — the staleness tracker.
        self._last_update: Dict[str, float] = {}

    def instance(self) -> str:
        return self._instance

    # ---- recording (called from the device task; all synchronous) --------------------------------

    def on_connect_attempt(self) -> None:
        with self._lock:
            self._conn.connect_attempts.add()

    def on_connected(self, now: float) -> None:
        """The connect attempt succeeded. A re-establishment (after a previous drop) also bumps
        ``reconnectAttempts``."""
        with self._lock:
            c = self._conn
            c.connected_since = now
            if c.ever_connected:
                c.reconnect_attempts.add()
            c.ever_connected = True

    def on_connect_failure(self) -> None:
        with self._lock:
            self._conn.connect_failures.add()

    def on_connection_dropped(self, now: float) -> None:
        with self._lock:
            c = self._conn
            c.accrue(now)
            c.connected_since = None
            c.connection_drops.add()

    def on_signal_update(self, signal_id: str, now: float) -> None:
        """Note that a signal just updated — feeds the ``staleSignals`` tracker."""
        with self._lock:
            self._last_update[signal_id] = now

    def record_command(self, verb: str, ok: bool, latency_ms: float) -> None:
        """Record one ``sb/*`` command outcome for its ``(verb, result)`` combo."""
        result = RESULT_SUCCESS if ok else RESULT_ERROR
        with self._lock:
            c = self._command.setdefault((verb, result), _CmdCounters())
            c.command_requests.add()
            c.command_latency_ms += float(latency_ms)
            if not ok:
                c.command_errors.add()

    def counters_view(self) -> Dict[str, Any]:
        """The connection-counter snapshot for ``sb/status`` / the diagnostics panel: each counter as
        ``{interval, total}``. Cheap; no device I/O."""
        with self._lock:
            def pair(p: _Pair) -> Dict[str, float]:
                return {"interval": p.interval, "total": p.total}

            return {
                "connectAttempts": pair(self._conn.connect_attempts),
                "connectFailures": pair(self._conn.connect_failures),
                "reconnectAttempts": pair(self._conn.reconnect_attempts),
                "connectionDrops": pair(self._conn.connection_drops),
            }

    def _stale_count(self, now: float) -> float:
        with self._lock:
            return float(sum(1 for t in self._last_update.values() if now - t > self._stale_after))

    # ---- definition + emission -------------------------------------------------------------------

    def define_all(self) -> None:
        """Pre-define every family × dimension combination at startup, so the metric set is fixed and
        discoverable. Each is also re-defined immediately before each emit (the name-keyed-store
        rule)."""
        self._define(HEALTH, [("instance", self._instance)])
        self._define(CONNECTION, [("instance", self._instance)])
        for verb in COMMAND_VERBS:
            for result in _RESULTS:
                self._define(COMMAND, [("instance", self._instance), ("verb", verb), ("result", result)])

    def _define(self, name: str, dimensions: List[Tuple[str, str]]) -> None:
        """Build + register one family combo's metric definition."""
        definition = _family_def(name)
        builder = MetricBuilder.create(name).with_config(self._config)
        for measure in definition.measures:
            builder = builder.add_measure(measure.name, measure.unit, measure.res)
        for key, value in dimensions:
            builder = builder.add_dimension(key, value)
        try:
            self._metrics.define_metric(builder.build())
        except Exception:  # noqa: BLE001 - a define failure must not crash the poll loop
            pass

    def _emit_combo(self, name: str, dimensions: List[Tuple[str, str]],
                    values: Dict[str, float], now: bool) -> None:
        """Re-define (with the combo's dimensions) then emit one family combo."""
        self._define(name, dimensions)
        payload = {k: float(v) for k, v in values.items()}
        try:
            if now:
                self._metrics.emit_metric_now(name, payload)
            else:
                self._metrics.emit_metric(name, payload)
        except Exception:  # noqa: BLE001 - a metric emit outage must not crash the poll loop
            pass

    def emit_periodic(self) -> None:
        """The full periodic emit (every metrics interval): ``southbound_health``, the connection
        family, and every command ``(verb, result)`` combo."""
        self._emit_health(False)
        self._emit_connection(False)
        self._emit_command()

    def emit_now(self) -> None:
        """The immediate transition emit: the mandatory ``southbound_health`` plus the connection
        gauges whose state just changed — flushed on connect / disconnect."""
        self._emit_health(True)
        self._emit_connection(True)

    def _emit_health(self, now: bool) -> None:
        v = {
            "connectionState": float(self._health.connection_state()),
            "publishLatencyMs": float(self._health.publish_latency_ms()),
            "pollLatencyMs": float(self._health.poll_latency_ms()),
            "readErrors": float(self._health.take_read_errors()),
            "staleSignals": self._stale_count(time.monotonic()),
            "reconnects": float(self._health.take_reconnects()),
        }
        self._emit_combo(HEALTH, [("instance", self._instance)], v, now)

    def _emit_connection(self, now: bool) -> None:
        state = float(self._health.connection_state())
        with self._lock:
            values = self._conn.drain(time.monotonic(), state)
        self._emit_combo(CONNECTION, [("instance", self._instance)], values, now)

    def _emit_command(self) -> None:
        with self._lock:
            rows = [((verb, result), c.drain()) for (verb, result), c in self._command.items()]
        for (verb, result), values in rows:
            self._emit_combo(
                COMMAND,
                [("instance", self._instance), ("verb", verb), ("result", result)],
                values,
                False,
            )
