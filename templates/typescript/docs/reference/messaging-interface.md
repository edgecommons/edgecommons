This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Messaging Interface & CLI

What this scaffold publishes and accepts, and the CLI flags. Addressing follows the **Unified
Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the model behind
the facades, see [../explanation.md](../explanation.md); for recipes, the
[how-to guides](../how-to-guides.md).

## Envelope

Every message uses the EdgeCommons JSON envelope: `{header, identity, tags, body}`. The library
stamps the top-level **`identity`** (`{hier, path, component, instance}`) on every message built
from config. Request/reply carries `header.reply_to` + `header.correlation_id`; the reply publishes
to `reply_to` with the same `correlation_id`.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `app` | `StatusUpdate` (demo) | component → bus | `ecv1/{device}/{component}/app/status` | — |
| `data` | `SouthboundSignalUpdate` (`demo-signal`) | component → bus | `ecv1/{device}/{component}/data/demo-signal` | — |
| `evt` | `sample-event` (demo) | component → bus | `ecv1/{device}/{component}/evt/info/sample-event` | — |
| `cmd` | `ping` / `reload-config` / `get-configuration` (built-in) | bus → component | `ecv1/{device}/{component}/cmd/{verb}` | `{ok,result}` |
| `cmd` | `set-greeting` (demo) | bus → component | `ecv1/{device}/{component}/cmd/set-greeting` | `{ok,result}` |
| `metric` | `loopTicks` (demo) | component → bus (auto, `target: messaging`) | `ecv1/{device}/{component}/metric/loopTicks` | — |
| `state` | keepalive | component → bus (auto) | `ecv1/{device}/{component}/state` | — |

`state`/`metric`/`cfg`/`log` are library-owned **reserved** classes — a direct publish to them is
rejected; this component only ever mints `app`/`data`/`evt` topics via `this.uns`/the `data()`/
`events()` facades, and `cmd` replies via the command inbox.

## The command inbox

The reply body is `{"ok": true, "result": <verb result>}` on success or
`{"ok": false, "error": {"code", "message"}}` on failure.

### `set-greeting` (demo)

```jsonc
// request body:  { "greeting": "Hi there" }
// result: { "previousGreeting": "Hello from <<COMPONENTNAME>>", "greeting": "Hi there" }
```

Throws `BAD_ARGS` when `greeting` is missing or not a string.

## Data plane

### `SouthboundSignalUpdate` (`demo-signal`, `data` class)

Published through `gg.data()`. An omitted quality defaults to `GOOD` (`qualityRaw: "unspecified"`):

```jsonc
"body": {
  "signal": { "id": "demo-signal" },
  "samples": [ { "value": 21.4, "quality": "GOOD", "qualityRaw": "unspecified", "serverTs": "..." } ]
}
```

## Events (`evt` class)

- **`evt/info/sample-event`** — emitted every tick, through `gg.events()`. Context carries
  `{seq, greeting}`.

## Metrics (`metric` class, reserved — automatic)

`loopTicks` — `tickCount` (Count, monotonic) and `uptimeSecs` (Seconds). See
[metrics.md](metrics.md).

## State keepalive (`state` class, reserved — automatic)

Every ~5 s (`heartbeat.intervalSecs`). This scaffold reports **no** instance connectivity
(`instanceConnectivity()` returns `[]`), so the keepalive carries no `instances[]` section.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
