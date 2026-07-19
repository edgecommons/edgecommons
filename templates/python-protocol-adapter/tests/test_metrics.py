"""The metric contract: `southbound_health` is EXACTLY the SOUTHBOUND.md §5 set, the operational
families are named from the component and low-cardinality, and the interval counters reset on emit.
Asserted against an independent literal transcription of §5 so a drift from the canonical doc fails
the test."""
from <<SNAKENAME>>.metrics import (
    COMMAND,
    CONNECTION,
    HEALTH,
    HEALTH_MEASURES,
    _Pair,
    family_defs,
)


def _family(name):
    return next(f for f in family_defs() if f.name == name)


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
