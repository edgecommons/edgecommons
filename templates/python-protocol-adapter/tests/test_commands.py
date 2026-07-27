"""The southbound command surface: every verb's happy path + each error code + the single-instance
default; the allow-list refusal proven to happen BEFORE any device I/O; pause gating a repoll; the
panel registration; and the scoped delivery contract — every verb declares
``CommandScope.INSTANCE``, and a live inbox routes a component-addressed and an instance-addressed
request into it and refuses a conflicting body before any handler runs. A mock control seam services
the verbs and RECORDS every call that reaches it — no device, no socket. Run with `pytest`.
"""
import pytest

from edgecommons.command_inbox import CommandException, CommandInbox, CommandScope
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient

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

#: The verbs `register_all` puts on the inbox — every one of them instance-scoped.
NINE_VERBS = [
    "sb/status", "sb/read", "sb/write", "sb/signals", "sb/browse", "sb/pause", "sb/resume",
    "reconnect", "repoll",
]


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
        #: Every control call that REACHED the seam — empty proves no handler ran.
        self.calls = []

    def read_now(self, ids):
        self.calls.append("read_now")
        if self.unavailable:
            raise DeviceUnavailable()
        if not self.read_ok:
            raise ReadFailed("link error")
        if self.read_empty:
            return []  # the device returned nothing for the requested ids -> NO_DATA per ref
        return [Reading(signal_id=i, name=None, value=42.0, quality=Quality.GOOD, quality_raw="OK")
                for i in ids]

    def write(self, signal_id, value):
        self.calls.append("write")
        if self.unavailable:
            raise DeviceUnavailable()
        self.writes.append((signal_id, value))
        if not self.write_ok:
            raise WriteRejected("device rejected")

    def browse(self, cursor, max_entries):
        self.calls.append("browse")
        if self.unavailable:
            raise DeviceUnavailable()
        if self.browse_kind == "unsupported":
            raise BrowseUnsupported()
        if self.browse_kind == "failed":
            raise BrowseFailed("mid-browse error")
        if self.browse_kind == "paged":
            # Two entries over two pages, so cursor-following and truncation are observable.
            if cursor is None:
                return BrowsePage(entries=[BrowsedSignal("temperature-1", "Ambient temperature", "REAL")],
                                  next_cursor="page-2")
            return BrowsePage(entries=[BrowsedSignal("pressure-1", "Line pressure", "REAL")],
                              next_cursor=None)
        return BrowsePage(entries=[BrowsedSignal("temperature-1", "Ambient temperature", "REAL")],
                          next_cursor=None)

    def pause(self):
        self.calls.append("pause")
        if self.unavailable:
            raise DeviceUnavailable()
        return set_paused(self._health, True)

    def resume(self):
        self.calls.append("resume")
        if self.unavailable:
            raise DeviceUnavailable()
        return set_paused(self._health, False)

    def reconnect(self):
        self.calls.append("reconnect")
        if self.unavailable:
            raise DeviceUnavailable()
        if not self.reconnect_ok:
            raise ReconnectFailed("no route to host")

    def repoll(self):
        self.calls.append("repoll")
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


# --- routing: the library hands over the addressed instance, this maps it to a device -----------

def test_instance_defaults_to_the_sole_device_and_unknown_or_missing_ids_error():
    c, _control, _health = commander()
    assert c.status(None)["id"] == "plc-1"
    assert err_code(lambda: c.status("nope")) == "NO_SUCH_INSTANCE"

    # Two devices: a missing `instance` is BAD_ARGS.
    h1, _, _ = make_handle(cfg=a_device_cfg("plc-1"))
    h2, _, _ = make_handle(cfg=a_device_cfg("plc-2"))
    multi = Commander([h1, h2])
    assert err_code(lambda: multi.status(None)) == "BAD_ARGS"
    assert multi.status("plc-2")["id"] == "plc-2"


# --- sb/status ---------------------------------------------------------------------------------

