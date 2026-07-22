# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic and message the scaffold publishes or accepts, and the CLI flags. Addressing follows the
Unified Namespace: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the model behind
this, see [explanation.md](../explanation.md); for client recipes, the [how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (the last `hierarchy` level, or `-t` directly).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — `main` for every topic below; the scaffold reports no other instances (see
  [explanation.md](../explanation.md#one-provider-two-surfaces-instance-connectivity)).

## Envelope

All messages use the EdgeCommons JSON envelope: `{header, identity, tags, body}`. The library stamps
the top-level **`identity`** (`{hier, path, component, instance}`) on every message built from config.
Request/reply carries `header.reply_to` + `header.correlation_id`; the reply publishes to `reply_to`
with the same `correlation_id`.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `app` | `StatusUpdate` | component → bus | `ecv1/{device}/<<BINNAME>>/main/app/status` | — |
| `metric` | `loopTicks` | component → bus (auto, when `target: messaging`) | `ecv1/{device}/<<BINNAME>>/main/metric/loopTicks` | — |
| `data` | `SouthboundSignalUpdate` | component → bus | `ecv1/{device}/<<BINNAME>>/main/data/demo-signal` | — |
| `evt` | `evt` | component → bus | `ecv1/{device}/<<BINNAME>>/main/evt/info/sample-event` | — |
| `cmd` | `set-greeting` | bus → component | `ecv1/{device}/<<BINNAME>>/main/cmd/set-greeting` | `{ok,result}` |
| `cmd` | `ping` / `reload-config` / `get-configuration` | bus → component | `ecv1/{device}/<<BINNAME>>/main/cmd/{verb}` (library built-ins) | `{ok,result}` |
| `state` | keepalive | component → bus (auto) | `ecv1/{device}/<<BINNAME>>/main/state` | — |

Fleet consumers subscribe the six UNS wildcards: telemetry `ecv1/+/+/+/data/#`; events
`ecv1/+/+/+/evt/#`; metrics `ecv1/+/+/+/metric/#`; state `ecv1/+/+/+/state`. `state`/`metric`/`cfg`/
`log` are library-owned **reserved** classes — a component publish to them directly is rejected; the
scaffold only ever mints `app`/`data`/`evt` topics via facades and `cmd` replies via the command
inbox, never a hand-assembled topic string.

## The command inbox

Served through the library's **command inbox** — a single component-scope subscription
`ecv1/{device}/<<BINNAME>>/main/cmd/#`. A request's **verb** is the topic channel after `cmd/` and
must equal `header.name`. Built-in verbs (`ping`, `reload-config`, `get-configuration`, `status`) ship
with every component; the scaffold adds `set-greeting`.

### `set-greeting` (command)

```jsonc
// request body
"body": { "greeting": "Hi there" }
// reply body: { "ok": true, "result": { "previousGreeting": "Hello from <<COMPONENTNAME>>", "greeting": "Hi there" } }
```

A malformed body (missing/non-string `greeting`) replies `{"ok": false, "error": {"code": "BAD_ARGS", ...}}`.

## Data plane

### `demo-signal` (`data` class)

Published through the `data()` facade, which constructs the body, sanitizes the channel, mints the
topic, and stamps identity:

```jsonc
"body": {
  "device": { "adapter": "<<BINNAME>>", "instance": "main" },
  "signal": { "id": "demo-signal" },
  "samples": [ { "value": 21.4, "quality": "GOOD", "qualityRaw": "unspecified", "serverTs": "2026-07-19T00:00:00Z" } ]
}
```

An omitted quality defaults to `GOOD` with `qualityRaw: "unspecified"` — a synthesized value, marked
as such so a consumer can tell it apart from a device-reported `GOOD`.

## Events (`evt` class)

Published through the `events()` facade: severity **derives** the channel `evt/{severity}/{type}`,
so the topic and the body can never disagree.

```jsonc
"body": {
  "severity": "info", "type": "sample-event", "message": "sample event from <<COMPONENTNAME>>",
  "timestamp": "2026-07-19T00:00:00Z", "context": { "seq": 12, "greeting": "Hello from <<COMPONENTNAME>>" }
}
```

## Metrics (`metric` class, reserved — automatic)

`loopTicks` publishes on `ecv1/{device}/<<BINNAME>>/main/metric/loopTicks` when
`metricEmission.target` is `messaging` (the default `log` target writes a local file instead). See
[Reference — Metrics](metrics.md) for its measures.

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every ~5 s by default
(`heartbeat.intervalSecs`). The RUNNING keepalive carries an `instances[]` array only when
`instance_connectivity()` returns at least one entry — the scaffold reports none, so this section is
omitted until you add a real connection (see the [how-to guide](../how-to-guides.md#report-a-real-connection)).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
