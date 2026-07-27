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

| Class | Message | Scope | Direction | Topic | Reply |
|-------|---------|-------|-----------|-------|-------|
| `data` | `SouthboundSignalUpdate` | — | adapter → bus | `ecv1/{device}/<<BINNAME>>/{instance}/data/{signal}` | — |
| `evt` | `evt` | — | adapter → bus | `ecv1/{device}/<<BINNAME>>/{instance}/evt/{severity}/{type}` | — |
| `cmd` | `sb/status` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/status` | `{ok,result}` |
| `cmd` | `sb/read` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/read` | `{ok,result}` |
| `cmd` | `sb/write` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/write` | `{ok,result}` |
| `cmd` | `sb/signals` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/signals` | `{ok,result}` |
| `cmd` | `sb/browse` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/browse` | `{ok,result}` |
| `cmd` | `sb/pause` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/pause` | `{ok,result}` |
| `cmd` | `sb/resume` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/sb/resume` | `{ok,result}` |
| `cmd` | `reconnect` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/reconnect` | `{ok,result}` |
| `cmd` | `repoll` | `instance` | bus → adapter | `ecv1/{device}/<<BINNAME>>/[{instance}/]cmd/repoll` | `{ok,result}` |
| `metric` | `southbound_health`, `<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command` | — | adapter → bus (auto) | `ecv1/{device}/<<BINNAME>>/metric/{metricName}` | — |
| `state` | keepalive | — | adapter → bus (auto) | `ecv1/{device}/<<BINNAME>>/state` | — |

**Scope** is the verb's declared addressing, advertised on its `describe` entry. All nine verbs act
on one device, so all nine are `instance`: a request may be addressed to a device on the topic
(`…/{instance}/cmd/{verb}`) or to the component (`…/cmd/{verb}`) naming the device in the body — see
[Addressing a verb](#addressing-a-verb).

Fleet consumers subscribe the six UNS wildcards: telemetry `ecv1/+/+/+/data/#`; events
`ecv1/+/+/+/evt/#`; metrics `ecv1/+/+/+/metric/#`; state `ecv1/+/+/+/state`. `state`/`metric`/`cfg`/
`log` are library-owned **reserved** classes — the adapter only ever mints `data`/`evt` topics via
the `data()`/`events()` facades and `cmd` replies via the command inbox, never a hand-assembled
string.

## The command inbox

Served through the library's **command inbox**, which subscribes both cmd wildcards:
`ecv1/{device}/<<BINNAME>>/cmd/#` (component-addressed) and
`ecv1/{device}/<<BINNAME>>/+/cmd/#` (instance-addressed). A request's **verb** is the topic channel
after `cmd/` and must equal `header.name`. Built-in verbs (`ping`, `status`, `describe`,
`reload-config`, `get-configuration`) ship with every component; this scaffold adds the `sb/*` +
`reconnect`/`repoll` verbs above. The reply body is `{"ok": true, "result": <verb result>}` on
success, or `{"ok": false, "error": {"code", "message"}}` on failure.

### Addressing a verb

Every verb here declares scope `instance`, and the **library** resolves the addressing before the
adapter's handler runs:

1. The topic's instance token is authoritative. `…/device-2/cmd/sb/read` acts on `device-2`.
2. A body `instance` that disagrees with the topic token is `BAD_ARGS` — checked first, before
   anything else about the request.
3. A component-addressed request may name the device in the body instead:
   `…/cmd/sb/read` with `{"instance": "device-2", …}` is equivalent to (1).
4. When neither names one, the adapter resolves it against its own configuration: with exactly one
   device configured that device answers; with two or more it is `BAD_ARGS`. An instance that is not
   configured is `NO_SUCH_INSTANCE`.

Steps 1–3 belong to the library and are identical for every EdgeCommons component; only step 4
needs this component's configuration.

### Standardized error codes

| Code | Meaning |
|---|---|
| `BAD_ARGS` | Malformed request; a body `instance` conflicting with the topic's token; or `instance` required (≥2 devices) but missing. |
| `NO_SUCH_INSTANCE` | The addressed instance is not a configured device. |
| `WRITE_NOT_ALLOWED` | Every entry of an `sb/write` was refused by the allow-list. |
| `WRITE_FAILED` | Every allowed write reached the device and every one failed. |
| `DEVICE_UNAVAILABLE` | The device task/session is not available (down, or shutting down). |
| `READ_FAILED` | An on-demand `sb/read` failed at the link. |
| `RECONNECT_FAILED` | A `reconnect` attempt failed. |
| `BROWSE_UNSUPPORTED` | The protocol has no discovery service (the default `DeviceSession.browse`). |
| `BROWSE_FAILED` | A mid-browse failure (a link error, a malformed reply). |
| `PAUSED` | A `repoll` was requested while the instance is paused — resume first. |

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

The sample's `serverTs` is the **capture** moment: the seam's `capture_ts` when the backend
supplies one, else the worker's read-completion receive stamp (a direct client's receive moment IS
the capture moment). A device-authored `source_ts` rides as `sourceTs` only when present — never
synthesized — and when a mediating server makes the adapter's receive moment differ from the
effective `serverTs`, it rides as a per-sample `receivedTs` extra:

```jsonc
"samples": [ { "value": 21.7, "quality": "GOOD", "qualityRaw": "OK",
               "sourceTs": "2026-07-19T00:00:00.1Z", "serverTs": "2026-07-19T00:00:00.4Z",
               "receivedTs": "2026-07-19T00:00:00.9Z" } ]
```

A failed read (no value at all, e.g. `pressure-1` in the simulator) rides the pre-built-body path
instead of `add_sample`, carrying the same quality and timestamp fields:

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
- **`sb/browse`** → paged discovery by default: `{ id, entries: [ { id, name, type } ], cursor? }`;
  the simulator returns one page. `BROWSE_UNSUPPORTED` when the protocol has none. A request
  carrying `ref` selects the hierarchical panel mode instead (below); mixing `ref`/`depth`/`maxRefs`
  with `cursor`/`max` is `BAD_ARGS`, as is `depth`/`maxRefs` without `ref`.
- **`sb/pause`** / **`sb/resume`** → `{ id, paused, changed }`. Idempotent: pausing an
  already-paused device reports `changed: false`.
- **`reconnect`** → `{ id, connected: true }` or a `RECONNECT_FAILED` error.
- **`repoll`** → `{ id, polled: <count> }`, or `PAUSED` if the instance is currently paused (resume
  first).

### Hierarchical `sb/browse` (the panel mode)

The `treeBrowser` panel drives `sb/browse` with `{ instance?, ref, depth?, maxRefs? }` instead of a
cursor. `ref` selects the node: `"root"` is the device itself, whose `contains` refs are the same
inventory the paged mode serves; a signal id selects that node as a known leaf (`"refs": []`). An
unknown `ref` is `BAD_ARGS`, and so is `depth`/`maxRefs` without `ref`. `depth` is bounded 1–4
(default 1) and `maxRefs` 1–1000 (default 200); the adapter's inventory is flat, so a deeper `depth`
finds no grandchildren.

```jsonc
// request body
"body": { "ref": "root", "depth": 1, "maxRefs": 200 }
// reply body: { "ok": true, "result": {
//   "id": "device-1", "mode": "hierarchical",
//   "root": { "nodeId": "root", "name": "device-1", "nodeClass": "device", "dataType": null,
//             "refs": [ { "referenceType": "contains",
//                         "target": { "nodeId": "temperature-1", "name": "Ambient temperature",
//                                     "nodeClass": "signal", "dataType": "REAL" } } ] },
//   "refCount": 1, "depth": 1, "truncated": false } }
```

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
`instances[]` array — one entry per configured device
(`{instance, connected, state, detail, attributes}`), driven by the same `Health`/`connectivity_of`
the metrics and `sb/status` read. `state` is this adapter's own vocabulary
(`CONNECTING`/`ONLINE`/`BACKOFF`/`PAUSED`), so a deliberately paused device is distinguishable from
one that has gone quiet; `connected` stays the normalized flag any consumer can read.

## Panels

Three edge-console panel descriptors are registered via `commands.register_panel`, `scope:
"instance"` (repeated on every command-backed widget), order 10/20/30:

- **`overview`** — an *Adapter overview* summary (Signals / Lifecycle / Writes rows) plus a
  *Lifecycle bindings* command summary (`sb/status`, `reconnect`, `sb/pause`, `sb/resume`,
  `repoll`).
- **`signals`** — a `signalGrid` bound to `sb/signals` through both `signalsVerb` and the
  renderer-compat `subscriptionsVerb` alias (a descriptor field alias — no `sb/subscriptions` wire
  verb exists), with `readVerb: sb/read`.
- **`diagnostics`** — a hierarchical `treeBrowser` (`browseVerb: sb/browse`, `rootRef: "root"`,
  `depth: 1`, `maxRefs: 200`, `readVerb: sb/read`) plus a *Diagnostic commands* summary
  (`sb/status`, `sb/browse`).

No widget names a `writeVerb` — writes stay on the command surface behind the allow-list.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