def test_status_reports_connected_state_paused_and_a_counter_snapshot():
    c, _control, _health = commander()
    out = c.status(None)
    assert out["connected"] is True
    assert out["state"] == "ONLINE"
    assert out["paused"] is False
    assert out["adapter"] == "sim"
    assert out["endpoint"] == "sim://plc-1"
    assert "connectAttempts" in out["metrics"]


# --- sb/signals --------------------------------------------------------------------------------

def test_signals_lists_the_inventory_with_the_writable_flag():
    c, _control, _health = commander()
    sigs = c.signals(None)["signals"]
    assert len(sigs) == 2
    setpoint = next(s for s in sigs if s["id"] == "setpoint-1")
    assert setpoint["writable"] is True, "setpoint-1 is on the allow-list"
    temp = next(s for s in sigs if s["id"] == "temperature-1")
    assert temp["writable"] is False, "temperature-1 is not"


# --- sb/read -----------------------------------------------------------------------------------

def test_read_returns_values_by_id_and_by_name_and_marks_unresolved_refs():
    c, _control, _health = commander()
    out = c.read(None, {"signals": [{"signalId": "temperature-1"}, {"name": "Setpoint"}, {"name": "ghost"}]})
    reads = out["reads"]
    assert reads[0]["signal"]["id"] == "temperature-1"
    assert reads[0]["quality"] == "GOOD"
    assert reads[1]["signal"]["id"] == "setpoint-1", "resolved by name"
    assert reads[2]["quality"] == "BAD", "an unknown name is a BAD/unresolved entry"
    assert reads[2]["qualityRaw"] == "UNRESOLVED_REF"


def test_read_without_a_signals_array_is_bad_args_and_a_link_error_is_read_failed():
    c, _control, _health = commander()
    assert err_code(lambda: c.read(None, {})) == "BAD_ARGS"

    c2, _control2, _health2 = commander(read_ok=False)
    assert err_code(lambda: c2.read(None, {"signals": [{"signalId": "temperature-1"}]})) == "READ_FAILED"


# --- sb/write: allow-list BEFORE any device I/O (the security guarantee) -----------------------

def test_a_refused_write_never_reaches_the_device():
    c, control, health = commander()
    # temperature-1 is NOT on the allow-list.
    code = err_code(lambda: c.write(None, {"writes": [{"signalId": "temperature-1", "value": 1}]}))
    assert code == "WRITE_NOT_ALLOWED"
    assert control.writes == [], "the refused write must never reach the device"
    assert health.take_write_errors() == 0, "an allow-list refusal is policy, not a device write error"


def test_an_allow_listed_write_is_confirmed_and_batches_mix_results():
    c, control, _health = commander()
    # A single allowed write (single-object shorthand).
    out = c.write(None, {"signalId": "setpoint-1", "value": 42})
    assert out["written"] == 1
    assert len(control.writes) == 1, "the allowed write reached the device"

    # A batch: one allowed (written), one refused (never sent).
    out = c.write(None, {"writes": [{"signalId": "setpoint-1", "value": 7},
                              {"signalId": "temperature-1", "value": 8}]})
    assert out["written"] == 1, "only the allow-listed entry is written"
    results = out["results"]
    assert sum(1 for r in results if r["ok"]) == 1
    assert sum(1 for r in results if r.get("error") == "not in writes.allow") == 1
    # Two device writes total (one from each successful call); the refused entry added none.
    assert len(control.writes) == 2


def test_a_write_the_device_rejects_is_write_failed_and_counts_a_write_error():
    c, _control, health = commander(write_ok=False)
    assert err_code(lambda: c.write(None, {"signalId": "setpoint-1", "value": 42})) == "WRITE_FAILED"
    assert health.take_write_errors() == 1, "one rejected entry feeds southbound_health.writeErrors"


def test_a_write_with_no_writes_or_value_is_bad_args():
    c, _control, _health = commander()
    assert err_code(lambda: c.write(None, {})) == "BAD_ARGS"


# --- sb/browse ---------------------------------------------------------------------------------

