"""The southbound command surface — the ``sb/*`` verbs + the three edge-console panels.

This module owns the whole ``gg.get_commands()`` registration for ``<<COMPONENTNAME>>``: ``sb/status``,
``sb/read``, ``sb/write``, ``sb/signals``, ``sb/browse``, ``sb/pause``, ``sb/resume``, ``reconnect``,
``repoll``. It is the generic southbound command family (SOUTHBOUND.md §2.2) every adapter serves — a
real adapter changes the *seam* behind it (``device.py`` + the control in ``adapter.py``), not this
surface.

## Conventions every verb follows

* **Instance routing (D-EIP-13):** ``body.instance`` is optional iff exactly one device is
  configured; with two or more, a missing id is ``BAD_ARGS`` and an unknown id is
  ``NO_SUCH_INSTANCE``.
* **Standardized error codes:** ``BAD_ARGS``, ``NO_SUCH_INSTANCE``, ``WRITE_NOT_ALLOWED``,
  ``WRITE_FAILED``, ``DEVICE_UNAVAILABLE``, ``READ_FAILED``, ``RECONNECT_FAILED``,
  ``BROWSE_UNSUPPORTED``, ``BROWSE_FAILED``.
* **The session is never touched here.** Every verb that reads/writes/reconnects/pauses is routed to
  the device's own control seam and *confirmed* through what it returns — the session lives in the
  device worker and is serialized there, never touched from the command thread.
* **``sb/write`` allow-lists BEFORE any device I/O.** A refused entry never reaches the control seam —
  an adapter that writes whatever it is asked to is a control-system vulnerability, not a feature.
* Every verb records into the ``<<COMPONENTNAME>>Command`` metric family
  (``instance``×``verb``×``result``).

Three panels (``overview``, ``signals``, ``diagnostics``) are registered via
``commands.register_panel`` for the edge-console descriptor surface — each ``scope: "instance"``,
``order`` 10/20/30.
"""
import time
from typing import Any, Dict, List, Optional, Tuple

from edgecommons.command_inbox import CommandException

from .device import (
    BrowseFailed,
    BrowsePage,
    BrowseUnsupported,
    DeviceUnavailable,
    ReadFailed,
    ReconnectFailed,
    RepollRefused,
    WriteRejected,
)


def panels() -> List[Dict[str, Any]]:
    """The three edge-console panel descriptors. Core validates ``id``/``title``/uniqueness; the
    widget kinds and bound verbs are console-interpreted, so they ride verbatim. ``order`` 10/20/30,
    ``scope: "instance"``."""
    return [
        {
            "id": "overview", "title": "Overview", "order": 10, "scope": "instance",
            "widgets": [
                {"kind": "summary", "fields": ["connected", "state", "paused", "endpoint"]},
                {"kind": "commandSummary", "actions": ["reconnect", "sb/pause", "sb/resume"]},
            ],
            "verbs": ["sb/status", "reconnect", "sb/pause", "sb/resume"],
        },
        {
            "id": "signals", "title": "Signals", "order": 20, "scope": "instance",
            "widgets": [{"kind": "signalGrid"}],
            "verbs": ["sb/signals", "sb/read", "sb/write", "repoll"],
        },
        {
            "id": "diagnostics", "title": "Diagnostics", "order": 30, "scope": "instance",
            "widgets": [{"kind": "treeBrowser"}, {"kind": "keyValueList"}],
            "verbs": ["sb/browse", "sb/status"],
        },
    ]


class DeviceHandle:
    """The per-device handles the command surface routes on: the config (routing, allow-list,
    inventory + endpoint), the control seam (session-touching verbs), the shared health
    (status/paused), and the metrics emitter (per-verb command counters).

    ``cfg`` must expose ``id``, ``adapter``, ``endpoint`` and ``permits(signal_id) -> bool``;
    ``health`` must expose ``link() -> str``, ``is_paused() -> bool`` and ``online() -> bool``;
    ``control`` is the seam described in the module docstring; ``signals`` is the ``sb/signals``
    inventory (a config/backend view, no device round-trip).
    """

    def __init__(self, cfg, control, health, dm, signals):
        self.cfg = cfg
        self.control = control
        self.health = health
        self.dm = dm
        self.signals = signals


