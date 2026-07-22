"""The southbound command surface: every verb's happy path + each error code + the single-instance
default; the allow-list refusal proven to happen BEFORE any device I/O; pause gating a repoll; and
the panel registration. A mock control seam services the verbs and RECORDS every write that reaches
it — no device, no socket. Run with `pytest`.
"""
import pytest

from edgecommons.command_inbox import CommandException

from <<SNAKENAME>>.adapter import ONLINE, DeviceConfig, Health, set_paused
from <<SNAKENAME>>.command_service import Commander, DeviceHandle, panels, register_all
from <<SNAKENAME>>.device import (
    BrowseFailed,
    BrowsePage,
    BrowsedSignal,
    BrowseUnsupported,
    DeviceUnavailable,
    Quality,
    Reading,
    ReadFailed,
    ReconnectFailed,
    RepollRefused,
    SignalInfo,
    WriteRejected,
)
from <<SNAKENAME>>.metrics import DeviceMetrics


# --- a no-op metrics recorder + real DeviceMetrics so the command counters work without a runtime ---

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


def a_device_cfg(instance_id="plc-1", allow=("setpoint-1",)):
    return DeviceConfig(instance_id, "sim", {"endpoint": f"sim://{instance_id}"}, 5000, list(allow))


def sim_signals():
    return [
        SignalInfo(id="temperature-1", name="Ambient temperature"),
        SignalInfo(id="setpoint-1", name="Setpoint"),
    ]


class MockControl:
    """A mock device control seam. It records every write that REACHES it — an empty log proves the
    allow-list refused before any I/O."""

    def __init__(self, health, *, write_ok=True, read_ok=True, reconnect_ok=True, repoll_ok=True,
                 repoll_refused=False, browse="one", unavailable=False, read_empty=False):
        self._health = health
        self.write_ok = write_ok
        self.read_ok = read_ok
        self.reconnect_ok = reconnect_ok
        self.repoll_ok = repoll_ok
        self.repoll_refused = repoll_refused
        self.browse_kind = browse
        self.unavailable = unavailable
        self.read_empty = read_empty
        self.writes = []

    def read_now(self, ids):
        if self.unavailable:
            raise DeviceUnavailable()
        if not self.read_ok:
            raise ReadFailed("link error")
        if self.read_empty:
            return []  # the device returned nothing for the requested ids -> NO_DATA per ref
        return [Reading(signal_id=i, name=None, value=42.0, quality=Quality.GOOD, quality_raw="OK")
                for i in ids]

    def write(self, signal_id, value):
        if self.unavailable:
            raise DeviceUnavailable()
        self.writes.append((signal_id, value))
        if not self.write_ok:
            raise WriteRejected("device rejected")

    def browse(self, cursor, max_entries):
        if self.unavailable:
            raise DeviceUnavailable()
        if self.browse_kind == "unsupported":
            raise BrowseUnsupported()
        if self.browse_kind == "failed":
            raise BrowseFailed("mid-browse error")
        next_cursor = "page-2" if self.browse_kind == "paged" else None
        return BrowsePage(entries=[BrowsedSignal("temperature-1", "Ambient temperature", "REAL")],
                          next_cursor=next_cursor)

    def pause(self):
        if self.unavailable:
            raise DeviceUnavailable()
        return set_paused(self._health, True)

    def resume(self):
        if self.unavailable:
            raise DeviceUnavailable()
        return set_paused(self._health, False)

    def reconnect(self):
        if self.unavailable:
            raise DeviceUnavailable()
        if not self.reconnect_ok:
            raise ReconnectFailed("no route to host")

    def repoll(self):
        if self.unavailable:
            raise DeviceUnavailable()
        if self.repoll_refused:
            raise RepollRefused("device is disconnected")
        if not self.repoll_ok:
            raise DeviceUnavailable("link error")
        return 2