def test_browse_returns_a_page_or_the_right_error_code():
    c, _control, _health = commander()
    out = c.browse(None, {})
    assert len(out["entries"]) == 1
    assert out["entries"][0]["id"] == "temperature-1"

    c2, _c2, _h2 = commander(browse="unsupported")
    assert err_code(lambda: c2.browse(None, {})) == "BROWSE_UNSUPPORTED"

    c3, _c3, _h3 = commander(browse="failed")
    assert err_code(lambda: c3.browse(None, {})) == "BROWSE_FAILED"


# --- sb/browse: the hierarchical panel mode ----------------------------------------------------


def test_hierarchical_browse_of_root_lists_the_inventory_as_contains_refs():
    c, _control, _health = commander()
    out = c.browse(None, {"ref": "root"})
    assert out["mode"] == "hierarchical"
    root = out["root"]
    assert root["nodeId"] == "root"
    assert root["name"] == "plc-1", "the root node is the instance"
    assert root["nodeClass"] == "device"
    assert root["dataType"] is None
    assert root["refs"] == [{
        "referenceType": "contains",
        "target": {"nodeId": "temperature-1", "name": "Ambient temperature",
                   "nodeClass": "signal", "dataType": "REAL"},
    }]
    assert out["refCount"] == 1
    assert out["depth"] == 1
    assert out["truncated"] is False


def test_hierarchical_browse_of_a_signal_is_a_known_leaf():
    c, _control, _health = commander()
    out = c.browse(None, {"ref": "temperature-1"})
    root = out["root"]
    assert root["nodeId"] == "temperature-1"
    assert root["nodeClass"] == "signal"
    assert root["dataType"] == "REAL"
    assert root["refs"] == [], "a known leaf carries an explicit empty refs"
    assert out["refCount"] == 0
    assert out["truncated"] is False


def test_hierarchical_browse_rejects_unknown_refs_and_mode_mixing():
    c, _control, _health = commander()
    assert err_code(lambda: c.browse(None, {"ref": "ghost"})) == "BAD_ARGS"
    # `ref`/`depth`/`maxRefs` (hierarchical) and `cursor`/`max` (paged) are mutually exclusive.
    assert err_code(lambda: c.browse(None, {"ref": "root", "cursor": "page-2"})) == "BAD_ARGS"
    assert err_code(lambda: c.browse(None, {"depth": 2, "max": 10})) == "BAD_ARGS"
    # The hierarchical-only arguments are rejected without a `ref` — no silent paged fallback.
    assert err_code(lambda: c.browse(None, {"depth": 2})) == "BAD_ARGS"
    assert err_code(lambda: c.browse(None, {"maxRefs": 10})) == "BAD_ARGS"
    assert err_code(lambda: c.browse(None, {"ref": 7})) == "BAD_ARGS", "a non-string ref is malformed"


def test_hierarchical_browse_bounds_depth_and_max_refs():
    c, _control, _health = commander()
    # Out-of-range values are clamped into 1..4 / 1..1000, the same convention as the paged `max`.
    assert c.browse(None, {"ref": "root", "depth": 99, "maxRefs": 5000})["depth"] == 4
    assert c.browse(None, {"ref": "root", "depth": 0})["depth"] == 1


def test_hierarchical_browse_truncates_at_max_refs_across_pages():
    c, _control, _health = commander(browse="paged")
    # The paged seam serves two entries over two pages; maxRefs 1 truncates the root's refs.
    out = c.browse(None, {"ref": "root", "maxRefs": 1})
    assert out["refCount"] == 1
    assert out["truncated"] is True
    # The second page's entry is still resolvable as a leaf ref — the whole inventory is one tree.
    assert c.browse(None, {"ref": "pressure-1"})["root"]["nodeClass"] == "signal"


def test_hierarchical_browse_maps_the_seam_errors_to_the_same_codes():
    assert err_code(lambda: commander(browse="unsupported")[0].browse(None, {"ref": "root"})) == "BROWSE_UNSUPPORTED"
    assert err_code(lambda: commander(browse="failed")[0].browse(None, {"ref": "root"})) == "BROWSE_FAILED"
    assert err_code(lambda: commander(unavailable=True)[0].browse(None, {"ref": "root"})) == "DEVICE_UNAVAILABLE"


