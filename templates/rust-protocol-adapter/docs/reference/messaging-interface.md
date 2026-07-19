# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic and message this adapter publishes or accepts, and its CLI flags. Addressing follows the
**Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
data/control plane model, see [explanation.md](../explanation.md); for client recipes, the
[how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (`-t`, or the last `hierarchy` level).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — a configured device id (`device-1`, …) for `data`/`evt`; the command inbox and the
  `state` keepalive are component-scoped (no instance token in their topic).

## Envelope

All messages use the EdgeCommons JSON envelope: `{header, identity, tags, body}`. The library stamps
the top-level `identity` (`{hier, path, component, instance}`) on every message built from a facade.
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

Fleet consumers subscribe the six UNS wildcards — telemetry `ecv1/+/+/+/data/#`, events
`ecv1/+/+/+/evt/#`, metrics `ecv1/+/+/+/metric/#`, state `ecv1/+/+/+/state`. `state`/`metric`/`cfg`
are library-owned **reserved** classes — this adapter only ever mints `data`/`evt` topics via the
`data()`/`events()` facades and `cmd` replies via the command inbox, never a hand-assembled string.

## The command inbox

Served through the library's **command inbox** — one component-scope subscription,
`ecv1/{device}/<<BINNAME>>/cmd/#`. A request's **verb** is the topic channel after `cmd/`, matching
`header.name`. Built-in verbs (`ping`, `reload-config`, `get-configuration`) ship automatically; this
scaffold registers the `sb/*` + `reconnect`/`repoll` verbs (`src/commands.rs`).

Because the inbox is component-scoped, a multi-device adapter selects the target with an optional
`instance` field in the request body (optional only when exactly one device is configured). The
reply body is `{"ok": true, "result": <verb result>}` on success, or
`{"ok": false, "error": {"code", "message"}}` on failure — codes: `BAD_ARGS`, `NO_SUCH_INSTANCE`,
`WRITE_NOT_ALLOWED`, `WRITE_FAILED`, `DEVICE_UNAVAILABLE`, `READ_FAILED`, `RECONNECT_FAILED`,
`BROWSE_UNSUPPORTED`, `BROWSE_FAILED`.

## Data plane

### `SouthboundSignalUpdate` (adapter → bus, `data` class)

Published through the library's `data()` facade — the adapter never hand-builds the body or the
topic:

```jsonc
"body": {
  "device": { "adapter": "sim", "instance": "device-1", "endpoint": "sim://device-1" },
  "signal": { "id": "temperature-1", "name": "Ambient temperature" },
  "samples": [ { "value": 21.7, "quality": "GOOD", "qualityRaw": "unspecified", "serverTs": "2026-07-19T00:00:00Z" } ]
}
```

An omitted `quality` defaults to `GOOD` with `qualityRaw: "unspecified"` (a synthesized-vs-reported
marker); a failed read (the simulator's `pressure-1`) publishes an explicit `BAD` with the native
fault text as `qualityRaw` and `value: null`.

### `sb/write` (command)

```jsonc
"body": { "writes": [ { "signalId": "temperature-1", "value": 42.5 } ] }
// result: { "id": "device-1", "written": 1,
//           "results": [ { "signal": "temperature-1", "value": 42.5, "ok": true } ] }
```

A single `{signalId/id/name, value}` object (no `writes` array) is also accepted. A signal-ref is
`{"signalId": "…"}` / `{"id": "…"}` (the stable id directly) or `{"name": "…"}` (resolved against
the configured inventory). Every entry is checked against `writes.allow` **before** it reaches the
device; `WRITE_NOT_ALLOWED` when every entry is refused, `WRITE_FAILED` when every attempted write
reaches the device and every one is rejected there.

### `sb/read` (command, request/reply)

```jsonc
// request: { "signals": [ { "signalId": "temperature-1" } ] }
// reply:   { "id": "device-1", "reads": [
//   { "signal": { "id": "temperature-1" }, "value": 21.7, "quality": "GOOD", "qualityRaw": "unspecified" } ] }
```

An unresolvable ref is reported per-entry with `quality: BAD`/`qualityRaw: "UNRESOLVED_REF"`, not
omitted.

## Control plane

- **`sb/status`** → `{ id, adapter, connected, state, paused, endpoint, metrics }`.
- **`sb/signals`** → `{ id, signals: [ { id, name, writable }, ... ] }` — the configured/backend
  inventory, no device round-trip.
- **`sb/browse`** → `{ id, entries: [ { id, name, type }, ... ], cursor? }`, or `BROWSE_UNSUPPORTED`
  if the backend has no discovery (the simulator's one-page browse is the worked example).
- **`sb/pause`** / **`sb/resume`** → `{ id, paused, changed }` — idempotent; pausing an
  already-paused device reports `changed: false`.
- **`reconnect`** → `{ id, connected: true }` or a `RECONNECT_FAILED` error.
- **`repoll`** → `{ id, polled: <count> }`; refused with `BAD_ARGS` while paused.

## Events (`evt` class)

Published through the library's `events()` facade; severity **derives** the channel
(`evt/{severity}/{type}`), so the topic and the body can never disagree. This scaffold emits
`device-connected` (info), `device-unreachable` (critical, raised on drop / cleared on restore),
`adapter-paused` (warning), and `adapter-resumed` (info).

## State keepalive (`state` class, reserved — automatic)

Publishes every ~5 s on `ecv1/{device}/<<BINNAME>>/state`. The RUNNING keepalive carries an
`instances[]` array — one entry per configured device — from the same connectivity provider
`sb/status` reads:

```jsonc
{ "status": "RUNNING", "uptimeSecs": 3600,
  "instances": [ { "instance": "device-1", "connected": true, "state": "ONLINE",
                    "detail": "sim://device-1", "attributes": { "adapter": "sim", "paused": false } } ] }
```

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/Kubernetes use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | Thing name; the `{device}` token of every UNS topic. |
