This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Messaging Interface & CLI

Every topic and message this scaffold publishes or accepts, and the CLI flags. Addressing follows
the **Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
model behind the data/control split, see [../explanation.md](../explanation.md); for client
recipes, the [how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (the last `hierarchy` level).
- `{component}` — this component's UNS token.
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
| `cmd` | `sb/status` | bus → adapter | `ecv1/{device}/{component}/cmd/sb/status` | `{ok,result}` |
| `cmd` | `sb/read` | bus → adapter | `ecv1/{device}/{component}/cmd/sb/read` | `{ok,result}` |
| `cmd` | `sb/write` | bus → adapter | `ecv1/{device}/{component}/cmd/sb/write` | `{ok,result}` |
| `cmd` | `sb/signals` | bus → adapter | `ecv1/{device}/{component}/cmd/sb/signals` | `{ok,result}` |
| `cmd` | `sb/browse` | bus → adapter | `ecv1/{device}/{component}/cmd/sb/browse` | `{ok,result}` |
| `cmd` | `sb/pause` / `sb/resume` | bus → adapter | `ecv1/{device}/{component}/cmd/sb/{pause,resume}` | `{ok,result}` |
| `cmd` | `reconnect` / `repoll` | bus → adapter | `ecv1/{device}/{component}/cmd/{reconnect,repoll}` | `{ok,result}` |
| `metric` | `southbound_health`, `<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command` | adapter → bus (auto) | `ecv1/{device}/{component}/metric/{metricName}` | — |
| `state` | keepalive | adapter → bus (auto) | `ecv1/{device}/{component}/state` | — |

Fleet consumers subscribe the six UNS wildcards — telemetry `ecv1/+/+/+/data/#`, events
`ecv1/+/+/+/evt/#`, metrics `ecv1/+/+/+/metric/#`, state `ecv1/+/+/+/state`.
`state`/`metric`/`cfg`/`log` are library-owned **reserved** classes — a direct publish to them is
rejected; this component only ever mints `data`/`evt` topics via the `data()`/`events()` facades
and `cmd` replies via the command inbox.

## The command inbox

Every `sb/*` verb plus `reconnect`/`repoll` is registered on the shared command inbox
(`src/commands.ts`'s `registerAll`) alongside the library's built-ins (`ping`, `reload-config`,
`get-configuration`). A request's **verb** is `header.name`; with two or more configured devices, a
body `instance` field selects which one (`BAD_ARGS` if missing, `NO_SUCH_INSTANCE` if unknown).
The reply body is `{"ok": true, "result": <verb result>}` on success or
`{"ok": false, "error": {"code", "message"}}` on failure. Error codes:
`BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`, `WRITE_FAILED`, `DEVICE_UNAVAILABLE`,
`READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`, `BROWSE_FAILED`.

## Data plane

### `SouthboundSignalUpdate` (adapter → bus, `data` class)

Published through the library's `data()` facade (`gg.instance(id).data()`), which constructs the
body, sanitizes the channel, mints the topic, and stamps identity — `src/app.ts` only ever calls
`.signal(id).name(n).device(...).addSample(v, {quality, qualityRaw}).publish()`.

```jsonc
"body": {
  "device": { "adapter": "sim", "instance": "device-1", "endpoint": "sim://device-1" },
  "signal": { "id": "temperature-1", "name": "Ambient temperature" },
  "samples": [ { "value": 21.4, "quality": "GOOD", "qualityRaw": "OK", "serverTs": "2026-07-19T00:00:00Z" } ]
}
```

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

### `sb/browse` (paged discovery)

```jsonc
// request body: { "cursor"?: "<opaque>", "max"?: 200 }
// result: { "id", "entries": [ { "id", "name", "type" }, ... ], "cursor"?: "<opaque>" }
```

`BROWSE_UNSUPPORTED` when the backend has no discovery service (the default seam behavior).

### `sb/pause` / `sb/resume`

```jsonc
// result: { "id", "paused": true|false, "changed": boolean }
```

Idempotent — pausing an already-paused instance replies `changed: false`.

### `reconnect` / `repoll`

```jsonc
// reconnect result: { "id", "connected": true }             (or a RECONNECT_FAILED error)
// repoll    result: { "id", "polled": <signals published> } (BAD_ARGS if paused)
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
`{instance, connected, state, attributes: {adapter, paused}}` — the same sample `sb/status`
answers on demand (one provider, two surfaces; see [../explanation.md](../explanation.md)).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
