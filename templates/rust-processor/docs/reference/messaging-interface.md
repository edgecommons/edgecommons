# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic this processor subscribes to or publishes, and its CLI flags. Addressing follows the
**Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
pipeline model, see [explanation.md](../explanation.md).

- `{device}` — the resolved Thing name (`-t`, or the last `hierarchy` level).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — a configured route id (`rollup`, …) for its own metric surface; the command inbox
  and `state` keepalive are component-scoped.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| *(configured)* | whatever `subscribe[]` names | bus → processor | per-route `subscribe` filters | — |
| *(configured)* | the transformed result | processor → bus | per-route `publishTopic` | — |
| `cmd` | `ping` / `reload-config` / `get-configuration` | bus → processor | `ecv1/{device}/<<BINNAME>>/cmd/{verb}` | `{ok,result}` |
| `metric` | `processorThroughput` | processor → bus (auto) | `ecv1/{device}/<<BINNAME>>/metric/processorThroughput` | — |
| `state` | keepalive | processor → bus (auto) | `ecv1/{device}/<<BINNAME>>/state` | — |

This scaffold registers no custom command verbs beyond the library's automatic
`ping`/`reload-config`/`get-configuration` — add your own with `gg.commands().register(...)` if your
pipeline needs one (a "flush now" verb for a windowed stage, say).

## Message envelope

Every message uses the EdgeCommons JSON envelope: `{header, identity, tags, body}`. Outbound
messages are rebuilt with `MessageBuilder::new(&m.msg.header.name, &m.msg.header.version)
.from_config(config).payload(...)` — the **identity restamp** — so what this processor publishes
always carries its own identity, never the identity of whoever produced the message it consumed.

```jsonc
// an inbound data message this scaffold's shipped route matches on:
"body": { "signal": { "id": "temperature-1" }, "samples": [ { "value": 21.4 } ] }

// the rollup this scaffold publishes after a tick:
"body": { "count": 3, "last": { "signal": { "id": "temperature-1" }, "samples": [ { "value": 21.4 } ] } }
```

## State keepalive (`state` class, reserved — automatic)

Publishes every ~5 s on `ecv1/{device}/<<BINNAME>>/state`. This scaffold's `instances[]` array is
omitted from the keepalive — a processor reports no southbound connectivity (see
[explanation.md](../explanation.md)).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/Kubernetes use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | Thing name; the `{device}` token of every UNS topic. |
