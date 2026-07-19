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
    App,
    Backoff,
    Device,
    DeviceConfig,
    Health,
    connectivity_of,
    set_paused,
)
from <<SNAKENAME>>.device import (
    BrowseFailed,
    BrowseUnsupported,
    DeviceError,
    DeviceUnavailable,
    Quality,
    ReadFailed,
    ReconnectFailed,
    SimBackend,
    WriteRejected,
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


# --- config parse guards -------------------------------------------------------------------------


def test_a_device_without_a_connection_object_is_rejected():
    with pytest.raises(ValueError, match="connection"):
        DeviceConfig.parse("plc-1", {"adapter": "sim"}, 5000)


def test_a_non_list_writes_allow_is_rejected():
    with pytest.raises(ValueError, match="writes.allow"):
        DeviceConfig.parse("plc-1", {"connection": {"endpoint": "sim://x"}, "writes": {"allow": "setpoint-1"}}, 5000)


# --- Health: one source, every surface -----------------------------------------------------------


def test_health_gauges_and_interval_counters_feed_the_metric():
    health = Health()
    health.set_poll_latency(12)
    health.set_publish_latency(8)
    assert (health.poll_latency_ms(), health.publish_latency_ms()) == (12, 8)

    health.incr_read_error()
    health.incr_read_error()
    health.incr_reconnect()
    # `take_*` drains the interval counter (read-and-reset) — the metric emit convention.
    assert health.take_read_errors() == 2
    assert health.take_read_errors() == 0, "the interval counter reset on read"
    assert health.take_reconnects() == 1
    assert health.take_reconnects() == 0


# --- Backoff: exponential with full jitter, capped -----------------------------------------------


def test_backoff_is_bounded_by_the_cap_and_never_negative():
    b = Backoff(base_ms=1000, max_ms=4000)
    # Full jitter: a random point in [0, cap], and the cap is min(base * 2**attempt, max_ms).
    for attempt in range(0, 25):
        delay = b.delay_secs(attempt)
        assert 0.0 <= delay <= 4.0, "never longer than the cap, never negative"
    # A large attempt is clamped to the max window, not grown into nonsense.
    assert all(b.delay_secs(50) <= 4.0 for _ in range(20))


# --- the device worker, driven against the in-process sim backend --------------------------------
#
# The infinite supervisor loop (Device._run) is the live-runtime seam (test_live_sim.py exercises it
# on real infra); its step methods are unit-testable directly against the sim, with a recording
# stand-in for the framework facade — no broker, no thread, no device.


class RecordingBuilder:
    def __init__(self, sink):
        self._sink = sink

    def name(self, name):
        return self

    def device(self, adapter=None, instance=None, endpoint=None):
        return self

    def add_sample(self, sample):
        return self

    def signal_path(self, signal_id):
        return self

    def publish(self):
        self._sink.published.append("sample")


class RecordingData:
    def __init__(self, sink):
        self._sink = sink

    def signal(self, signal_id):
        return RecordingBuilder(self._sink)

    def publish_body(self, signal_id, body):
        self._sink.published.append(("body", signal_id, body))


class RecordingEvents:
    def __init__(self, sink):
        self._sink = sink

    def emit(self, type, message, context, severity=None):
        self._sink.events.append(type)

    def clear_alarm(self, type):
        self._sink.events.append(("clear", type))

    def raise_alarm(self, type, message, context, severity=None):
        self._sink.events.append(("alarm", type))


class RecordingInstance:
    def __init__(self, sink):
        self._sink = sink

    def data(self):
        return RecordingData(self._sink)

    def events(self):
        return RecordingEvents(self._sink)


class NoopMetrics:
    def define_metric(self, metric):
        pass

    def emit_metric(self, name, values):
        pass

    def emit_metric_now(self, name, values):
        pass


class FakeConfigManager:
    def get_thing_name(self):
        return "thing-1"

    def get_component_name(self):
        return "com.example.MyAdapter"


class FakeGg:
    """The framework facade a Device is handed: an instance-scoped data/events facade, the metric
    emitter, and the config manager. Records every publish and event."""

    def __init__(self):
        self.published = []
        self.events = []

    def instance(self, instance_id):
        return RecordingInstance(self)

    def get_metrics(self):
        return NoopMetrics()

    def get_config_manager(self):
        return FakeConfigManager()


def _sim_cfg(allow=("setpoint-1",)):
    return DeviceConfig("plc-1", "sim", {"endpoint": "sim://plc-1"}, 5000, list(allow))


def _device(gg=None, cfg=None):
    return Device(gg or FakeGg(), cfg or _sim_cfg(), stale_signal_secs=30)


def test_a_device_wires_its_seam_and_reports_its_handle_and_connectivity():
    gg = FakeGg()
    device = _device(gg)
    handle = device.handle()
    assert handle.cfg.id == "plc-1"
    assert [s.id for s in handle.signals] == ["temperature-1", "pressure-1"], "the sim inventory"
    assert device.connectivity().state == CONNECTING, "not connected until the first connect"


def test_connect_polls_and_publishes_good_and_bad_readings_through_the_data_facade():
    gg = FakeGg()
    device = _device(gg)

    device._connect_once()
    assert device.connectivity().connected is True
    assert "device-connected" in gg.events

    device._poll_tick()
    # The GOOD reading rides the sample builder; the BAD (value=None) rides the pre-built body path —
    # a failed read is published as BAD, never omitted.
    assert "sample" in gg.published, "the GOOD reading published a sample"
    assert any(isinstance(p, tuple) and p[0] == "body" for p in gg.published), "the BAD reading published a body"


def test_a_dropped_link_backs_off_and_raises_the_unreachable_alarm():
    gg = FakeGg()
    device = _device(gg)
    device._connect_once()

    device._on_drop()

    assert device.connectivity().state == BACKOFF
    assert ("alarm", "device-unreachable") in gg.events


def test_the_control_seam_reads_writes_browses_and_repolls_over_the_live_session():
    device = _device()
    device._connect_once()

    got = device.read_now(["temperature-1"])
    assert [r.signal_id for r in got] == ["temperature-1"]

    device.write("setpoint-1", 42)  # the sim accepts every write

    page = device.browse(None, 100)
    assert [e.id for e in page.entries] == ["temperature-1", "pressure-1"]

    assert device.repoll() == 2, "a repoll reads every configured signal once"

    # pause/resume flip the flag and emit an event; reconnect re-establishes the session.
    assert device.pause() is True
    assert device.pause() is False, "pausing again is idempotent"
    assert device.resume() is True
    device.reconnect()
    assert device.connectivity().connected is True


def test_the_control_seam_reports_device_unavailable_before_a_session_exists():
    device = _device()  # never connected
    with pytest.raises(DeviceUnavailable):
        device.read_now(["temperature-1"])
    with pytest.raises(DeviceUnavailable):
        device.write("setpoint-1", 1)
    with pytest.raises(DeviceUnavailable):
        device.browse(None, 10)
    with pytest.raises(DeviceUnavailable):
        device.repoll()


def test_stop_closes_the_session_and_is_safe_before_connecting():
    device = _device()
    device.stop()  # no session yet — must not raise
    device._connect_once()
    assert device._session is not None
    device.stop()
    assert device._session is None, "stop closed and cleared the live session"


class _FailingSession:
    """A session whose every device call fails at the link, to drive the error branches."""

    def read_signals(self):
        raise DeviceError("link down")

    def read_named(self, ids):
        raise DeviceError("link down")

    def write_signal(self, signal_id, value):
        raise DeviceError("device rejected")

    def browse(self, cursor, max_entries):
        raise DeviceError("mid-browse failure")

    def close(self):
        return None


class _SessionBackend:
    """A backend that connects to a supplied session (so a test can inject a failing one)."""

    def __init__(self, session):
        self._session = session

    def kind(self):
        return "sim"

    def inventory(self, connection):
        return []

    def connect(self, connection):
        return self._session


def test_a_read_failure_during_a_poll_drops_the_link_and_reconnects():
    gg = FakeGg()
    device = _device(gg)
    device._backend = _SessionBackend(_FailingSession())
    device._connect_once()

    device._poll_tick()  # read_signals raises -> _LinkLost -> session closed -> _on_drop

    assert device._session is None, "a broken read drops the session so the loop reconnects"
    assert device.connectivity().state == BACKOFF


def test_the_control_seam_maps_session_errors_to_the_standard_coded_failures():
    device = _device()
    device._backend = _SessionBackend(_FailingSession())
    device._connect_once()

    with pytest.raises(ReadFailed):
        device.read_now(["temperature-1"])
    with pytest.raises(WriteRejected):
        device.write("setpoint-1", 1)
    with pytest.raises(BrowseFailed):
        device.browse(None, 10)


def test_a_repoll_that_loses_the_link_drops_and_reports_unavailable():
    device = _device()
    device._backend = _SessionBackend(_FailingSession())
    device._connect_once()

    with pytest.raises(DeviceUnavailable):
        device.repoll()
    assert device._session is None


def test_a_reconnect_that_cannot_re_establish_reports_reconnect_failed():
    device = _device()
    device._connect_once()  # a live sim session first
    device._backend = _FailingBackend(transient=True)  # now the endpoint is gone
    with pytest.raises(ReconnectFailed):
        device.reconnect()


class _FailingBackend:
    """A backend whose connect always fails — to drive the connect-failure branch of _connect_once
    without waiting on a real backoff (the Device's Backoff is swapped for a ~0ms one)."""

    def __init__(self, transient):
        self._transient = transient

    def kind(self):
        return "failing"

    def inventory(self, connection):
        return []

    def connect(self, connection):
        raise DeviceError("unreachable", transient=self._transient)


@pytest.mark.parametrize("transient", [True, False])
def test_a_failed_connect_records_the_failure_and_backs_off(transient):
    device = _device()
    device._backend = _FailingBackend(transient)
    device._backoff = Backoff(base_ms=1, max_ms=1)  # keep the backoff wait ~0ms in the test

    device._connect_once()

    assert device.connectivity().state == BACKOFF
    assert device.connectivity().connected is False


# --- App: build one device per component.instances[] entry ---------------------------------------


class AppConfigManager:
    def __init__(self, global_cfg, instances):
        self._global = global_cfg
        self._instances = instances

    def get_global_config(self):
        return self._global

    def get_instance_ids(self):
        return list(self._instances)

    def get_instance_config(self, instance_id):
        return self._instances[instance_id]

    # for DeviceMetrics' MetricBuilder.with_config
    def get_thing_name(self):
        return "thing-1"

    def get_component_name(self):
        return "com.example.MyAdapter"


class AppGg(FakeGg):
    def __init__(self, global_cfg=None, instances=None):
        super().__init__()
        self._cm = AppConfigManager(global_cfg or {}, instances or {})

    def get_config_manager(self):
        return self._cm


def test_app_builds_one_device_per_instance_and_skips_a_malformed_one():
    gg = AppGg(
        global_cfg={"healthThresholds": {"staleSignalSecs": 15}, "defaults": {"pollIntervalMs": 2000}},
        instances={
            "plc-1": {"adapter": "sim", "connection": {"endpoint": "sim://plc-1"}},
            "plc-2": {"adapter": "sim"},  # no connection -> malformed -> skipped
        },
    )
    app = App(gg)
    assert [d.handle().cfg.id for d in app._devices] == ["plc-1"], "the malformed device was skipped"


def test_app_with_no_valid_devices_fails_loudly():
    gg = AppGg(instances={"plc-2": {"adapter": "sim"}})  # every entry malformed
    with pytest.raises(RuntimeError, match="no valid devices"):
        App(gg)