# --- pause / resume / repoll -------------------------------------------------------------------

def test_pause_is_idempotent_and_repoll_is_refused_while_paused():
    c, _control, health = commander()

    # repoll works while running.
    assert c.repoll(None)["polled"] == 2

    out = c.pause(None)
    assert out["paused"] is True
    assert out["changed"] is True
    assert health.is_paused()

    # repoll is refused while paused, with the dedicated PAUSED code.
    assert err_code(lambda: c.repoll(None)) == "PAUSED"

    # pausing again is idempotent.
    assert c.pause(None)["changed"] is False

    # resume clears it and repoll works again.
    out = c.resume(None)
    assert out["paused"] is False
    assert out["changed"] is True
    assert not health.is_paused()
    assert c.repoll(None)["polled"] == 2


# --- reconnect ---------------------------------------------------------------------------------

def test_reconnect_confirms_or_reports_reconnect_failed():
    c, _control, _health = commander()
    assert c.reconnect(None)["connected"] is True

    c2, _c2, _h2 = commander(reconnect_ok=False)
    assert err_code(lambda: c2.reconnect(None)) == "RECONNECT_FAILED"


def test_device_unavailable_when_the_seam_is_gone():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.reconnect(None)) == "DEVICE_UNAVAILABLE"


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


def test_the_overview_panel_carries_the_summary_rows_and_lifecycle_bindings():
    summary, lifecycle = panels()[0]["widgets"]
    assert (summary["kind"], summary["id"], summary["title"]) == \
        ("summary", "overview-summary", "Adapter overview")
    assert summary["rows"] == [
        {"label": "Signals", "value": "Configured signal inventory via cmd/sb/signals"},
        {"label": "Lifecycle", "value": "Pause, resume, reconnect, and repoll the instance"},
        {"label": "Writes", "value": "Allow-listed via writes.allow[]; checked before device I/O"},
    ]
    assert lifecycle == {"kind": "commandSummary", "id": "overview-lifecycle",
                         "title": "Lifecycle bindings",
                         "verbs": ["sb/status", "reconnect", "sb/pause", "sb/resume", "repoll"]}


def test_the_signal_grid_binds_sb_signals_through_both_verb_keys_and_never_a_write_verb():
    (grid,) = panels()[1]["widgets"]
    assert (grid["kind"], grid["id"], grid["title"]) == \
        ("signalGrid", "configured-signals", "Configured signals")
    assert grid["scope"] == "instance", "command-backed widgets repeat the view scope"
    assert grid["signalsVerb"] == "sb/signals"
    assert grid["subscriptionsVerb"] == "sb/signals", "the renderer-compat alias binds the same verb"
    assert grid["readVerb"] == "sb/read"
    assert "writeVerb" not in grid, "panels never advertise a write surface"


def test_the_diagnostics_tree_browser_is_hierarchical_and_bounded():
    tree, commands_widget = panels()[2]["widgets"]
    assert (tree["kind"], tree["id"], tree["title"]) == ("treeBrowser", "inventory-tree", "Inventory")
    assert tree["scope"] == "instance"
    assert (tree["mode"], tree["rootRef"]) == ("hierarchical", "root")
    assert (tree["depth"], tree["maxRefs"]) == (1, 200)
    assert (tree["browseVerb"], tree["readVerb"]) == ("sb/browse", "sb/read")
    assert "writeVerb" not in tree, "panels never advertise a write surface"
    assert commands_widget == {"kind": "commandSummary", "id": "diagnostic-commands",
                               "title": "Diagnostic commands", "verbs": ["sb/status", "sb/browse"]}


class _FakeRequest:
    def __init__(self, body):
        self._body = body

    def get_body(self):
        return self._body


