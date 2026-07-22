"""<<COMPONENTNAME>> — a southbound protocol adapter.

An **adapter** connects to devices, reads signals, and publishes them onto the UNS in the shape the
rest of the fleet expects — so a consumer can chart a Modbus register and an OPC UA node without
knowing either protocol::

    connect ──► poll ──► publish SouthboundSignalUpdate ──► report health
       ▲                                                        │
       └──────────── reconnect with backoff ◄───────────────────┘

One worker thread per instance: an instance is one device, and its connection lifecycle is its own.
The device's session is **serialized** behind that worker's lock — the command surface
(:mod:`command_service`) never touches the session directly; each session-touching verb is routed to
this device's control seam (the :class:`Device` methods ``read_now``/``write``/``browse``/
``reconnect``/``repoll``) and confirmed through what it returns.

## The contract you are implementing (docs/SOUTHBOUND.md)

* Publish ``SouthboundSignalUpdate`` on the ``data`` class, **via the ``data()`` facade** — never
  hand-build the body and never hand-write the topic.
* **Quality on every sample**, normalized to ``GOOD | BAD | UNCERTAIN``, with the native code in
  ``qualityRaw``.
* Emit **``southbound_health``** (the exact §5 set — see :mod:`metrics`), dimensioned by instance.
* Report **per-instance connectivity** (:func:`connectivity_of`).
* Serve **read/write/browse/reconnect/pause commands** — and allow-list the writes.
"""
import logging
import random
import threading
import time
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

from edgecommons.facades.quality import Quality as WireQuality
from edgecommons.facades.severity import Severity
from edgecommons.facades.signal_update import Sample
from edgecommons.facades.util import format_instant
from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity

from .command_service import DeviceHandle, register_all
from .device import (
    BrowseFailed,
    BrowseUnsupported,
    DeviceError,
    DeviceUnavailable,
    Quality,
    ReadFailed,
    ReconnectFailed,
    WriteRejected,
    make_backend,
)
from .metrics import DeviceMetrics

logger = logging.getLogger("<<COMPONENTNAME>>")

#: How often the periodic metrics emit runs, in seconds (SOUTHBOUND.md §5 cadence).
METRICS_INTERVAL = 30.0
#: The ``component.global.healthThresholds.staleSignalSecs`` default (SOUTHBOUND.md §4/§5).
DEFAULT_STALE_SIGNAL_SECS = 30
#: The ``component.global.defaults.pollIntervalMs`` default.
DEFAULT_POLL_MS = 5000

#: This adapter's OWN vocabulary for a link's condition. A boolean cannot tell "still trying" from
#: "backing off after a failure"; an operator needs to, so the richer token exists alongside the
#: normalized flag.
CONNECTING = "CONNECTING"   # connecting for the first time; nothing has failed yet
ONLINE = "ONLINE"           # the session is up and being polled
BACKOFF = "BACKOFF"         # the link failed; reconnecting with backoff


# =================================================================================================
# Config
# =================================================================================================

class DeviceConfig:
    """One device == one entry of ``component.instances[]``.

    ``connection`` is deliberately OPEN — every protocol needs different keys (a unit id, a security
    policy, a slave address). Writes are **allow-listed by stable ``signal.id``**: an empty list
    means this adapter is read-only, which is the correct default for anything touching a control
    system.
    """

    def __init__(self, id: str, adapter: str, connection: Dict[str, Any],
                 poll_interval_ms: int, allow: List[str]):
        self.id = id
        self.adapter = adapter
        self.connection = connection
        self.poll_interval_ms = poll_interval_ms
        self.allow = list(allow)

    @staticmethod
    def parse(instance_id: str, inst: Dict[str, Any], default_poll_ms: int) -> "DeviceConfig":
        connection = inst.get("connection")
        if not isinstance(connection, dict):
            raise ValueError("`connection` is required")
        allow = ((inst.get("writes") or {}).get("allow")) or []
        if not isinstance(allow, list):
            raise ValueError("`writes.allow` must be an array")
        poll = inst.get("pollIntervalMs")
        poll_ms = int(poll) if isinstance(poll, int) and poll > 0 else int(default_poll_ms)
        return DeviceConfig(
            id=instance_id,
            adapter=inst.get("adapter") or "sim",
            connection=connection,
            poll_interval_ms=poll_ms,
            allow=allow,
        )

    @property
    def endpoint(self) -> Optional[str]:
        return self.connection.get("endpoint")

    def permits(self, signal_id: str) -> bool:
        """Whether ``signal_id`` is on this device's write allow-list. Nothing else is writable,
        whatever a command asks for."""
        return signal_id in self.allow