def _device_unavailable() -> CommandException:
    return CommandException("DEVICE_UNAVAILABLE", "device task is unavailable")


def _bad_read(signal_id: str, raw: str) -> Dict[str, Any]:
    return {"signal": {"id": signal_id}, "value": None, "quality": "BAD", "qualityRaw": raw}


def _write_entries(body: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Normalize an ``sb/write`` body to a list of ``{ref…, value}`` entries: a ``writes`` array, or
    a single object carrying ``value`` (§2.2). Raises ``BAD_ARGS`` when neither form is present."""
    writes = body.get("writes")
    if isinstance(writes, list):
        return list(writes)
    if "value" in body:
        return [body]
    raise CommandException("BAD_ARGS", "expected a `writes` array or a single write object with `value`")


class Commander:
    """The command dispatcher: owns the per-device handles + the config order (for the
    single-instance default)."""

    def __init__(self, handles: List[DeviceHandle]):
        self._ids = [h.cfg.id for h in handles]
        self._devices = {h.cfg.id: h for h in handles}

    def _resolve(self, body: Dict[str, Any]) -> DeviceHandle:
        """Route to the addressed device (D-EIP-13): ``body.instance`` optional iff exactly one
        device is configured; with two or more a missing/unknown id is ``BAD_ARGS`` /
        ``NO_SUCH_INSTANCE``."""
        instance = body.get("instance")
        if instance is not None:
            device = self._devices.get(instance)
            if device is None:
                raise CommandException("NO_SUCH_INSTANCE", f"no configured device `{instance}`")
            return device
        if len(self._ids) == 1:
            return self._devices[self._ids[0]]
        raise CommandException("BAD_ARGS", "field `instance` is required when multiple devices are configured")

    @staticmethod
    def _resolve_ref(h: DeviceHandle, ref: Dict[str, Any]) -> Tuple[Optional[str], str]:
        """Resolve an ``sb/read``/``sb/write`` signal-ref to its stable id: ``{"signalId"}`` /
        ``{"id"}`` directly, or ``{"name"}`` looked up against the configured inventory. Returns
        ``(id, label)``; ``id`` is ``None`` for a BAD / unresolved entry, and ``label`` names it."""
        if isinstance(ref.get("signalId"), str):
            return ref["signalId"], ref["signalId"]
        if isinstance(ref.get("id"), str):
            return ref["id"], ref["id"]
        name = ref.get("name")
        if isinstance(name, str):
            for s in h.signals:
                if s.name == name:
                    return s.id, name
            return None, name
        return None, "<invalid ref>"

    # --- sb/status ---------------------------------------------------------------------------------

    def status(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        connected = h.health.online()
        paused = h.health.is_paused()
        state = "PAUSED" if (paused and connected) else h.health.link()
        out = {
            "id": h.cfg.id,
            "adapter": h.cfg.adapter,
            "connected": connected,
            "state": state,
            "paused": paused,
            "endpoint": h.cfg.endpoint,
            "metrics": h.dm.counters_view(),
        }
        h.dm.record_command("sb/status", True, _ms(started))
        return out

    # --- sb/signals (the configured inventory, no device I/O) --------------------------------------

    def signals(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        signals = [{"id": s.id, "name": s.name, "writable": h.cfg.permits(s.id)} for s in h.signals]
        h.dm.record_command("sb/signals", True, _ms(started))
        return {"id": h.cfg.id, "signals": signals}

    # --- sb/read (on-demand read of named signals) ------------------------------------------------

    def read(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        refs = body.get("signals")
        if not isinstance(refs, list):
            h.dm.record_command("sb/read", False, _ms(started))
            raise CommandException("BAD_ARGS", "expected a `signals` array")

        plan = [self._resolve_ref(h, r if isinstance(r, dict) else {}) for r in refs]
        ids = [sid for (sid, _label) in plan if sid is not None]

        readings: Dict[str, Any] = {}
        if ids:
            try:
                for r in h.control.read_now(ids):
                    readings[r.signal_id] = r
            except ReadFailed as e:
                h.dm.record_command("sb/read", False, _ms(started))
                raise CommandException("READ_FAILED", str(e))
            except DeviceUnavailable:
                h.dm.record_command("sb/read", False, _ms(started))
                raise _device_unavailable()

        reads = []
        for (sid, label) in plan:
            if sid is None:
                reads.append(_bad_read(label, "UNRESOLVED_REF"))
                continue
            r = readings.get(sid)
            if r is None:
                reads.append(_bad_read(sid, "NO_DATA"))
            else:
                reads.append({
                    "signal": {"id": sid},
                    "value": r.value,
                    "quality": r.quality,
                    "qualityRaw": r.quality_raw,
                })

        h.dm.record_command("sb/read", True, _ms(started))
        return {"id": h.cfg.id, "reads": reads}

    # --- sb/write (§2.2 batch shape; allow-list BEFORE any device I/O; confirmed) ------------------

    def write(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        entries = _write_entries(body)

        results: List[Dict[str, Any]] = []
        refused = 0
        attempted = 0
        succeeded = 0

        for entry in entries:
            entry = entry if isinstance(entry, dict) else {}
            sid, label = self._resolve_ref(h, entry)
            if sid is None:
                results.append({"signal": label, "ok": False, "error": "unresolved ref"})
                continue
            # THE ALLOW-LIST — checked here, BEFORE the write ever reaches the device.
            if not h.cfg.permits(sid):
                refused += 1
                results.append({"signal": sid, "ok": False, "error": "not in writes.allow"})
                continue
            if "value" not in entry:
                results.append({"signal": sid, "ok": False, "error": "missing value"})
                continue

            value = entry["value"]
            attempted += 1
            try:
                h.control.write(sid, value)
                succeeded += 1
                results.append({"signal": sid, "value": value, "ok": True})
            except WriteRejected as e:
                results.append({"signal": sid, "value": value, "ok": False, "error": str(e)})
            except DeviceUnavailable:
                h.dm.record_command("sb/write", False, _ms(started))
                raise _device_unavailable()

        # WRITE_NOT_ALLOWED only when EVERY entry was an allow-list refusal (nothing else attempted).
        if entries and refused == len(entries):
            h.dm.record_command("sb/write", False, _ms(started))
            raise CommandException("WRITE_NOT_ALLOWED", "no entry is in this instance's writes.allow list")
        # WRITE_FAILED when every allowed write reached the device and every one failed.
        if attempted > 0 and succeeded == 0:
            h.dm.record_command("sb/write", False, _ms(started))
            raise CommandException("WRITE_FAILED", "every attempted write was rejected by the device")

        h.dm.record_command("sb/write", True, _ms(started))
        return {"id": h.cfg.id, "written": succeeded, "results": results}

    # --- sb/browse (paged address-space discovery) ------------------------------------------------

    def browse(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        cursor = body.get("cursor") if isinstance(body.get("cursor"), str) else None
        max_entries = body.get("max")
        max_entries = int(max_entries) if isinstance(max_entries, int) else 200
        max_entries = max(1, min(1000, max_entries))

        try:
            page: BrowsePage = h.control.browse(cursor, max_entries)
        except BrowseUnsupported:
            h.dm.record_command("sb/browse", False, _ms(started))
            raise CommandException("BROWSE_UNSUPPORTED", "this adapter has no discovery service")
        except BrowseFailed as e:
            h.dm.record_command("sb/browse", False, _ms(started))
            raise CommandException("BROWSE_FAILED", str(e))
        except DeviceUnavailable:
            h.dm.record_command("sb/browse", False, _ms(started))
            raise _device_unavailable()

        out: Dict[str, Any] = {
            "id": h.cfg.id,
            "entries": [{"id": e.id, "name": e.name, "type": e.type_name} for e in page.entries],
        }
        if page.next_cursor is not None:
            out["cursor"] = page.next_cursor
        h.dm.record_command("sb/browse", True, _ms(started))
        return out

    # --- sb/pause + sb/resume (idempotent {paused, changed}) --------------------------------------

    def pause(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        try:
            changed = h.control.pause()
        except DeviceUnavailable:
            h.dm.record_command("sb/pause", False, _ms(started))
            raise _device_unavailable()
        h.dm.record_command("sb/pause", True, _ms(started))
        return {"id": h.cfg.id, "paused": True, "changed": changed}

    def resume(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        try:
            changed = h.control.resume()
        except DeviceUnavailable:
            h.dm.record_command("sb/resume", False, _ms(started))
            raise _device_unavailable()
        h.dm.record_command("sb/resume", True, _ms(started))
        return {"id": h.cfg.id, "paused": False, "changed": changed}

    # --- reconnect ---------------------------------------------------------------------------------

    def reconnect(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        try:
            h.control.reconnect()
        except ReconnectFailed as e:
            h.dm.record_command("reconnect", False, _ms(started))
            raise CommandException("RECONNECT_FAILED", str(e))
        except DeviceUnavailable:
            h.dm.record_command("reconnect", False, _ms(started))
            raise _device_unavailable()
        h.dm.record_command("reconnect", True, _ms(started))
        return {"id": h.cfg.id, "connected": True}

    # --- repoll (refused while paused) ------------------------------------------------------------

    def repoll(self, body: Dict[str, Any]) -> Dict[str, Any]:
        h = self._resolve(body)
        started = time.monotonic()
        if h.health.is_paused():
            h.dm.record_command("repoll", False, _ms(started))
            raise CommandException("BAD_ARGS", "instance is paused - resume first")
        try:
            polled = h.control.repoll()
        except RepollRefused as e:
            h.dm.record_command("repoll", False, _ms(started))
            raise CommandException("BAD_ARGS", str(e))
        except DeviceUnavailable:
            h.dm.record_command("repoll", False, _ms(started))
            raise _device_unavailable()
        h.dm.record_command("repoll", True, _ms(started))
        return {"id": h.cfg.id, "polled": polled}


def _ms(started: float) -> float:
    return (time.monotonic() - started) * 1000.0


def _body(request) -> Dict[str, Any]:
    """The request body as a dict (an empty dict for any non-dict / missing body)."""
    try:
        b = request.get_body()
    except Exception:  # noqa: BLE001 - a malformed request must never crash the handler
        return {}
    return b if isinstance(b, dict) else {}


def register_all(commands, handles: List[DeviceHandle]) -> None:
    """Register every ``sb/*`` verb + the three edge-console panels on the command inbox.

    ``commands`` is the ``gg.get_commands()`` facade (``CommandInbox``). Handlers return the verb
    result object (wrapped by the inbox as ``{"ok": true, "result": …}``) or raise
    :class:`~edgecommons.command_inbox.CommandException` for a coded error reply.
    """
    commander = Commander(handles)

    commands.register("sb/status", lambda req: commander.status(_body(req)))
    commands.register("sb/read", lambda req: commander.read(_body(req)))
    commands.register("sb/write", lambda req: commander.write(_body(req)))
    commands.register("sb/signals", lambda req: commander.signals(_body(req)))
    commands.register("sb/browse", lambda req: commander.browse(_body(req)))
    commands.register("sb/pause", lambda req: commander.pause(_body(req)))
    commands.register("sb/resume", lambda req: commander.resume(_body(req)))
    commands.register("reconnect", lambda req: commander.reconnect(_body(req)))
    commands.register("repoll", lambda req: commander.repoll(_body(req)))

    for panel in panels():
        commands.register_panel(panel)