class _FakeCommands:
    def __init__(self):
        self.handlers = {}
        self.scopes = {}
        self.panels = []

    def register(self, verb, scope, handler):
        self.handlers[verb] = handler
        self.scopes[verb] = scope

    def register_panel(self, panel):
        self.panels.append(panel)


# --- signal-ref resolution + the remaining coded error branches --------------------------------


def test_read_resolves_a_signal_by_id_and_flags_a_ref_with_no_recognizable_key():
    c, _control, _health = commander()
    out = c.read(None, {"signals": [{"id": "temperature-1"}, {"nonsense": 1}]})
    reads = out["reads"]
    assert reads[0]["signal"]["id"] == "temperature-1", "{'id': ...} resolves directly"
    assert reads[1]["quality"] == "BAD" and reads[1]["qualityRaw"] == "UNRESOLVED_REF"


def test_a_resolved_ref_the_device_returns_no_value_for_is_marked_no_data():
    c, _control, _health = commander(read_empty=True)
    out = c.read(None, {"signals": [{"signalId": "temperature-1"}]})
    assert out["reads"][0]["qualityRaw"] == "NO_DATA", "resolved, requested, but nothing came back"


def test_read_maps_a_missing_device_to_device_unavailable():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.read(None, {"signals": [{"signalId": "temperature-1"}]})) == "DEVICE_UNAVAILABLE"


def test_write_flags_an_unresolved_entry_and_an_allow_listed_entry_with_no_value():
    c, control, health = commander()
    # An entry with no recognizable ref, and an allow-listed ref with no `value`: both are per-entry
    # results, neither reaches the device, and (nothing attempted) the call still succeeds overall.
    out = c.write(None, {"writes": [{"value": 1}, {"signalId": "setpoint-1"}]})
    errors = {r.get("error") for r in out["results"]}
    assert "unresolved ref" in errors
    assert "missing value" in errors
    assert out["written"] == 0
    assert control.writes == []
    assert health.take_write_errors() == 0, "entries that never reach the device are not write errors"


def test_write_maps_a_missing_device_to_device_unavailable():
    c, _control, health = commander(unavailable=True)
    assert err_code(lambda: c.write(None, {"signalId": "setpoint-1", "value": 1})) == "DEVICE_UNAVAILABLE"
    # The attempted (allow-listed) entry was aborted on the device path — it counts as a write error.
    assert health.take_write_errors() == 1


def test_browse_carries_a_next_cursor_when_the_page_is_partial_and_maps_unavailable():
    c, _control, _health = commander(browse="paged")
    out = c.browse(None, {})
    assert out["cursor"] == "page-2", "a partial page advertises where to resume"

    c2, _c2, _h2 = commander(unavailable=True)
    assert err_code(lambda: c2.browse(None, {})) == "DEVICE_UNAVAILABLE"


def test_pause_and_resume_map_a_missing_device_to_device_unavailable():
    c, _control, _health = commander(unavailable=True)
    assert err_code(lambda: c.pause(None)) == "DEVICE_UNAVAILABLE"
    assert err_code(lambda: c.resume(None)) == "DEVICE_UNAVAILABLE"


def test_repoll_maps_a_refusal_to_bad_args_and_a_missing_device_to_device_unavailable():
    # A generic seam refusal (RepollRefused) stays BAD_ARGS; only the paused gate is PAUSED.
    c, _control, _health = commander(repoll_refused=True)
    assert err_code(lambda: c.repoll(None)) == "BAD_ARGS"

    c2, _c2, _h2 = commander(unavailable=True)
    assert err_code(lambda: c2.repoll(None)) == "DEVICE_UNAVAILABLE"


def test_a_request_whose_body_cannot_be_read_is_treated_as_an_empty_body():
    # `_body` must never crash a body-reading handler: a malformed request degrades to an empty
    # dict, which `sb/browse` then serves as a default paged browse.
    handle, _control, _health = make_handle()
    commands = _FakeCommands()
    register_all(commands, [handle])

    class _Broken:
        def get_body(self):
            raise RuntimeError("body decode failed")

    out = commands.handlers["sb/browse"](_Broken(), None)
    assert out["id"] == "plc-1"
    assert out["entries"][0]["id"] == "temperature-1"