def make_handle(cfg=None, **control_opts):
    cfg = cfg or a_device_cfg()
    health = Health()
    health.set_link(ONLINE)
    dm = DeviceMetrics(NoopMetrics(), FakeConfigManager(), cfg.id, health, 30)
    control = MockControl(health, **control_opts)
    handle = DeviceHandle(cfg=cfg, control=control, health=health, dm=dm, signals=sim_signals())
    return handle, control, health


def commander(**opts):
    handle, control, health = make_handle(**opts)
    return Commander([handle]), control, health


def err_code(fn):
    with pytest.raises(CommandException) as ei:
        fn()
    return ei.value.code


# --- routing / single-instance default (D-EIP-13) ---------------------------------------------

def test_instance_defaults_to_the_sole_device_and_unknown_or_missing_ids_error():
    c, _control, _health = commander()
    assert c.status({})["id"] == "plc-1"
    assert err_code(lambda: c.status({"instance": "nope"})) == "NO_SUCH_INSTANCE"

    # Two devices: a missing `instance` is BAD_ARGS.
    h1, _, _ = make_handle(cfg=a_device_cfg("plc-1"))
    h2, _, _ = make_handle(cfg=a_device_cfg("plc-2"))
    multi = Commander([h1, h2])
    assert err_code(lambda: multi.status({})) == "BAD_ARGS"
    assert multi.status({"instance": "plc-2"})["id"] == "plc-2"


# --- sb/status ---------------------------------------------------------------------------------

def test_status_reports_connected_state_paused_and_a_counter_snapshot():
    c, _control, _health = commander()
    out = c.status({})
    assert out["connected"] is True
    assert out["state"] == "ONLINE"
    assert out["paused"] is False
    assert out["adapter"] == "sim"
    assert out["endpoint"] == "sim://plc-1"
    assert "connectAttempts" in out["metrics"]


# --- sb/signals --------------------------------------------------------------------------------

def test_signals_lists_the_inventory_with_the_writable_flag():
    c, _control, _health = commander()
    sigs = c.signals({})["signals"]
    assert len(sigs) == 2
    setpoint = next(s for s in sigs if s["id"] == "setpoint-1")
    assert setpoint["writable"] is True, "setpoint-1 is on the allow-list"
    temp = next(s for s in sigs if s["id"] == "temperature-1")
    assert temp["writable"] is False, "temperature-1 is not"


# --- sb/read -----------------------------------------------------------------------------------

def test_read_returns_values_by_id_and_by_name_and_marks_unresolved_refs():
    c, _control, _health = commander()
    out = c.read({"signals": [{"signalId": "temperature-1"}, {"name": "Setpoint"}, {"name": "ghost"}]})
    reads = out["reads"]
    assert reads[0]["signal"]["id"] == "temperature-1"
    assert reads[0]["quality"] == "GOOD"
    assert reads[1]["signal"]["id"] == "setpoint-1", "resolved by name"
    assert reads[2]["quality"] == "BAD", "an unknown name is a BAD/unresolved entry"
    assert reads[2]["qualityRaw"] == "UNRESOLVED_REF"


def test_read_without_a_signals_array_is_bad_args_and_a_link_error_is_read_failed():
    c, _control, _health = commander()
    assert err_code(lambda: c.read({})) == "BAD_ARGS"

    c2, _control2, _health2 = commander(read_ok=False)
    assert err_code(lambda: c2.read({"signals": [{"signalId": "temperature-1"}]})) == "READ_FAILED"


# --- sb/write: allow-list BEFORE any device I/O (the security guarantee) -----------------------

def test_a_refused_write_never_reaches_the_device():
    c, control, _health = commander()
    # temperature-1 is NOT on the allow-list.
    code = err_code(lambda: c.write({"writes": [{"signalId": "temperature-1", "value": 1}]}))
    assert code == "WRITE_NOT_ALLOWED"
    assert control.writes == [], "the refused write must never reach the device"