# =================================================================================================
# Health — one source, several surfaces
# =================================================================================================

class Health:
    """The shared per-device state the metrics emitter reads and the connectivity provider renders.
    The gauges (``connection_state``, latencies) and the interval counters (``read_errors``,
    ``reconnects``) feed ``southbound_health`` (:mod:`metrics`); ``paused`` and ``link`` feed the
    connectivity token and ``sb/status``. One source, several surfaces — so a health dot, a metric,
    and a status reply can never disagree."""

    def __init__(self):
        self._lock = threading.Lock()
        self._link = CONNECTING
        self._connection_state = 0
        self._paused = False
        self._poll_latency_ms = 0
        self._publish_latency_ms = 0
        self._read_errors = 0
        self._reconnects = 0

    def set_link(self, state: str) -> None:
        """Record the link's condition. The metric's boolean and the reported state token move
        **together**, so the health dot and the label a console shows can never disagree."""
        with self._lock:
            self._link = state
            self._connection_state = 1 if state == ONLINE else 0

    def link(self) -> str:
        with self._lock:
            return self._link

    def online(self) -> bool:
        with self._lock:
            return self._link == ONLINE

    def connection_state(self) -> int:
        with self._lock:
            return self._connection_state

    def is_paused(self) -> bool:
        with self._lock:
            return self._paused

    def _swap_paused(self, paused: bool) -> bool:
        with self._lock:
            changed = self._paused != paused
            self._paused = paused
            return changed

    def poll_latency_ms(self) -> int:
        with self._lock:
            return self._poll_latency_ms

    def publish_latency_ms(self) -> int:
        with self._lock:
            return self._publish_latency_ms

    def set_poll_latency(self, ms: int) -> None:
        with self._lock:
            self._poll_latency_ms = int(ms)

    def set_publish_latency(self, ms: int) -> None:
        with self._lock:
            self._publish_latency_ms = int(ms)

    def incr_read_error(self) -> None:
        with self._lock:
            self._read_errors += 1

    def incr_reconnect(self) -> None:
        with self._lock:
            self._reconnects += 1

    def take_read_errors(self) -> int:
        with self._lock:
            v = self._read_errors
            self._read_errors = 0
            return v

    def take_reconnects(self) -> int:
        with self._lock:
            v = self._reconnects
            self._reconnects = 0
            return v


def set_paused(health: Health, paused: bool) -> bool:
    """Flip the paused flag, returning whether the state actually changed (idempotent — pausing an
    already-paused device is not an error). The event is emitted by the caller, which holds the
    ``events()`` facade."""
    return health._swap_paused(paused)


def connectivity_of(cfg: DeviceConfig, health: Health) -> InstanceConnectivity:
    """One device's connectivity sample, for the instance-connectivity provider.

    * ``connected`` is the **normalized** flag — always present.
    * ``state`` is *this adapter's* vocabulary — ``PAUSED`` when paused and up, else the raw link
      token (so a break while paused still reads ``BACKOFF``, ``connected`` staying truthful).
    * ``attributes`` is the **open** bag: domain data only this adapter understands.
    """
    connected = health.online()
    paused = health.is_paused()
    state = "PAUSED" if (paused and connected) else health.link()
    return (
        InstanceConnectivity.of(cfg.id, connected, cfg.endpoint)
        .with_state(state)
        .with_attributes({"adapter": cfg.adapter, "paused": paused})
    )


# =================================================================================================
# Backoff
# =================================================================================================

class Backoff:
    """Reconnect backoff. Exponential with full jitter and a cap — so a site whose PLC reboots does
    not get every adapter in the plant reconnecting in lockstep on the same second."""

    def __init__(self, base_ms: int = 1000, max_ms: int = 60000):
        self.base_ms = base_ms
        self.max_ms = max_ms

    def delay_secs(self, attempt: int) -> float:
        exp = self.base_ms * (1 << min(attempt, 20))
        cap = min(exp, self.max_ms)
        return (random.random() * cap) / 1000.0