def test_register_all_registers_the_nine_verbs_and_three_panels_and_dispatches():
    handle, _control, _health = make_handle()
    commands = _FakeCommands()
    register_all(commands, [handle])

    assert sorted(commands.handlers) == sorted(NINE_VERBS)
    assert [p["id"] for p in commands.panels] == ["overview", "signals", "diagnostics"]

    # A registered handler dispatches into the commander and returns the verb result.
    out = commands.handlers["sb/status"](_FakeRequest({}), None)
    assert out["id"] == "plc-1"


def test_every_verb_declares_instance_scope():
    # Every sb/* verb acts on ONE device, so each declares CommandScope.INSTANCE — the inbox then
    # resolves the topic/body addressing before the handler runs and hands over the instance.
    handle, _control, _health = make_handle()
    commands = _FakeCommands()
    register_all(commands, [handle])
    assert commands.scopes == {verb: CommandScope.INSTANCE for verb in NINE_VERBS}


def test_a_handler_routes_on_the_addressed_instance_the_library_passes_it():
    # The handler ignores the body entirely: the instance it acts on is the one the inbox resolved.
    h1, _c1, _h1 = make_handle(cfg=a_device_cfg("plc-1"))
    h2, _c2, _h2 = make_handle(cfg=a_device_cfg("plc-2"))
    commands = _FakeCommands()
    register_all(commands, [h1, h2])

    status = commands.handlers["sb/status"]
    assert status(_FakeRequest({}), "plc-2")["id"] == "plc-2"
    # A body `instance` is NOT read here — with two devices and no addressed instance it is BAD_ARGS,
    # even though the body names one (the library would have folded it into addressed_instance).
    assert err_code(lambda: status(_FakeRequest({"instance": "plc-2"}), None)) == "BAD_ARGS"
    assert err_code(lambda: status(_FakeRequest({}), "ghost")) == "NO_SUCH_INSTANCE"


# --- scoped delivery end-to-end, through a REAL command inbox ----------------------------------
#
# The registrations above declare CommandScope.INSTANCE; these prove what that buys, by driving a
# live `CommandInbox` over fake messaging/config seams: a component-addressed topic and an
# instance-addressed topic both reach the verb, the instance token routes, and a body `instance`
# that conflicts with the topic is refused by the LIBRARY — the handler never runs.

DEVICE = "test-thing"
COMPONENT = "TestComponent"
REPLY_TO = "edgecommons/reply-test-1"


class _FakeConfigManager:
    """Minimal ConfigManager stand-in: identity resolution + reply stamping."""

    def __init__(self):
        self._identity = MessageIdentity([HierEntry("device", DEVICE)], COMPONENT)

    def get_component_identity(self):
        return self._identity

    def is_topic_include_root(self):
        return False

    def get_tag_config(self):
        return None

    def get_thing_name(self):
        return DEVICE

    def get_component_name(self):
        return COMPONENT


class _FakeMessaging:
    """Records subscriptions and replies. `simulate_message` delivers to every subscription whose
    filter matches the concrete topic through the real MQTT wildcard matcher, so the inbox's two
    `cmd/#` subscriptions receive concrete verb topics exactly like a broker would."""

    def __init__(self):
        self._callbacks = {}
        self.replies = []

    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None):
        self._callbacks[topic] = callback

    def subscribe_acknowledged(self, topic, callback, max_concurrency=None, max_messages=None,
                               timeout_secs=10.0):
        self.subscribe(topic, callback, max_concurrency, max_messages)

    def unsubscribe(self, topic):
        self._callbacks.pop(topic, None)

    def simulate_message(self, topic, message):
        for sub, callback in list(self._callbacks.items()):
            if sub == topic or MessagingClient.topic_matches_sub(sub, topic):
                callback(topic, message)

    def reply(self, request, reply):
        self.replies.append(reply)