def test_an_allow_listed_write_is_confirmed_and_batches_mix_results():
    c, control, _health = commander()
    # A single allowed write (single-object shorthand).
    out = c.write({"signalId": "setpoint-1", "value": 42})
    assert out["written"] == 1
    assert len(control.writes) == 1, "the allowed write reached the device"

    # A batch: one allowed (written), one refused (never sent).
    out = c.write({"writes": [{"signalId": "setpoint-1", "value": 7},
                              {"signalId": "temperature-1", "value": 8}]})
    assert out["written"] == 1, "only the allow-listed entry is written"
    results = out["results"]
    assert sum(1 for r in results if r["ok"]) == 1
    assert sum(1 for r in results if r.get("error") == "not in writes.allow") == 1
    # Two device writes total (one from each successful call); the refused entry added none.
    assert len(control.writes) == 2


def test_a_write_the_device_rejects_is_write_failed():
    c, _control, _health = commander(write_ok=False)
    assert err_code(lambda: c.write({"signalId": "setpoint-1", "value": 42})) == "WRITE_FAILED"


def test_a_write_with_no_writes_or_value_is_bad_args():
    c, _control, _health = commander()
    assert err_code(lambda: c.write({})) == "BAD_ARGS"


# --- sb/browse ---------------------------------------------------------------------------------

def test_browse_returns_a_page_or_the_right_error_code():
    c, _control, _health = commander()
    out = c.browse({})
    assert len(out["entries"]) == 1
    assert out["entries"][0]["id"] == "temperature-1"

    c2, _c2, _h2 = commander(browse="unsupported")
    assert err_code(lambda: c2.browse({})) == "BROWSE_UNSUPPORTED"

    c3, _c3, _h3 = commander(browse="failed")
    assert err_code(lambda: c3.browse({})) == "BROWSE_FAILED"


# --- pause / resume / repoll -------------------------------------------------------------------

def test_pause_is_idempotent_and_repoll_is_refused_while_paused():
    c, _control, health = commander()

    # repoll works while running.
    assert c.repoll({})["polled"] == 2

    out = c.pause({})
    assert out["paused"] is True
    assert out["changed"] is True
    assert health.is_paused()

    # repoll is refused while paused (BAD_ARGS).
    assert err_code(lambda: c.repoll({})) == "BAD_ARGS"

    # pausing again is idempotent.
    assert c.pause({})["changed"] is False

    # resume clears it and repoll works again.
    out = c.resume({})
    assert out["paused"] is False
    assert out["changed"] is True
    assert not health.is_paused()
    assert c.repoll({})["polled"] == 2


# --- reconnect ---------------------------------------------------------------------------------

def test_reconnect_confirms_or_reports_reconnect_failed():
    c, _control, _health = commander()
    assert c.reconnect({})["connected"] is True

    c2, _c2, _h2 = commander(reconnect_ok=False)
    assert err_code(lambda: c2.reconnect({})) == "RECONNECT_FAILED"


def test_device_unavailable_when_the_seam_is_gone():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.reconnect({})) == "DEVICE_UNAVAILABLE"


# --- panels ------------------------------------------------------------------------------------

def test_the_three_panels_are_registered_with_the_right_ids_orders_and_scope():
    ps = panels()
    assert [p["id"] for p in ps] == ["overview", "signals", "diagnostics"]
    assert [p["order"] for p in ps] == [10, 20, 30]
    for p in ps:
        assert p["scope"] == "instance", "every panel is instance-scoped"
    # The signals panel binds the signal verbs; diagnostics binds browse.
    assert ps[1]["verbs"] == ["sb/signals", "sb/read", "sb/write", "repoll"]
    assert ps[2]["verbs"] == ["sb/browse", "sb/status"]


class _FakeRequest:
    def __init__(self, body):
        self._body = body

    def get_body(self):
        return self._body


class _FakeCommands:
    def __init__(self):
        self.handlers = {}
        self.panels = []

    def register(self, verb, handler):
        self.handlers[verb] = handler

    def register_panel(self, panel):
        self.panels.append(panel)


