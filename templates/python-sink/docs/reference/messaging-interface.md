# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic and message the scaffold publishes or accepts, and the CLI flags. Addressing follows the
Unified Namespace: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the model behind
this, see [explanation.md](../explanation.md); for client recipes, the [how-to guides](../how-to-guides.md).

- `{device}` — the resolved Thing name (the last `hierarchy` level, or `-t` directly).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — a sink id (e.g. `archive`) for its events; `main` for the shared command inbox, the
  `state` keepalive, and `metric`.

## Envelope

All messages use the EdgeCommons JSON envelope: `{header, identity, tags, body}`. Every event a sink
emits rides `gg.instance(sink.id).events()`, which stamps this component's config-resolved identity
with the sink's instance token.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| `evt` | `delivery-started` | component → bus | `ecv1/{device}/<<BINNAME>>/{sink}/evt/info/delivery-started` | — |
| `evt` | `delivery-completed` | component → bus | `ecv1/{device}/<<BINNAME>>/{sink}/evt/info/delivery-completed` | — |
| `evt` | `delivery-failed` | component → bus | `ecv1/{device}/<<BINNAME>>/{sink}/evt/warning/delivery-failed` | — |
| `evt` | `delivery-exhausted` | component → bus | `ecv1/{device}/<<BINNAME>>/{sink}/evt/critical/delivery-exhausted` | — |
| `cmd` | `ping` / `reload-config` / `get-configuration` | bus → component | `ecv1/{device}/<<BINNAME>>/main/cmd/{verb}` (library built-ins; this scaffold adds none) | `{ok,result}` |
| `metric` | `sinkDeliveries` | component → bus (auto, when `target: messaging`) | `ecv1/{device}/<<BINNAME>>/main/metric/sinkDeliveries` | — |
| `state` | keepalive | component → bus (auto) | `ecv1/{device}/<<BINNAME>>/main/state` | — |

A sink's own **inbound** subscription is named by its `subscribe` config, not fixed by this scaffold
— see [sample-configurations.md](../sample-configurations.md).

Fleet consumers subscribe the six UNS wildcards: events `ecv1/+/+/+/evt/#` (or just the alarms,
`ecv1/+/+/+/evt/critical/#`); metrics `ecv1/+/+/+/metric/#`; state `ecv1/+/+/+/state`. `state`/
`metric`/`cfg`/`log` are library-owned **reserved** classes — a component publish to them directly is
rejected.

## Events (`evt` class) — the delivery ladder

Published through `gg.instance(sink.id).events()`: severity **derives** the channel
`evt/{severity}/{type}`, so the topic and the body can never disagree.

| Event | Severity | When |
|---|---|---|
| `delivery-started` | Info | the item was dequeued and delivery began |
| `delivery-completed` | Info | delivered **and verified**; the source is released here, never before |
| `delivery-failed` | Warning | a transient failure; body carries `willRetry: true` and `nextAttemptInMs` |
| `delivery-exhausted` | **Critical (alarm)** | permanent failure, or the time budget is spent — **this is data that did not arrive** |

```jsonc
"body": {
  "severity": "critical", "type": "delivery-exhausted",
  "message": "archive gave up on archive/temperature-1/uuid.json",
  "timestamp": "2026-07-19T00:00:00Z",
  "context": { "sink": "archive", "key": "archive/temperature-1/uuid.json", "attempts": 6, "reason": "..." },
  "alarm": true, "active": true
}
```

## Metrics (`metric` class, reserved — automatic)

`sinkDeliveries` publishes on `ecv1/{device}/<<BINNAME>>/main/metric/sinkDeliveries` when
`metricEmission.target` is `messaging`. See [Reference — Metrics](metrics.md) for its measures.

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every ~5 s by default. The RUNNING keepalive
carries an `instances[]` array with **one entry per configured sink** — a sink's destinations *are*
its instances, reported from startup before a single message arrives (see
[explanation.md](../explanation.md#instance-connectivity-a-sinks-destinations-are-its-instances)).

```jsonc
"body": {
  "status": "RUNNING", "uptimeSecs": 3600,
  "instances": [
    { "instance": "archive", "connected": true,  "detail": "delivered archive/temperature-1/uuid.json" }
  ]
}
```

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