def _request(verb, body=None):
    message = MessageBuilder.create(verb, "1.0").with_payload(body if body is not None else {}).build()
    message.make_request(REPLY_TO)
    return message


class _Inbox:
    """A live inbox with the nine verbs registered over one or more devices."""

    def __init__(self, *cfgs):
        cfgs = cfgs or (a_device_cfg(),)
        self.handles, self.controls, self.healths = [], [], []
        for cfg in cfgs:
            handle, control, health = make_handle(cfg=cfg)
            self.handles.append(handle)
            self.controls.append(control)
            self.healths.append(health)
        self.messaging = _FakeMessaging()
        self.inbox = CommandInbox(_FakeConfigManager(), self.messaging, lambda: 1,
                                  lambda: True, lambda: None)
        register_all(self.inbox, self.handles)
        self.inbox.start()

    def send(self, verb, instance=None, body=None):
        prefix = f"ecv1/{DEVICE}/{COMPONENT}"
        topic = f"{prefix}/{instance}/cmd/{verb}" if instance else f"{prefix}/cmd/{verb}"
        self.messaging.simulate_message(topic, _request(verb, body))
        assert len(self.messaging.replies) == 1, "exactly one reply expected"
        return self.messaging.replies.pop().get_body()


def test_a_component_addressed_and_an_instance_addressed_request_both_reach_the_verb():
    # D-U28: the inbox subscribes both cmd wildcards. With one device configured, the
    # component-addressed delivery resolves to it (no instance named anywhere); the
    # instance-addressed one carries the token on the topic.
    io = _Inbox()
    body = io.send("sb/status")
    assert body["ok"] is True and body["result"]["id"] == "plc-1"

    body = io.send("sb/status", instance="plc-1")
    assert body["ok"] is True and body["result"]["id"] == "plc-1"


def test_the_topic_instance_token_selects_the_device_among_several():
    io = _Inbox(a_device_cfg("plc-1"), a_device_cfg("plc-2"))
    assert io.send("sb/status", instance="plc-2")["result"]["id"] == "plc-2"
    # No instance anywhere, two devices: only the component can answer that — BAD_ARGS from here.
    body = io.send("sb/status")
    assert body["ok"] is False and body["error"]["code"] == "BAD_ARGS"
    # An instance this component does not have is NO_SUCH_INSTANCE, also from here.
    body = io.send("sb/status", instance="ghost")
    assert body["ok"] is False and body["error"]["code"] == "NO_SUCH_INSTANCE"


def test_a_body_instance_conflicting_with_the_topic_is_refused_by_the_library():
    # The conflict is the LIBRARY's call, made before dispatch: the reply is BAD_ARGS and the
    # handler never ran — the device was never paused and the control seam saw nothing.
    io = _Inbox(a_device_cfg("plc-1"), a_device_cfg("plc-2"))
    body = io.send("sb/pause", instance="plc-1", body={"instance": "plc-2"})
    assert body["ok"] is False
    assert body["error"]["code"] == "BAD_ARGS"
    for control in io.controls:
        assert control.calls == [], "no handler may run on an addressing error"
    for health in io.healths:
        assert not health.is_paused()


def test_a_body_instance_alone_still_addresses_the_verb():
    # A component-addressed delivery whose body names an instance: the library folds the body into
    # `addressed_instance`, so the handler routes on it without ever reading the body.
    io = _Inbox(a_device_cfg("plc-1"), a_device_cfg("plc-2"))
    assert io.send("sb/status", body={"instance": "plc-2"})["result"]["id"] == "plc-2"


def test_describe_advertises_every_verb_as_instance_scoped():
    # The console derives its UI from the advertised scope: `instance` means "show an instance
    # selector". Every verb this adapter registers says so on the wire.
    io = _Inbox()
    entries = io.send("describe")["result"]["commands"]
    scopes = {e["verb"]: e["scope"] for e in entries}
    for verb in NINE_VERBS:
        assert scopes[verb] == "instance", f"{verb} must advertise instance scope"