# --- signal-ref resolution + the remaining coded error branches --------------------------------


def test_read_resolves_a_signal_by_id_and_flags_a_ref_with_no_recognizable_key():
    c, _control, _health = commander()
    out = c.read({"signals": [{"id": "temperature-1"}, {"nonsense": 1}]})
    reads = out["reads"]
    assert reads[0]["signal"]["id"] == "temperature-1", "{'id': ...} resolves directly"
    assert reads[1]["quality"] == "BAD" and reads[1]["qualityRaw"] == "UNRESOLVED_REF"


def test_a_resolved_ref_the_device_returns_no_value_for_is_marked_no_data():
    c, _control, _health = commander(read_empty=True)
    out = c.read({"signals": [{"signalId": "temperature-1"}]})
    assert out["reads"][0]["qualityRaw"] == "NO_DATA", "resolved, requested, but nothing came back"


def test_read_maps_a_missing_device_to_device_unavailable():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.read({"signals": [{"signalId": "temperature-1"}]})) == "DEVICE_UNAVAILABLE"


def test_write_flags_an_unresolved_entry_and_an_allow_listed_entry_with_no_value():
    c, control, _health = commander()
    # An entry with no recognizable ref, and an allow-listed ref with no `value`: both are per-entry
    # results, neither reaches the device, and (nothing attempted) the call still succeeds overall.
    out = c.write({"writes": [{"value": 1}, {"signalId": "setpoint-1"}]})
    errors = {r.get("error") for r in out["results"]}
    assert "unresolved ref" in errors
    assert "missing value" in errors
    assert out["written"] == 0
    assert control.writes == []


def test_write_maps_a_missing_device_to_device_unavailable():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.write({"signalId": "setpoint-1", "value": 1})) == "DEVICE_UNAVAILABLE"


def test_browse_carries_a_next_cursor_when_the_page_is_partial_and_maps_unavailable():
    c, _control, _health = commander(browse="paged")
    out = c.browse({})
    assert out["cursor"] == "page-2", "a partial page advertises where to resume"

    c2, _c2, _h2 = commander(unavailable=True)
    assert err_code(lambda: c2.browse({})) == "DEVICE_UNAVAILABLE"


def test_pause_and_resume_map_a_missing_device_to_device_unavailable():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.pause({})) == "DEVICE_UNAVAILABLE"
    assert err_code(lambda: c.resume({})) == "DEVICE_UNAVAILABLE"


def test_repoll_maps_a_refusal_to_bad_args_and_a_missing_device_to_device_unavailable():
    c, _control, _health = commander(repoll_refused=True)
    assert err_code(lambda: c.repoll({})) == "BAD_ARGS"

    c2, _c2, _h2 = commander(unavailable=True)
    assert err_code(lambda: c2.repoll({})) == "DEVICE_UNAVAILABLE"


def test_a_request_whose_body_cannot_be_read_is_treated_as_an_empty_body():
    # `_body` must never crash the handler: a malformed request degrades to an empty dict, which the
    # single-device default then routes to the sole device.
    handle, _control, _health = make_handle()
    commands = _FakeCommands()
    register_all(commands, [handle])

    class _Broken:
        def get_body(self):
            raise RuntimeError("body decode failed")

    out = commands.handlers["sb/status"](_Broken())
    assert out["id"] == "plc-1"


def test_register_all_registers_the_nine_verbs_and_three_panels_and_dispatches():
    handle, _control, _health = make_handle()
    commands = _FakeCommands()
    register_all(commands, [handle])

    assert sorted(commands.handlers) == sorted([
        "sb/status", "sb/read", "sb/write", "sb/signals", "sb/browse", "sb/pause", "sb/resume",
        "reconnect", "repoll",
    ])
    assert [p["id"] for p in commands.panels] == ["overview", "signals", "diagnostics"]

    # A registered handler dispatches into the commander and returns the verb result.
    out = commands.handlers["sb/status"](_FakeRequest({}))
    assert out["id"] == "plc-1"
