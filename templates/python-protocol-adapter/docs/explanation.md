# Explanation — How the adapter archetype works, and why

*This documents the generated scaffold; rewrite it as you build the component out.*

This page is the mental model for the **protocol-adapter** archetype. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## The southbound contract

An adapter is a *producer* of the cross-language **southbound contract** (`docs/SOUTHBOUND.md`): it
publishes a normalized `SouthboundSignalUpdate` envelope, exposes a read/write/browse command
surface, and emits `southbound_health` plus operational metrics. The cloud sees the same shape
regardless of protocol — only `device.adapter`, the opaque `signal.address`/`signal.id`, and any
protocol-specific metric families differ.

```text
    connect ──► poll ──► publish SouthboundSignalUpdate ──► report health
       ▲                                                        │
       └──────────── reconnect with backoff ◄───────────────────┘
```

## The device seam — the one boundary that matters

`<<SNAKENAME>>/device.py` defines `DeviceSession` (one live connection to one device) and
`DeviceBackend` (opens sessions). Implement it once per protocol, and **nothing above it learns your
protocol** — the connect/poll/reconnect worker (`adapter.py`), the command surface
(`command_service.py`), and the metrics (`metrics.py`) are all written against the abstraction.

**The boundary rule, worth enforcing in review:** a backend knows protocols. It does **not** know
EdgeCommons topics, the UNS, message envelopes, or metrics — `device.py` deliberately imports nothing
from `edgecommons`. The mapping from a protocol `Reading` to a `SouthboundSignalUpdate` lives one
layer up, in `adapter.py`. If your `DeviceSession` starts importing `edgecommons.uns`, the seam has
leaked.

## Signals, not tags

A **signal** is one data point — a measured value with identity, quality, and timestamps. (OPC UA
calls it a "tag"; Modbus calls it a "register".) The word "tag" is reserved in EdgeCommons for the
envelope's *business metadata*, which is a different thing entirely — see the org convention in
`AGENTS.md`.

## Quality is not optional

Every sample carries a `quality` normalized to `GOOD | BAD | UNCERTAIN`, plus the native code in
`quality_raw` for diagnosis. This is what lets a consumer gate on quality without knowing your
protocol — and it is why a read failure is published as a `BAD` sample rather than swallowed: a
signal that silently stops updating is indistinguishable from one that is simply not changing. The
simulator demonstrates this deliberately: `pressure-1` always reads `BAD`/`SENSOR_FAULT`, so a fresh
scaffold shows both the good and the bad path on the very first run.

## One worker per device

One `Device` instance runs one worker thread per `component.instances[]` entry: an instance is one
device, and its connection lifecycle is its own. A device going offline never disturbs another — each
has its own connect/backoff loop (`Backoff`: exponential with full jitter, so a site whose PLC
reboots doesn't get every adapter reconnecting in lockstep on the same second).

The device's session is **serialized** behind that worker's lock (`_session_lock`) — the command
surface never touches the session directly. Every session-touching verb (`sb/read`, `sb/write`,
`sb/browse`, `reconnect`, `repoll`) is routed to the device's control seam (`Device.read_now` /
`.write` / `.browse` / `.reconnect` / `.repoll`) and **confirmed** through what it returns, so a
command can never race a poll read on the same connection.

## Health — one source, several surfaces

`Health` (`adapter.py`) is the shared per-device state that feeds `southbound_health`
(gauges/counters), the `state` keepalive's `instances[]` (via `connectivity_of`), and the `sb/status`
reply. One source, several surfaces — so a health dot, a metric, and a status reply can never
disagree. The adapter's own richer link vocabulary (`CONNECTING`/`ONLINE`/`BACKOFF`) rides alongside
the normalized `connected` boolean, because a boolean can't tell "still trying" from "backing off
after a failure" and an operator needs to.

## Instance routing

`body.instance` is optional **iff** exactly one device is configured (D-EIP-13 convention); with two
or more, a missing id is `BAD_ARGS` and an unknown id is `NO_SUCH_INSTANCE`. This is why the tutorial
never needs `instance` in its requests — the scaffold ships one device.

## The command surface, and why the allow-list comes first

`command_service.py` registers the generic `sb/*` family every adapter serves —
`sb/status`/`sb/read`/`sb/write`/`sb/signals`/`sb/browse`/`sb/pause`/`sb/resume`/`reconnect`/`repoll`
— dimensioned into the `<<COMPONENTNAME>>Command` metric family by `verb`×`result`. `sb/write`
checks the write allow-list **before any device I/O**: a refused entry never reaches your
`write_signal()`. An adapter that writes whatever it is asked to is a control-system vulnerability,
not a feature — this ordering is not negotiable.

## The operational-metrics pattern

`southbound_health` is the exact SOUTHBOUND.md §5 measure set — `connectionState`,
`publishLatencyMs`, `pollLatencyMs`, `readErrors`, `staleSignals`, plus the optional `reconnects` —
every adapter emits it, regardless of protocol. On top of that, `<<COMPONENTNAME>>Connection` and
`<<COMPONENTNAME>>Command` are the **worked operational-family pattern**: a `Total`/`Interval`
counter pair per measure (monotonic since start, reset on each emit), low-cardinality dimensions
only. Your protocol likely wants `Inventory`/`Poll`/`Publish` families of its own — see the
[how-to guide](how-to-guides.md#add-your-protocols-metric-families).

## Panels

Three edge-console panel descriptors (`overview`, `signals`, `diagnostics`; order 10/20/30,
`scope: "instance"`) are registered via `commands.register_panel` alongside the command verbs, bound
to the verbs above — so a console gets a working operator surface the moment this component is
deployed, before any custom UI work.

## UNS addressing

Topics follow `ecv1/{device}/{component}/{instance}/{class}[/channel]`, built and validated by the
library. Telemetry rides `data` (via the `data()` facade); the command surface rides the library's
`cmd` inbox; `state`/`metric`/`cfg` are library-owned reserved classes. A fleet consumer subscribes
one wildcard per class rather than per-adapter topic templates.