class _LinkLost(Exception):
    """Internal: a poll read broke the connection; the loop reconnects."""


# =================================================================================================
# The device worker + control seam
# =================================================================================================

class Device:
    """One device's lifecycle: connect, poll, publish, reconnect — and serve the control seam the
    command surface routes on. The session is serialized behind ``_session_lock``, so a command
    (write/read/browse) can never race a poll read on the same connection."""

    def __init__(self, gg, cfg: DeviceConfig, stale_signal_secs: int):
        self._gg = gg
        self._cfg = cfg
        # The instance-scoped facades: data()/events() mint this instance's data/evt topics and
        # stamp the config-resolved identity with this instance token.
        instance = gg.instance(cfg.id)
        self._data = instance.data()
        self._events = instance.events()
        self._backend = make_backend(cfg.adapter)
        self._health = Health()
        self._dm = DeviceMetrics(gg.get_metrics(), gg.get_config_manager(), cfg.id, self._health,
                                 stale_signal_secs)
        self._dm.define_all()
        # The signal inventory `sb/signals` shows — a config/backend view, no device round-trip.
        self._signals = self._backend.inventory(cfg.connection) if self._backend is not None else []
        self._session = None
        self._session_lock = threading.RLock()
        self._stop = threading.Event()
        self._attempt = 0
        self._backoff = Backoff()
        self._last_metrics = time.monotonic()
        self._thread = threading.Thread(target=self._run, name=f"device-{cfg.id}", daemon=True)

    # ---- lifecycle -------------------------------------------------------------------------------

    def start(self) -> None:  # pragma: no cover - live-runtime seam: spawns the connect/poll worker thread; the loop it runs is exercised by tests/test_live_sim.py on real infra, not offline.
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        with self._session_lock:
            if self._session is not None:
                try:
                    self._session.close()
                except Exception:  # noqa: BLE001
                    pass
                self._session = None

    def handle(self) -> DeviceHandle:
        return DeviceHandle(cfg=self._cfg, control=self, health=self._health, dm=self._dm,
                            signals=self._signals)

    def connectivity(self) -> InstanceConnectivity:
        return connectivity_of(self._cfg, self._health)

    # ---- the connect/poll loop -------------------------------------------------------------------

    def _run(self) -> None:  # pragma: no cover - live-runtime seam: the connect-retry supervisor, an infinite connect/poll/reconnect loop driven by a real device session and wall-clock waits; exercised by tests/test_live_sim.py on real infra (edgecommons AGENTS.md validation matrix), not offline unit tests. The step methods it drives (_connect_once, _poll_tick, _poll_once, _publish_reading, _on_drop) are unit-tested directly against the in-process sim backend.
        if self._backend is None:
            logger.error("[%s] unknown adapter '%s' — worker not started", self._cfg.id,
                         self._cfg.adapter)
            return
        while not self._stop.is_set():
            if self._session is None:
                self._connect_once()
                continue
            if not self._health.is_paused():
                self._poll_tick()
            self._stop.wait(self._cfg.poll_interval_ms / 1000.0)
            if time.monotonic() - self._last_metrics >= METRICS_INTERVAL:
                self._dm.emit_periodic()
                self._last_metrics = time.monotonic()

    def _connect_once(self) -> None:
        self._dm.on_connect_attempt()
        self._health.set_link(CONNECTING if self._attempt == 0 else BACKOFF)
        try:
            session = self._backend.connect(self._cfg.connection)
        except DeviceError as e:
            self._dm.on_connect_failure()
            self._health.set_link(BACKOFF)
            permanent = not e.transient
            wait = (self._backoff.max_ms / 1000.0) if permanent else self._backoff.delay_secs(self._attempt)
            self._attempt += 1
            logger.warning("[%s] connect failed (permanent=%s, wait=%.1fs): %s", self._cfg.id,
                           permanent, wait, e)
            self._stop.wait(wait)
            return
        with self._session_lock:
            self._session = session
        self._attempt = 0
        self._dm.on_connected(time.monotonic())
        self._health.set_link(ONLINE)
        self._dm.emit_now()
        try:
            self._events.emit("device-connected", f"connected to {self._cfg.endpoint}",
                              {"instance": self._cfg.id, "adapter": self._backend.kind()},
                              Severity.INFO)
            self._events.clear_alarm("device-unreachable")
        except Exception:  # noqa: BLE001 - an event outage must not kill the loop
            pass

    def _poll_tick(self) -> None:
        broke = False
        with self._session_lock:
            if self._session is not None:
                try:
                    self._poll_once(self._session)
                except _LinkLost:
                    try:
                        self._session.close()
                    except Exception:  # noqa: BLE001
                        pass
                    self._session = None
                    broke = True
        if broke:
            self._on_drop()

    def _on_drop(self) -> None:
        self._health.set_link(BACKOFF)
        self._health.incr_reconnect()
        self._dm.on_connection_dropped(time.monotonic())
        self._dm.emit_now()
        try:
            self._events.raise_alarm("device-unreachable", f"lost the link to {self._cfg.endpoint}",
                                     {"instance": self._cfg.id})
        except Exception:  # noqa: BLE001
            pass

    def _poll_once(self, session) -> int:
        """One poll: read, publish each reading, record latencies + staleness. Returns the number of
        signals published; raises :class:`_LinkLost` when the *connection* broke."""
        started = time.monotonic()
        try:
            readings = session.read_signals()
        except DeviceError as e:
            logger.warning("[%s] read failed; reconnecting: %s", self._cfg.id, e)
            self._health.incr_read_error()
            raise _LinkLost()
        self._health.set_poll_latency((time.monotonic() - started) * 1000.0)

        publish_started = time.monotonic()
        published = 0
        for r in readings:
            try:
                self._publish_reading(r)
                published += 1
                self._dm.on_signal_update(r.signal_id, time.monotonic())
            except Exception as e:  # noqa: BLE001 - a publish failure must not kill the poll loop
                logger.warning("[%s] publish of '%s' failed: %s", self._cfg.id, r.signal_id, e)
        self._health.set_publish_latency((time.monotonic() - publish_started) * 1000.0)
        return published

    def _publish_reading(self, r) -> None:
        """Publish one reading as a ``SouthboundSignalUpdate`` through the ``data()`` facade — which
        builds the body, mints the topic, and stamps identity. A failed read carries no value at
        all, which the facade's ``samples[]`` cannot express, so it rides the pre-built-body path."""
        wire_quality = _WIRE_QUALITY.get(r.quality, WireQuality.GOOD)
        if r.value is not None:
            builder = self._data.signal(r.signal_id)
            if r.name is not None:
                builder = builder.name(r.name)
            builder = builder.device(adapter=self._cfg.adapter, instance=self._cfg.id,
                                     endpoint=self._cfg.endpoint)
            builder.add_sample(Sample(r.value, wire_quality, r.quality_raw)).signal_path(r.signal_id).publish()
        else:
            body = {
                "device": {"adapter": self._cfg.adapter, "instance": self._cfg.id,
                           "endpoint": self._cfg.endpoint},
                "signal": {"id": r.signal_id, **({"name": r.name} if r.name is not None else {})},
                "samples": [{
                    "value": None,
                    "quality": wire_quality.wire(),
                    "qualityRaw": r.quality_raw if r.quality_raw is not None else "unspecified",
                    "serverTs": format_instant(datetime.now(timezone.utc)),
                }],
            }
            self._data.publish_body(r.signal_id, body)

    # ---- the control seam (called from the command thread; see command_service) ------------------

    def read_now(self, ids: List[str]):
        with self._session_lock:
            if self._session is None:
                raise DeviceUnavailable()
            try:
                return self._session.read_named(ids)
            except DeviceError as e:
                self._health.incr_read_error()
                raise ReadFailed(str(e))

    def write(self, signal_id: str, value: Any) -> None:
        with self._session_lock:
            if self._session is None:
                raise DeviceUnavailable()
            try:
                self._session.write_signal(signal_id, value)
            except DeviceError as e:
                raise WriteRejected(str(e))

    def browse(self, cursor: Optional[str], max_entries: int):
        with self._session_lock:
            if self._session is None:
                raise DeviceUnavailable()
            try:
                return self._session.browse(cursor, max_entries)
            except (BrowseUnsupported, BrowseFailed):
                raise
            except DeviceError as e:
                raise BrowseFailed(str(e))

    def pause(self) -> bool:
        changed = set_paused(self._health, True)
        if changed:
            try:
                self._events.emit("adapter-paused", "telemetry production paused",
                                  {"instance": self._cfg.id}, Severity.WARNING)
            except Exception:  # noqa: BLE001
                pass
        return changed

    def resume(self) -> bool:
        changed = set_paused(self._health, False)
        if changed:
            try:
                self._events.emit("adapter-resumed", "telemetry production resumed",
                                  {"instance": self._cfg.id}, Severity.INFO)
            except Exception:  # noqa: BLE001
                pass
        return changed

    def reconnect(self) -> None:
        with self._session_lock:
            if self._session is not None:
                try:
                    self._session.close()
                except Exception:  # noqa: BLE001
                    pass
                self._session = None
            self._dm.on_connect_attempt()
            try:
                session = self._backend.connect(self._cfg.connection)
            except DeviceError as e:
                self._dm.on_connect_failure()
                self._health.set_link(BACKOFF)
                raise ReconnectFailed(str(e))
            self._session = session
            self._attempt = 0
            self._dm.on_connected(time.monotonic())
            self._health.set_link(ONLINE)
            self._dm.emit_now()

    def repoll(self) -> int:
        with self._session_lock:
            if self._session is None:
                raise DeviceUnavailable("device is disconnected")
            try:
                return self._poll_once(self._session)
            except _LinkLost:
                try:
                    self._session.close()
                except Exception:  # noqa: BLE001
                    pass
                self._session = None
                self._on_drop()
                raise DeviceUnavailable("link error")


