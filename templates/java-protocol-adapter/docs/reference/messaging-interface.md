# Reference — Messaging Interface & CLI

> This documents the generated scaffold; rewrite it as you build the component out.

Complete specification of every UNS topic and message this scaffold publishes or accepts, and its
CLI flags. For the data-plane/control-plane model, see [../explanation.md](../explanation.md).

> **Unified Namespace.** All topics follow `ecv1/{device}/{component}/{instance}/{class}[/channel]`,
> minted by the library's topic builder — never hand-assembled. The enterprise hierarchy rides the
> top-level envelope `identity` element, not the topic.

## Envelope

All messages use the EdgeCommons JSON envelope, `{header, identity, tags, body}`:

```jsonc
{
  "header": {
    "name": "SouthboundSignalUpdate",
    "version": "1.0",
    "timestamp": "2026-07-19T12:00:00Z",
    "uuid": "…",
    "correlation_id": "…",   // present on replies (echoes the request)
    "reply_to": "…"          // present on requests
  },
  "identity": { "hier": [ … ], "path": "site1/tutorial-thing", "component": "<<BINNAME>>", "instance": "device-1" },
  "tags": { … },
  "body": { … }
}
```

## Topics (UNS classes)

| Class | Message | Direction | Topic |
|-------|---------|-----------|-------|
| `data` | `SouthboundSignalUpdate` | adapter → bus | `ecv1/{device}/<<BINNAME>>/{instance}/data/{signalPath}` |
| `cmd` | `sb/*` verbs, `reconnect`, `repoll` | bus ↔ adapter | `ecv1/{device}/<<BINNAME>>/cmd/{verb}` |
| `evt` | connection alarms | adapter → bus | `ecv1/{device}/<<BINNAME>>/{instance}/evt/{severity}/{type}` |
| `metric` | `southbound_health`, `<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command` | adapter → bus | `ecv1/{device}/<<BINNAME>>/metric/{metricName}` |
| `state` | keepalive | adapter → bus | `ecv1/{device}/<<BINNAME>>/state` |

`state`/`metric`/`cfg`/`log` are reserved (library-owned); the adapter publishes to them only through
the metrics/heartbeat subsystems, never by a direct publish.

## The command surface

A request is a `cmd` envelope whose `header.name` equals the verb; a reply carries a uniform body:

```jsonc
{ "ok": true,  "result": { … } }
{ "ok": false, "error": { "code": "…", "message": "…" } }
```

**Instance selector.** Every `sb/*` request body may carry an `"instance"` field. With exactly one
device configured it is optional; with two or more, a missing id is `BAD_ARGS` and an unknown one is
`NO_SUCH_INSTANCE`.

| Verb | Purpose |
|------|---------|
| `sb/status` | Per-device link state / paused / endpoint + connection counters. |
| `sb/read` | On-demand read of named signals. |
| `sb/write` | Allow-listed batch write, per-entry confirmed. |
| `sb/signals` | The configured signal inventory (no device round-trip). |
| `sb/browse` | Paged device discovery; `BROWSE_UNSUPPORTED` by default. |
| `sb/pause` / `sb/resume` | Idempotent pause/resume of telemetry production. |
| `reconnect` | Drop the session and re-establish it (one confirmed attempt). |
| `repoll` | Trigger an immediate poll cycle. |

The library also serves `ping`, `reload-config`, and `get-configuration` on the same inbox.

Error codes: `BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`, `WRITE_FAILED`,
`DEVICE_UNAVAILABLE`, `READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`, `BROWSE_FAILED`.

### `SouthboundSignalUpdate` (data plane)

```jsonc
"body": {
  "device": { "adapter": "sim", "instance": "device-1", "endpoint": "sim://device-1" },
  "signal": { "id": "temperature-1", "name": "Ambient temperature" },
  "samples": [ { "value": 21.3, "quality": "GOOD", "qualityRaw": "OK",
                 "sourceTs": null, "serverTs": "2026-07-19T12:00:00Z" } ]
}
```

### `sb/read`

```jsonc
"body": { "instance": "device-1", "signals": [ { "signalId": "temperature-1" }, { "name": "Line pressure" } ] }
```
```jsonc
{ "ok": true, "result": { "id": "device-1", "reads": [
    { "signal": { "id": "temperature-1" }, "value": 21.3, "quality": "GOOD", "qualityRaw": "OK" } ] } }
```
A ref is resolved by `signalId`/`id` directly, or by `name` looked up against the configured
inventory. An unresolved ref reads back with `quality: "BAD"`, `qualityRaw: "UNRESOLVED_REF"`.

### `sb/write` (batch, allow-listed, confirmed)

```jsonc
"body": { "instance": "device-1", "writes": [ { "signalId": "temperature-1", "value": 22.0 } ] }
```
A single object without the `writes` array is also accepted.
```jsonc
{ "ok": true, "result": { "id": "device-1", "written": 1, "results": [
    { "signal": "temperature-1", "value": 22.0, "ok": true } ] } }
```
A refused entry reports `"ok": false, "error": "not in writes.allow"` and is never sent to the
device. `WRITE_NOT_ALLOWED` is thrown only when **every** entry was refused by the allow-list;
`WRITE_FAILED` only when every attempted write reached the device and failed there.

### `sb/signals`

```jsonc
{ "ok": true, "result": { "id": "device-1", "signals": [
    { "id": "temperature-1", "name": "Ambient temperature", "writable": false } ] } }
```

### `sb/browse`

```jsonc
"body": { "instance": "device-1", "cursor": null, "max": 200 }
```
```jsonc
{ "ok": true, "result": { "id": "device-1", "entries": [
    { "id": "temperature-1", "name": "Ambient temperature", "type": "REAL" } ], "cursor": null } }
```
`cursor` is present in the reply only while more pages remain.

### `sb/status`

```jsonc
{ "ok": true, "result": {
    "id": "device-1", "adapter": "sim", "connected": true, "state": "ONLINE", "paused": false,
    "endpoint": "sim://device-1",
    "metrics": { "connectAttempts": {"interval":1,"total":1}, "connectFailures": {"interval":0,"total":0},
                 "reconnectAttempts": {"interval":0,"total":0}, "connectionDrops": {"interval":0,"total":0} } } }
```

### `sb/pause` / `sb/resume`

```jsonc
{ "ok": true, "result": { "id": "device-1", "paused": true, "changed": true } }
```
Idempotent: pausing an already-paused device returns `"changed": false`.

### `reconnect` / `repoll`

```jsonc
{ "ok": true, "result": { "id": "device-1", "connected": true } }        // reconnect
{ "ok": true, "result": { "id": "device-1", "polled": 2 } }              // repoll
```
`repoll` refuses with `BAD_ARGS` ("instance is paused - resume first") while the device is paused.

## Events (`evt` class)

| Channel | When |
|---------|------|
| `evt/info/device-connected` | A connect attempt (initial or reconnect) succeeds. |
| `evt/critical/device-unreachable` | The link drops (raise) or is restored (clear on next connect). |
| `evt/warning/adapter-paused` / `evt/info/adapter-resumed` | `sb/pause` / `sb/resume` actually changed state. |

## State keepalive

Each RUNNING tick, `instances[]` carries one `{instance, connected, detail}` per configured device —
the passive counterpart to `sb/status`.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `IPC` \| `MQTT [path]` | Defaults from the platform; `IPC` only on GREENGRASS. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `SHADOW` \| `CONFIG_COMPONENT` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name = the device (last hierarchy level) in every UNS topic. |
