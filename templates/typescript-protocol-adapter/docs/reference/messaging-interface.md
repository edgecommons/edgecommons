This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Messaging Interface & CLI

Every topic and message this scaffold publishes or accepts, and the CLI flags. Addressing follows
the **Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
model behind the data/control split, see [../explanation.md](../explanation.md); for client
recipes, the [how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (the last `hierarchy` level).
- `{component}` — the component UNS token, `<<BINNAME>>`, set by `component.token`. It is a
  separate identifier from the Greengrass component name (`<<COMPONENTFULLNAME>>`), which never
  appears on the wire.
- `{instance}` — the configured device id (`device-1`, …) for `data`/`evt`; the command inbox and
  `state`/`metric` are component-scoped.

## Envelope

Every message uses the EdgeCommons JSON envelope: `{header, identity, tags, body}`. The library
stamps the top-level **`identity`** (`{hier, path, component, instance}`) on every message built
from config. Request/reply carries `header.reply_to` + `header.correlation_id`; the reply publishes
to `reply_to` with the same `correlation_id`.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `data` | `SouthboundSignalUpdate` | adapter → bus | `ecv1/{device}/{component}/{instance}/data/{signal}` | — |
| `evt` | `evt` | adapter → bus | `ecv1/{device}/{component}/{instance}/evt/{severity}/{type}` | — |
| `cmd` | `sb/status` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/sb/status` | `{ok,result}` |
| `cmd` | `sb/read` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/sb/read` | `{ok,result}` |
| `cmd` | `sb/write` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/sb/write` | `{ok,result}` |
| `cmd` | `sb/signals` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/sb/signals` | `{ok,result}` |
| `cmd` | `sb/browse` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/sb/browse` | `{ok,result}` |
| `cmd` | `sb/pause` / `sb/resume` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/sb/{pause,resume}` | `{ok,result}` |
| `cmd` | `reconnect` / `repoll` | bus → adapter | `ecv1/{device}/{component}/{instance}/cmd/{reconnect,repoll}` | `{ok,result}` |
| `metric` | `southbound_health`, `<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command` | adapter → bus (auto) | `ecv1/{device}/{component}/metric/{metricName}` | — |
| `state` | keepalive | adapter → bus (auto) | `ecv1/{device}/{component}/state` | — |

Fleet consumers subscribe the six UNS wildcards — telemetry `ecv1/+/+/+/data/#`, events
`ecv1/+/+/+/evt/#`, metrics `ecv1/+/+/+/metric/#`, state `ecv1/+/+/+/state`.
`state`/`metric`/`cfg`/`log` are library-owned **reserved** classes — a direct publish to them is
rejected; this component only ever mints `data`/`evt` topics via the `data()`/`events()` facades
and `cmd` replies via the command inbox.

## The command inbox

Every `sb/*` verb plus `reconnect`/`repoll` is registered on the shared command inbox
(`src/commands.ts`'s `registerAll`) alongside the library's built-ins (`ping`, `status`, `describe`,
`reload-config`, `get-configuration`). A request's **verb** is `header.name`.
The reply body is `{"ok": true, "result": <verb result>}` on success or
`{"ok": false, "error": {"code", "message"}}` on failure. Error codes:
`BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`, `WRITE_FAILED`, `DEVICE_UNAVAILABLE`,
`READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`, `BROWSE_FAILED`, `PAUSED`.

### Scope and addressing

| Verb | Scope |
|------|-------|
| `sb/status`, `sb/read`, `sb/write`, `sb/signals`, `sb/browse` | `instance` |
| `sb/pause`, `sb/resume`, `reconnect`, `repoll` | `instance` |
| `ping`, `status`, `describe`, `reload-config`, `get-configuration` (library) | `both` |

An `instance`-scoped verb acts on exactly one configured device. Address it either way — the library
resolves both, and refuses a request that addresses two devices at once:

- **On the topic:** `ecv1/{device}/{component}/{instance}/cmd/{verb}` names the device directly.
- **In the body:** an `"instance"` field on a command sent to the component topic
  `ecv1/{device}/{component}/cmd/{verb}`.
- **Both:** the topic wins, and a body naming a *different* device is `BAD_ARGS`
  ("instance in body conflicts with the addressed instance") — checked before the device is looked
  up, so the verb never runs.
- **Neither:** with exactly one device configured the command applies to it; with two or more it is
  `BAD_ARGS`. An instance that names no configured device is `NO_SUCH_INSTANCE`.

A `both`-scoped verb answers identically on either topic. Each verb's scope is published in the
`describe` reply (`commands[].scope`), which is how the edge-console decides whether to offer a
device selector.

## Data plane

### `SouthboundSignalUpdate` (adapter → bus, `data` class)

Published through the library's `data()` facade (`gg.instance(id).data()`), which constructs the
body, sanitizes the channel, mints the topic, and stamps identity — `src/app.ts` only ever calls
`.signal(id).name(n).device(...).addSample(v, {quality, qualityRaw, sourceTs, serverTs, extra}).publish()`.

```jsonc
"body": {
  "device": { "adapter": "sim", "instance": "device-1", "endpoint": "sim://device-1" },
  "signal": { "id": "temperature-1", "name": "Ambient temperature" },
  "samples": [ { "value": 21.4, "quality": "GOOD", "qualityRaw": "OK",
                 "serverTs": "2026-07-19T00:00:00Z",          // capture; here = adapter receipt
                 "sourceTs": "2026-07-18T23:59:58Z",          // machine time, only when supplied
                 "receivedTs": "2026-07-19T00:00:02Z" } ]     // extra, only when != serverTs
}
```

Per-sample timestamps follow the four-slot model (`docs/SOUTHBOUND.md` §2), identically on the
GOOD and BAD/null paths: the reading's `captureTs` becomes `serverTs` (falling back to the
worker's auto-stamped `receivedTs` when the protocol has no mediating server — a direct client's
receive moment IS the capture moment); `sourceTs` appears only when the protocol supplied it,
never synthesized; and `receivedTs` rides as an additive extra only when it differs from the
effective `serverTs`. The simulator is a direct client, so its samples carry only `serverTs`.

Published every poll — the scaffold has no deadband/onChange filter (add one in `src/app.ts` if
your protocol needs it, following `modbus-adapter`'s pattern).

## Control plane

### `sb/status`

```jsonc
// result: { "id", "adapter", "connected", "state", "paused", "endpoint", "metrics": {...} }
```

### `sb/signals`

```jsonc
// result: { "id", "signals": [ { "id", "name", "writable" }, ... ] }
```

The configured inventory, from `DeviceBackend.inventory()` — no device round-trip.

### `sb/read`

```jsonc
// request body:  { "signals": [ { "name": "temperature-1" }, { "signalId": "pressure-1" } ] }
// result: { "id", "reads": [ { "signal": {"id"}, "value", "quality", "qualityRaw" }, ... ] }
```

A signal-ref is `signalId`, `id`, or `name` (looked up against the inventory). An unresolvable ref
returns `quality: BAD, qualityRaw: "UNRESOLVED_REF"`.

### `sb/write` (§2.2 batch shape)

```jsonc
// request body:  { "writes": [ { "signalId": "temperature-1", "value": 42.5 } ] }
// (a single { signalId, value } object with no `writes` array is also accepted)
// result: { "id", "written": 1, "results": [ { "signal": "temperature-1", "value": 42.5, "ok": true } ] }
```

Allow-list checked **before** any device I/O. `WRITE_NOT_ALLOWED` when every entry is refused by
the allow-list; `WRITE_FAILED` when every allowed write reached the device and every one failed.

### `sb/browse` (paged and hierarchical discovery)

Two request forms serve the same browsed inventory.

**Paged** (`cursor`/`max`):

```jsonc
// request body: { "cursor"?: "<opaque>", "max"?: 200 }
// result: { "id", "entries": [ { "id", "name", "type" }, ... ], "cursor"?: "<opaque>" }
```

**Hierarchical** (`ref`/`depth`/`maxRefs`) — the form the edge-console `treeBrowser` widget sends.
Presence of `ref` selects it; mixing `ref`/`depth`/`maxRefs` with `cursor`/`max` is `BAD_ARGS`.
`depth` is bounded 1–4 (default 1) and `maxRefs` 1–1000 (default 200).

```jsonc
// request body: { "ref": "root", "depth"?: 1, "maxRefs"?: 200 }
// result: { "id", "mode": "hierarchical",
//           "root": { "nodeId": "root", "name": "<instance>", "nodeClass": "device",
//                     "dataType": null,
//                     "refs": [ { "referenceType": "contains",
//                                 "target": { "nodeId", "name", "nodeClass": "signal",
//                                             "dataType" } }, ... ] },
//           "refCount": 1, "depth": 1, "truncated": false }
```

`ref: "root"` answers the device node whose `contains` refs are the browsed signals (bounded by
`maxRefs`; `truncated` reports whether more exist). A signal id as `ref` answers that node with
`"refs": []` (a known leaf); an unknown ref is `BAD_ARGS`. The scaffold's tree is flat, so a depth
beyond 1 finds no grandchildren.

`BROWSE_UNSUPPORTED` when the backend has no discovery service (the default seam behavior).

### `sb/pause` / `sb/resume`

```jsonc
// result: { "id", "paused": true|false, "changed": boolean }
```

Idempotent — pausing an already-paused instance replies `changed: false`.

### `reconnect` / `repoll`

```jsonc
// reconnect result: { "id", "connected": true }             (or a RECONNECT_FAILED error)
// repoll    result: { "id", "polled": <signals published> } (PAUSED if paused)
```

## Events (`evt` class)

Published through the `events()` facade: severity **derives** the channel `evt/{severity}/{type}`,
so the topic and body can never disagree.

- **`evt/info/device-connected`** — a device (re)connected.
- **`evt/critical/device-unreachable`** — a stateful alarm: raised when the link drops, cleared on
  reconnect. Context carries `{instance}`.
- **`evt/warning/adapter-paused`** / **`evt/info/adapter-resumed`** — a `sb/pause`/`sb/resume`
  transition (only emitted when the state actually changed).

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every `heartbeat.intervalSecs` (default
5s). The RUNNING keepalive's `instances[]` array carries one entry per configured device —
`{instance, connected, state, detail, attributes: {adapter, paused}}` — the same sample `sb/status`
answers on demand (one provider, two surfaces; see [../explanation.md](../explanation.md)).

`state` is this adapter's own vocabulary — `CONNECTING`, `ONLINE`, `BACKOFF`, or `PAUSED` — read
from the same per-device state that answers `sb/status`, so a fleet view and a status reply cannot
disagree. A deliberately paused device reads `PAUSED` while `connected` stays truthful; a link that
breaks while paused reads `BACKOFF`.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
