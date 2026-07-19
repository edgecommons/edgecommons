# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic this component publishes or accepts, and its CLI flags. Addressing follows the
**Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For why the
facades are used instead of hand-built topics, see [explanation.md](../explanation.md).

- `{device}` — the resolved Thing name (`-t`, or the last `hierarchy` level).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — this scaffold's demo publishes use the **component-scoped** facades
  (`gg.data()`/`gg.events()`/`gg.metrics()`), so no instance token appears in their topics; use
  `gg.instance(id)?` for instance-scoped topics/messages instead.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `app` | `StatusUpdate` | component → bus | `ecv1/{device}/<<BINNAME>>/app/status` | — |
| `metric` | `loopTicks` | component → bus (target-dependent) | `ecv1/{device}/<<BINNAME>>/metric/loopTicks` | — |
| `data` | `demo-signal` | component → bus | `ecv1/{device}/<<BINNAME>>/data/demo-signal` | — |
| `evt` | `sample-event` | component → bus | `ecv1/{device}/<<BINNAME>>/evt/info/sample-event` | — |
| `cmd` | `set-greeting` | bus → component | `ecv1/{device}/<<BINNAME>>/cmd/set-greeting` | `{ok,result}` |
| `cmd` | `ping` / `reload-config` / `get-configuration` | bus → component | `ecv1/{device}/<<BINNAME>>/cmd/{verb}` | `{ok,result}` |
| `state` | keepalive | component → bus (auto) | `ecv1/{device}/<<BINNAME>>/state` | — |

`state`/`metric`/`cfg` are library-owned **reserved** classes — this scaffold never writes them
directly; it only calls the `metrics()`/`data()`/`events()` facades and lets the library mint the
topic.

## Data plane

### `demo-signal` (`data` class)

Published through `gg.data().publish_value("demo-signal", value)`, which omits an explicit quality
and lets the facade default it to `GOOD` (`qualityRaw: "unspecified"`):

```jsonc
"body": {
  "signal": { "id": "demo-signal" },
  "samples": [ { "value": 21.7, "quality": "GOOD", "qualityRaw": "unspecified", "serverTs": "2026-07-19T00:00:00Z" } ]
}
```

### `set-greeting` (command)

```jsonc
"body": { "greeting": "Hi there" }
// result: { "previousGreeting": "Hello from <<COMPONENTNAME>>", "greeting": "Hi there" }
```

A malformed body (missing `greeting`) replies `{"ok": false, "error": {"code": "BAD_ARGS", ...}}`.

## Events (`evt` class)

`sample-event` is emitted through `gg.events().emit(Severity::Info, "sample-event", message,
context)` on a fixed timer — a real component should emit on actual occurrences instead (a threshold
crossed, a connection lost/restored), and use `raise_alarm`/`clear_alarm` for a stateful condition.

```jsonc
"body": {
  "severity": "info", "type": "sample-event",
  "message": "sample event from <<COMPONENTNAME>>",
  "context": { "seq": 4, "greeting": "Hello from <<COMPONENTNAME>>" }
}
```

## State keepalive (`state` class, reserved — automatic)

Publishes every ~5 s on `ecv1/{device}/<<BINNAME>>/state`. This scaffold's instance-connectivity
provider returns an empty list, so the RUNNING keepalive's `instances[]` section is omitted — see
[explanation.md](../explanation.md).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/Kubernetes use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | Thing name; the `{device}` token of every UNS topic. |
