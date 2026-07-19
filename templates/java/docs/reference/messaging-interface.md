# Reference — Messaging Interface & CLI

> This documents the generated scaffold; rewrite it as you build the component out.

Complete specification of every UNS topic and message this scaffold publishes or accepts, and its
CLI flags.

> **Unified Namespace.** All topics follow `ecv1/{device}/{component}/{instance}/{class}[/channel]`,
> minted by the library's topic builder — never hand-assembled.

## Envelope

All messages use the EdgeCommons JSON envelope, `{header, identity, tags, body}`:

```jsonc
{
  "header": { "name": "StatusUpdate", "version": "1.0", "timestamp": "2026-07-19T12:00:00Z", "uuid": "…" },
  "identity": { "hier": [ … ], "path": "factory-1/tutorial-thing", "component": "<<BINNAME>>", "instance": "main" },
  "tags": { … },
  "body": { … }
}
```

## Topics (UNS classes)

| Class | Message | Direction | Topic |
|-------|---------|-----------|-------|
| `app` | `StatusUpdate` (demo) | component → bus | `ecv1/{device}/<<BINNAME>>/app/status` |
| `data` | `SouthboundSignalUpdate` (`demo-signal`) | component → bus | `ecv1/{device}/<<BINNAME>>/data/demo-signal` |
| `evt` | `sample-event` (demo) | component → bus | `ecv1/{device}/<<BINNAME>>/evt/info/sample-event` |
| `cmd` | `set-greeting` (demo) + built-ins | bus → component | `ecv1/{device}/<<BINNAME>>/cmd/set-greeting` |
| `metric` | `loopTicks` (demo) | component → bus | `ecv1/{device}/<<BINNAME>>/metric/loopTicks` |
| `state` | keepalive | component → bus | `ecv1/{device}/<<BINNAME>>/state` |

`state`/`metric`/`cfg`/`log` are reserved (library-owned) — publish through the metrics/heartbeat
subsystems, never by a direct publish. `app`, `data`, `evt`, and `cmd` are the application classes
this scaffold's demo surface uses.

## The demo surface

### `StatusUpdate` (`app` class)

```jsonc
"body": { "seq": 42, "message": "Hello world" }
```
Published once per `publish_interval`; `message` reflects the current greeting.

### `demo-signal` (`data` class, via `data()`)

```jsonc
"body": {
  "signal": { "id": "demo-signal", "name": "Demo Signal" },
  "samples": [ { "value": 21.9, "quality": "GOOD", "qualityRaw": "unspecified" } ]
}
```
`qualityRaw: "unspecified"` marks a sample published with no explicit quality (the facade's honest
default) — pass an explicit `Quality` when your source knows a read failed or is stale.

### `sample-event` (`evt` class, via `events()`)

```jsonc
"body": { "severity": "info", "type": "sample-event", "message": "sample event from <<COMPONENTNAME>>",
          "context": { "seq": 42, "greeting": "Hello world" } }
```

### `set-greeting` (custom command verb)

```jsonc
// request
{ "header": { "name": "set-greeting", "version": "1.0" }, "body": { "greeting": "Hi there" } }
// reply
{ "ok": true, "result": { "previousGreeting": "Hello world", "greeting": "Hi there" } }
```
A missing/malformed `greeting` returns `{ "ok": false, "error": { "code": "BAD_ARGS", … } }`.

## Built-in command verbs

`ping`, `reload-config`, `get-configuration` answer on the same inbox with zero code, as soon as the
transport subscription is acknowledged.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `IPC` \| `MQTT [path]` | Defaults from the platform; `IPC` only on GREENGRASS. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `SHADOW` \| `CONFIG_COMPONENT` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name = the device (last hierarchy level) in every UNS topic. |