_WIRE_QUALITY = {
    Quality.GOOD: WireQuality.GOOD,
    Quality.BAD: WireQuality.BAD,
    Quality.UNCERTAIN: WireQuality.UNCERTAIN,
}


# =================================================================================================
# App
# =================================================================================================

class App:
    """Builds one :class:`Device` per ``component.instances[]`` entry, wires the connectivity
    provider and the ``sb/*`` command surface, and blocks until shutdown."""

    def __init__(self, gg):
        self._gg = gg
        cm = gg.get_config_manager()
        global_cfg = cm.get_global_config() or {}
        thresholds = global_cfg.get("healthThresholds") or {}
        self._stale_secs = int(thresholds.get("staleSignalSecs") or DEFAULT_STALE_SIGNAL_SECS)
        default_poll = int((global_cfg.get("defaults") or {}).get("pollIntervalMs") or DEFAULT_POLL_MS)

        self._devices: List[Device] = []
        for instance_id in cm.get_instance_ids():
            inst = cm.get_instance_config(instance_id) or {}
            try:
                cfg = DeviceConfig.parse(instance_id, inst, default_poll)
            except Exception as e:  # noqa: BLE001
                logger.warning("skipping malformed device '%s': %s", instance_id, e)
                continue
            self._devices.append(Device(gg, cfg, self._stale_secs))

        if not self._devices:
            raise RuntimeError("no valid devices in component.instances[]")

    def run(self) -> None:  # pragma: no cover - live-runtime seam: registers the connectivity provider + sb/* command surface, starts each device's worker thread, and blocks until the library's signal hook exits; exercised by the HOST/GREENGRASS smoke, not offline unit tests. App.__init__ (config -> devices) and the pieces this wires (connectivity, register_all, Device.start) are covered separately.
        gg = self._gg

        # ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
        # instances[] every tick, and returns the very same sample from the built-in `status`
        # command verb. Whoever watches and whoever asks cannot get different answers.
        gg.set_instance_connectivity_provider(lambda: [d.connectivity() for d in self._devices])

        # The southbound command surface (command_service). `ping`/`status`/`reload-config`/
        # `get-configuration` are already live — the library registered them before we ran.
        commands = gg.get_commands()
        if commands is not None:
            register_all(commands, [d.handle() for d in self._devices])
            logger.info("command verbs registered: %s", sorted(commands.verbs()))
        else:
            logger.warning("no command inbox (unresolved identity) — command surface disabled")

        for d in self._devices:
            d.start()
        gg.set_ready(True)

        try:
            threading.Event().wait()  # block until the library's signal hook exits the process
        finally:
            for d in self._devices:
                try:
                    d.stop()
                except Exception:  # noqa: BLE001
                    pass
            gg.shutdown()
