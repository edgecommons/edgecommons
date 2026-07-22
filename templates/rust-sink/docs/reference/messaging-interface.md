# Reference — Messaging Interface & CLI

*This documents the generated scaffold; rewrite it as you build the component out.*

Every topic this sink subscribes to or publishes, and its CLI flags. Addressing follows the
**Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
delivery model, see [explanation.md](../explanation.md).

- `{device}` — the resolved Thing name (`-t`, or the last `hierarchy` level).
- `{component}` — the component UNS token, `<<BINNAME>>`.
- `{instance}` — a configured sink id (`archive`, …) for its own `evt`/connectivity surface; the
  command inbox and `state` keepalive are component-scoped.

## Topics

| Class | Message | Direction | Topic | Reply |
|-------|---------|-----------|-------|-------|
| *(configured)* | whatever `subscribe` names | bus → sink | per-sink `subscribe` filter | — |
| `evt` | `evt` | sink → bus | `ecv1/{device}/<<BINNAME>>/{instance}/evt/{severity}/{type}` | — |
| `cmd` | `ping` / `reload-config` / `get-configuration` | bus → sink | `ecv1/{device}/<<BINNAME>>/cmd/{verb}` | `{ok,result}` |
| `metric` | `sinkDeliveries` | sink → bus (auto) | `ecv1/{device}/<<BINNAME>>/metric/sinkDeliveries` | — |
| `state` | keepalive | sink → bus (auto) | `ecv1/{device}/<<BINNAME>>/state` | — |

This scaffold registers no custom command verbs beyond the library's automatic
`ping`/`reload-config`/`get-configuration`.

## Events (`evt` class)

Published through the library's `events()` facade; severity **derives** the channel
(`evt/{severity}/{type}`), so the topic and the body can never disagree.

| Type | Severity | When |
|------|----------|------|
| `delivery-started` | info | A delivery attempt begins. |
| `delivery-completed` | info | Delivered and verified; carries `attempts`, `elapsedMs`. |
| `delivery-failed` | warning | A transient failure with retry budget remaining; carries `attempt`, `willRetry: true`, `nextAttemptInMs`. |
| `delivery-exhausted` | critical | A permanent failure, or the retry time budget is spent; carries `attempts`/`reason`. This is data that did not arrive — deliberately loud. |

```jsonc
"body": {
  "severity": "critical", "type": "delivery-exhausted",
  "message": "archive gave up on archive/temp/9c2e....json",
  "context": { "sink": "archive", "key": "archive/temp/9c2e....json", "attempts": 6, "reason": "..." }
}
```

## State keepalive (`state` class, reserved — automatic)

Publishes every ~5 s on `ecv1/{device}/<<BINNAME>>/state`. The RUNNING keepalive's `instances[]`
array carries one entry per configured sink — **a sink's destinations are its instances**:

```jsonc
{ "instance": "archive", "connected": true, "state": "IDLE",
  "detail": "./out", "attributes": { "destination": "local" } }
```

`state` is this sink's own vocabulary: `IDLE` (untried, reachable) / `ONLINE` (last delivery
verified) / `BACKOFF` (retrying) / `FAILED` (gave up — an operator must be paged).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/Kubernetes use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | Thing name; the `{device}` token of every UNS topic. |
