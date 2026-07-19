"""The device seam + what the adapter reports about its devices. Pure logic — no protocol client, no
broker, no transport: the simulator connects and reads, a failed read is published as BAD quality
rather than omitted, and a configured-but-down device is still *reported* (and says which kind of
unreachable it is). Run with `pytest`.
"""
import pytest

from <<SNAKENAME>>.adapter import (
    BACKOFF,
    CONNECTING,
    ONLINE,
    DeviceConfig,
    Health,
    connectivity_of,
    set_paused,
)
from <<SNAKENAME>>.device import (
    BrowseUnsupported,
    DeviceError,
    Quality,
    SimBackend,
    make_backend,
)


# --- the simulated backend --------------------------------------------------------------------

def test_the_sim_backend_connects_and_reads():
    session = SimBackend().connect({"endpoint": "sim://device"})
    readings = session.read_signals()
    assert len(readings) == 2
    assert readings[0].signal_id == "temperature-1"
    assert readings[0].quality == Quality.GOOD


def test_a_failed_read_is_published_as_bad_quality_not_omitted():
    session = SimBackend().connect({"endpoint": "sim://device"})
    readings = session.read_signals()
    bad = next(r for r in readings if r.signal_id == "pressure-1")
    assert bad.quality == Quality.BAD
    assert bad.quality_raw == "SENSOR_FAULT"
    assert bad.value is None


def test_a_misconfiguration_is_permanent_so_the_supervisor_does_not_hammer_it():
    with pytest.raises(DeviceError) as ei:
        SimBackend().connect({})
    assert ei.value.transient is False, "a missing endpoint will never fix itself by retrying"


def test_read_named_returns_only_the_requested_signals():
    session = SimBackend().connect({"endpoint": "sim://device"})
    got = session.read_named(["temperature-1"])
    assert len(got) == 1
    assert got[0].signal_id == "temperature-1"
    assert session.read_named(["nope"]) == []


def test_the_sim_browses_one_page_and_stops():
    session = SimBackend().connect({"endpoint": "sim://device"})
    page = session.browse(None, 100)
    assert len(page.entries) == 2
    assert page.entries[0].id == "temperature-1"
    assert page.next_cursor is None
    assert session.browse("x", 100).entries == []


def test_the_sim_advertises_its_inventory_without_connecting():
    inv = SimBackend().inventory({"endpoint": "sim://device"})
    assert len(inv) == 2
    assert inv[0].id == "temperature-1"
    assert inv[0].name == "Ambient temperature"


def test_make_backend_resolves_sim_and_is_none_for_unknown():
    assert make_backend("sim") is not None
    assert make_backend("nope") is None


# --- config -----------------------------------------------------------------------------------

def test_a_device_parses_from_its_instance_config():
    cfg = DeviceConfig.parse("plc-1", {
        "adapter": "sim",
        "connection": {"endpoint": "sim://plc-1", "unitId": 3},
        "pollIntervalMs": 1000,
        "writes": {"allow": ["setpoint-1"]},
    }, 5000)
    assert cfg.id == "plc-1"
    assert cfg.poll_interval_ms == 1000
    assert cfg.endpoint == "sim://plc-1"
    assert cfg.connection["unitId"] == 3


def test_an_adapter_is_read_only_until_a_write_is_allow_listed():
    cfg = DeviceConfig.parse("plc-1", {"connection": {"endpoint": "sim://plc-1"}}, 5000)
    assert not cfg.permits("setpoint-1"), "nothing is writable by default"

    cfg2 = DeviceConfig.parse("plc-1", {
        "connection": {"endpoint": "sim://plc-1"}, "writes": {"allow": ["setpoint-1"]},
    }, 5000)
    assert cfg2.permits("setpoint-1")
    assert not cfg2.permits("setpoint-2"), "only the listed signal, not its neighbours"


# --- connectivity -----------------------------------------------------------------------------

def test_every_device_reports_its_own_connectivity():
    cfg = DeviceConfig.parse("plc-1", {"adapter": "sim", "connection": {"endpoint": "sim://plc-1"}}, 5000)
    health = Health()

    # Before the first connect: not reachable, and the token says why — CONNECTING, not BACKOFF.
    c = connectivity_of(cfg, health)
    assert c.instance == "plc-1"
    assert c.connected is False
    assert c.state == CONNECTING
    assert c.detail == "sim://plc-1", "the endpoint, for a human"
    assert c.attributes["adapter"] == "sim"
    assert c.attributes["paused"] is False

    health.set_link(ONLINE)
    c = connectivity_of(cfg, health)
    assert c.connected is True
    assert c.state == ONLINE

    health.set_link(BACKOFF)
    assert connectivity_of(cfg, health).connected is False


def test_a_paused_online_device_reports_paused_but_stays_connected():
    cfg = DeviceConfig.parse("plc-1", {"connection": {"endpoint": "sim://plc-1"}}, 5000)
    health = Health()
    health.set_link(ONLINE)

    assert set_paused(health, True), "pausing changed the state"
    assert not set_paused(health, True), "pausing again is idempotent"
    c = connectivity_of(cfg, health)
    assert c.state == "PAUSED", "paused + online = PAUSED"
    assert c.connected is True, "connected stays truthful while paused"
    assert c.attributes["paused"] is True

    # A break while paused reports BACKOFF (not PAUSED), connected false.
    health.set_link(BACKOFF)
    c = connectivity_of(cfg, health)
    assert c.state == BACKOFF
    assert c.connected is False


def test_the_normalized_flag_and_the_health_metric_cannot_disagree():
    health = Health()
    health.set_link(ONLINE)
    assert health.connection_state() == 1
    health.set_link(BACKOFF)
    assert health.connection_state() == 0


def test_browse_is_unsupported_by_default():
    from <<SNAKENAME>>.device import DeviceSession

    class NoBrowse(DeviceSession):
        def read_signals(self):
            return []

        def write_signal(self, signal_id, value):
            return None

    with pytest.raises(BrowseUnsupported):
        NoBrowse().browse(None, 10)
