"""The metric contract: `southbound_health` is EXACTLY the SOUTHBOUND.md §5 set, the operational
families are named from the component and low-cardinality, and the interval counters reset on emit.
Asserted against an independent literal transcription of §5 so a drift from the canonical doc fails
the test."""
from <<SNAKENAME>>.adapter import Health
from <<SNAKENAME>>.metrics import (
    COMMAND,
    COMMAND_VERBS,
    CONNECTION,
    HEALTH,
    HEALTH_MEASURES,
    DeviceMetrics,
    _Pair,
    family_defs,
)


def _family(name):
    return next(f for f in family_defs() if f.name == name)


# --- a recording metrics emitter + a config stand-in so DeviceMetrics runs with no runtime --------


class RecordingMetrics:
    """The injected emitter: records every define/emit so a test can assert what was emitted."""

    def __init__(self):
        self.defined = []
        self.emitted = []       # (name, values) via the periodic emit_metric
        self.emitted_now = []   # (name, values) via the transition emit_metric_now

    def define_metric(self, metric):
        self.defined.append(metric)

    def emit_metric(self, name, values):
        self.emitted.append((name, values))

    def emit_metric_now(self, name, values):
        self.emitted_now.append((name, values))


class FakeConfigManager:
    def get_thing_name(self):
        return "thing-1"

    def get_component_name(self):
        return "com.example.MyAdapter"


def _dm(metrics=None, health=None, stale_secs=30):
    return DeviceMetrics(metrics or RecordingMetrics(), FakeConfigManager(), "plc-1",
                         health or Health(), stale_secs)


def test_southbound_health_emits_exactly_the_section_5_measure_set():
    # A second, independent copy of §5 — NOT the module const, so a wrong edit to one is caught.
    section_5 = {
        "connectionState", "publishLatencyMs", "pollLatencyMs", "readErrors", "staleSignals",
        "reconnects",
    }
    emitted = {m.name for m in _family(HEALTH).measures}
    assert emitted == section_5, "southbound_health must be the exact §5 set — no more, no less"
    assert set(HEALTH_MEASURES) == section_5, "HEALTH_MEASURES must equal the §5 set"


def test_operational_families_are_named_from_the_component_and_low_cardinality():
    names = [f.name for f in family_defs()]
    assert CONNECTION in names, "the Connection family is present"
    assert COMMAND in names, "the Command family is present"
    # Named from the component token — a fleet view separates adapters by name.
    assert CONNECTION.endswith("Connection") and CONNECTION != "Connection"
    assert COMMAND.endswith("Command") and COMMAND != "Command"
    assert _family(COMMAND).dimensions == ("instance", "verb", "result"), "closed, low-cardinality dims only"


def test_the_connection_family_is_the_counter_pair_pattern():
    names = [m.name for m in _family(CONNECTION).measures]
    for base in ("connectAttempts", "connectFailures", "reconnectAttempts", "connectionDrops"):
        assert f"{base}Total" in names, f"{base}Total present"
        assert f"{base}Interval" in names, f"{base}Interval present"
    assert "connectionState" in names, "the state gauge"
    assert "connectedDurationMs" in names, "the connected-duration sum"


def test_interval_counters_reset_on_drain_but_totals_do_not():
    p = _Pair()
    p.add(3.0)
    out = {}
    p.drain_into(out, "x")
    assert out["xTotal"] == 3.0
    assert out["xInterval"] == 3.0

    p.add(2.0)
    out2 = {}
    p.drain_into(out2, "x")
    assert out2["xTotal"] == 5.0, "total is monotonic across emits"
    assert out2["xInterval"] == 2.0, "interval resets to only what accrued since the last emit"


# --- the DeviceMetrics emitter: definition, recording, and the drain-on-emit convention -----------


def test_define_all_pre_defines_every_family_and_command_combo():
    metrics = RecordingMetrics()
    _dm(metrics).define_all()
    # HEALTH + CONNECTION + one COMMAND definition per (verb, result) combo.
    assert len(metrics.defined) == 2 + len(COMMAND_VERBS) * 2


def test_connection_lifecycle_recording_drives_the_connection_counters():
    dm = _dm()
    dm.on_connect_attempt()
    dm.on_connected(now=100.0)          # first connect: no reconnect bump
    dm.on_connection_dropped(now=101.0)
    dm.on_connect_attempt()
    dm.on_connected(now=102.0)          # a re-establishment: bumps reconnectAttempts
    dm.on_connect_failure()

    view = dm.counters_view()
    assert view["connectAttempts"]["total"] == 2.0
    assert view["reconnectAttempts"]["total"] == 1.0, "the second connect is a reconnect"
    assert view["connectionDrops"]["total"] == 1.0
    assert view["connectFailures"]["total"] == 1.0


def test_recording_a_command_tracks_requests_errors_and_latency():
    dm = _dm()
    dm.record_command("sb/read", True, 4.0)
    dm.record_command("sb/read", False, 6.0)   # an error bumps both requests and errors
    dm.emit_periodic()
    # The COMMAND family emits one combo per (verb, result); find the sb/read error combo's payload.
    metrics = dm._metrics
    combos = [v for (n, v) in metrics.emitted if n == COMMAND]
    assert any(v.get("commandRequestsTotal") == 1.0 and v.get("commandErrorsTotal") == 1.0
               for v in combos), "the error combo recorded a request and an error"


def test_a_signal_with_no_recent_update_is_counted_stale():
    health = Health()
    dm = _dm(health=health, stale_secs=30)
    # A very old update is stale; a fresh one is not.
    dm.on_signal_update("temp-1", now=0.0)
    assert dm._stale_count(now=1000.0) == 1.0
    dm.on_signal_update("temp-1", now=1000.0)
    assert dm._stale_count(now=1000.0) == 0.0


def test_emit_periodic_emits_health_connection_and_every_command_combo():
    metrics = RecordingMetrics()
    dm = _dm(metrics)
    dm.emit_periodic()

    emitted_names = {n for (n, _v) in metrics.emitted}
    assert HEALTH in emitted_names
    assert CONNECTION in emitted_names
    # One COMMAND emit per (verb, result) combo.
    assert sum(1 for (n, _v) in metrics.emitted if n == COMMAND) == len(COMMAND_VERBS) * 2


def test_emit_now_flushes_health_and_connection_as_transition_metrics():
    metrics = RecordingMetrics()
    dm = _dm(metrics)
    dm.emit_now()
    names = {n for (n, _v) in metrics.emitted_now}
    assert names == {HEALTH, CONNECTION}, "the immediate transition emit is health + connection only"


def test_the_emitter_never_lets_a_define_or_emit_outage_crash_the_poll_loop():
    class Broken:
        def define_metric(self, metric):
            raise RuntimeError("define down")

        def emit_metric(self, name, values):
            raise RuntimeError("emit down")

        def emit_metric_now(self, name, values):
            raise RuntimeError("emit-now down")

    dm = _dm(Broken())
    # None of these may propagate — a metrics outage must not take the device worker down.
    dm.define_all()
    dm.emit_periodic()
    dm.emit_now()
