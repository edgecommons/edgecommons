# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic and message the adapter publishes or accepts, and the CLI flags. Addressing follows the
Unified Namespace: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the model behind
this, see [explanation.md](../explanation.md); for the type/quality system, see
[data-types.md](data-types.md); for client recipes, the [how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (the last `hierarchy` level, or `-t` directly).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — a device instance id (`device-1`, …) for `data`/`evt`; the shared command inbox,
  `state`, and `metric` are component-scope (no instance token in the topic).

## Envelope

All messages use the EdgeCommons JSON envelope: `{header, identity, tags, body}`. The library stamps
the top-level **`identity`** (`{hier, path, component, instance}`) on every message built from config.
Request/reply carries `header.reply_to` + `header.correlation_id`; the reply publishes to `reply_to`
with the same `correlation_id`.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `data` | `SouthboundSignalUpdate` | adapter → bus | `ecv1/{device}/<<BINNAME>>/{instance}/data/{signal}` | — |
| `evt` | `evt` | adapter → bus | `ecv1/{device}/<<BINNAME>>/{instance}/evt/{severity}/{type}` | — |
| `cmd` | `sb/status` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/status` | `{ok,result}` |
| `cmd` | `sb/read` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/read` | `{ok,result}` |
| `cmd` | `sb/write` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/write` | `{ok,result}` |
| `cmd` | `sb/signals` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/signals` | `{ok,result}` |
| `cmd` | `sb/browse` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/browse` | `{ok,result}` |
| `cmd` | `sb/pause` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/pause` | `{ok,result}` |
| `cmd` | `sb/resume` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/sb/resume` | `{ok,result}` |
| `cmd` | `reconnect` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/reconnect` | `{ok,result}` |
| `cmd` | `repoll` | bus → adapter | `ecv1/{device}/<<BINNAME>>/cmd/repoll` | `{ok,result}` |
| `metric` | `southbound_health`, `<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command` | adapter → bus (auto) | `ecv1/{device}/<<BINNAME>>/metric/{metricName}` | — |
| `state` | keepalive | adapter → bus (auto) | `ecv1/{device}/<<BINNAME>>/state` | — |

Fleet consumers subscribe the six UNS wildcards: telemetry `ecv1/+/+/+/data/#`; events
`ecv1/+/+/+/evt/#`; metrics `ecv1/+/+/+/metric/#`; state `ecv1/+/+/+/state`. `state`/`metric`/`cfg`/
`log` are library-owned **reserved** classes — the adapter only ever mints `data`/`evt` topics via
the `data()`/`events()` facades and `cmd` replies via the command inbox, never a hand-assembled
string.

## The command inbox

Served through the library's **command inbox** — a single component-scope subscription
`ecv1/{device}/<<BINNAME>>/cmd/#`. A request's **verb** is the topic channel after `cmd/` and must
equal `header.name`. Built-in verbs (`ping`, `reload-config`, `get-configuration`) ship with every
component; this scaffold adds the `sb/*` + `reconnect`/`repoll` verbs above.

Because the inbox is component-scope, a multi-device adapter selects the target with an **`instance`**
field in the request body (optional when only one device is configured — see
[explanation.md](../explanation.md#instance-routing)). The reply body is
`{"ok": true, "result": <verb result>}` on success, or
`{"ok": false, "error": {"code", "message"}}` on failure.

### Standardized error codes

| Code | Meaning |
|---|---|
| `BAD_ARGS` | Malformed request, or `instance` required (≥2 devices) but missing. |
| `NO_SUCH_INSTANCE` | `instance` named a device that is not configured. |
| `WRITE_NOT_ALLOWED` | Every entry of an `sb/write` was refused by the allow-list. |
| `WRITE_FAILED` | Every allowed write reached the device and every one failed. |
| `DEVICE_UNAVAILABLE` | The device task/session is not available (down, or shutting down). |
| `READ_FAILED` | An on-demand `sb/read` failed at the link. |
| `RECONNECT_FAILED` | A `reconnect` attempt failed. |
| `BROWSE_UNSUPPORTED` | The protocol has no discovery service (the default `DeviceSession.browse`). |
| `BROWSE_FAILED` | A mid-browse failure (a link error, a malformed reply). |

## Sample object

`sb/read` reply `reads[]` entries carry:

| Field | Type | Notes |
|-------|------|-------|
| `value` | number \| boolean \| string \| null | `null` for a signal that could not be resolved or read. |
| `quality` | string | Normalized `GOOD` \| `BAD` \| `UNCERTAIN`. |
| `qualityRaw` | string | The backend's native detail, or `UNRESOLVED_REF`/`NO_DATA` for a signal-ref problem. |

## Data plane

### `SouthboundSignalUpdate` (adapter → bus, `data` class)

Published through the library's `data()` facade (`gg.instance(id).data()`), which constructs the
body, sanitizes the channel, mints the topic, and stamps identity:

```jsonc
"body": {
  "device": { "adapter": "sim", "instance": "device-1", "endpoint": "sim://device-1" },
  "signal": { "id": "temperature-1", "name": "Ambient temperature" },
  "samples": [ { "value": 21.7, "quality": "GOOD", "qualityRaw": "OK", "serverTs": "2026-07-19T00:00:00Z" } ]
}
```

A failed read (no value at all, e.g. `pressure-1` in the simulator) rides the pre-built-body path
instead of `add_sample`, since the facade's `samples[]` cannot express "no value":

```jsonc
"body": {
  "device": { "adapter": "sim", "instance": "device-1", "endpoint": "sim://device-1" },
  "signal": { "id": "pressure-1", "name": "Line pressure" },
  "samples": [ { "value": null, "quality": "BAD", "qualityRaw": "SENSOR_FAULT", "serverTs": "2026-07-19T00:00:00Z" } ]
}
```

### `sb/write` (command)

```jsonc
// request body
"body": { "writes": [ { "signalId": "temperature-1", "value": 25.0 } ] }
// success: { "id": "device-1", "written": 1, "results": [ { "signal": "temperature-1", "value": 25.0, "ok": true } ] }
// refused (not in writes.allow): {"ok": false, "error": {"code": "WRITE_NOT_ALLOWED", ...}}
```

A single `{signalId|id|name, value}` object (no `writes` array) is also accepted. A signal-ref is
`{"signalId": "..."}` / `{"id": "..."}` (the stable id directly) or `{"name": "..."}` (looked up
against the configured inventory). Entries that don't resolve, are missing `value`, or fail the
allow-list are reported per-entry as `{"ok": false, "error": ...}` without touching any other entry.

### `sb/read` (command, request/reply)

```jsonc
// request body
"body": { "signals": [ { "name": "temperature-1" } ] }
// reply body: { "ok": true, "result": { "id": "device-1", "reads": [
//   { "signal": {"id": "temperature-1"}, "value": 21.7, "quality": "GOOD", "qualityRaw": "OK" } ] } }
```

## Control plane

- **`sb/status`** → `{ id, adapter, connected, state, paused, endpoint, metrics }`. `state` is this
  adapter's own vocabulary (`CONNECTING`/`ONLINE`/`BACKOFF`/`PAUSED`); `connected` is the normalized
  flag.
- **`sb/signals`** → `{ id, signals: [ { id, name, writable } ] }` — the configured inventory, no
  device round-trip.
- **`sb/browse`** → `{ id, entries: [ { id, name, type } ], cursor? }` — paged discovery; the
  simulator returns one page. `BROWSE_UNSUPPORTED` when the protocol has none.
- **`sb/pause`** / **`sb/resume`** → `{ id, paused, changed }`. Idempotent: pausing an
  already-paused device reports `changed: false`.
- **`reconnect`** → `{ id, connected: true }` or a `RECONNECT_FAILED` error.
- **`repoll`** → `{ id, polled: <count> }`, or `BAD_ARGS` if the device is currently paused (resume
  first).

## Events (`evt` class)

Published through the library's `events()` facade: severity **derives** the channel
`evt/{severity}/{type}`, so the topic and the body can never disagree.

- **`device-connected`** (info) / a connection-loss alarm on the built-in connectivity provider —
  raised on drop, cleared on restore.
- **`adapter-paused`** (warning) / **`adapter-resumed`** (info) — emitted only when `sb/pause`/
  `sb/resume` actually change the paused state (idempotent calls emit nothing).

## Metrics (`metric` class, reserved — automatic)

See [Reference — Metrics](metrics.md) for every metric's dimensions, measures, and purpose.

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every ~5 s. The RUNNING keepalive carries an
`instances[]` array — one entry per configured device (`{instance, connected, detail}`), driven by
the same `Health`/`connectivity_of` the metrics and `sb/status` read.

## Panels

Three edge-console panel descriptors are registered via `commands.register_panel`, `scope:
"instance"`, order 10/20/30: **`overview`** (connected/state/paused/endpoint summary; actions
`reconnect`/`sb/pause`/`sb/resume`), **`signals`** (a signal grid; verbs `sb/signals`/`sb/read`/
`sb/write`/`repoll`), **`diagnostics`** (a tree browser + key-value list; verbs `sb/browse`/
`sb/status`).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
