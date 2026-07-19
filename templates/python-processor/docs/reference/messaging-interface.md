# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic and message the scaffold publishes or accepts, and the CLI flags. Addressing follows the
Unified Namespace: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the model behind
this, see [explanation.md](../explanation.md); for client recipes, the [how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (the last `hierarchy` level, or `-t` directly).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — a route id (e.g. `rollup`) for its published output and its events; `main` for the
  shared command inbox, the `state` keepalive, and `metric`.

## Envelope

All messages use the EdgeCommons JSON envelope: `{header, identity, tags, body}`. Every outbound
message is rebuilt through `gg.instance(route.id).new_message(...)`, which stamps this component's
config-resolved identity onto it — never the identity of whoever produced the message this route
consumed.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| *(route-defined)* | whatever the pipeline emits | component → bus | the route's configured `publishTopic` | — |
| `evt` | `publish-failed` | component → bus | `ecv1/{device}/<<BINNAME>>/{route}/evt/warning/publish-failed` | — |
| `cmd` | `ping` / `reload-config` / `get-configuration` | bus → component | `ecv1/{device}/<<BINNAME>>/main/cmd/{verb}` (library built-ins; this scaffold adds none) | `{ok,result}` |
| `metric` | `processorThroughput` | component → bus (auto, when `target: messaging`) | `ecv1/{device}/<<BINNAME>>/main/metric/processorThroughput` | — |
| `state` | keepalive | component → bus (auto) | `ecv1/{device}/<<BINNAME>>/main/state` | — |

A route's own **inbound** subscriptions are named by its `subscribe` config, not fixed by this
scaffold — see [sample-configurations.md](../sample-configurations.md).

Fleet consumers subscribe the six UNS wildcards: telemetry `ecv1/+/+/+/data/#`; events
`ecv1/+/+/+/evt/#`; metrics `ecv1/+/+/+/metric/#`; state `ecv1/+/+/+/state`. `state`/`metric`/`cfg`/
`log` are library-owned **reserved** classes — a component publish to them directly is rejected.

## Events (`evt` class)

Published through `gg.instance(route.id).events()` only when a publish actually fails — not on a
timer. Severity derives the channel, so the topic and body can never disagree.

```jsonc
"body": {
  "severity": "warning", "type": "publish-failed",
  "message": "route rollup could not publish",
  "timestamp": "2026-07-19T00:00:00Z",
  "context": { "route": "rollup", "topic": "ecv1/gw-01/<<BINNAME>>/rollup/data/summary", "reason": "..." }
}
```

## Metrics (`metric` class, reserved — automatic)

`processorThroughput` publishes on `ecv1/{device}/<<BINNAME>>/main/metric/processorThroughput` when
`metricEmission.target` is `messaging`. See [Reference — Metrics](metrics.md) for its measures.

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every ~5 s by default. The RUNNING keepalive
carries an `instances[]` array only when `instance_connectivity()` returns at least one entry — this
scaffold reports none, since a processor's routes are subscriptions on a bus the library already
reports on, not links to a device (see [explanation.md](../explanation.md#instance-connectivity-a-processor-reports-none)).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
