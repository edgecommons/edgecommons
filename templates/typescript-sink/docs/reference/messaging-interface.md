This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Messaging Interface & CLI

What this scaffold subscribes to, publishes, and accepts, and the CLI flags. Addressing follows the
**Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
model behind the delivery ladder, see [../explanation.md](../explanation.md); for recipes, the
[how-to guides](../how-to-guides.md).

## Envelope

Every message uses the EdgeCommons JSON envelope: `{header, identity, tags, body}`. This sink
consumes whatever `subscribe` matches and delivers the message `body` verbatim (as JSON bytes) to
its destination — it imposes no shape of its own on the payload.

## Topics

| Class | Direction | Topic | Notes |
|-------|-----------|-------|-------|
| (sink-configured) | bus → sink | each sink's `subscribe` filter | A single filter per sink (not an array — see `SinkConfig`). |
| `evt` | sink → bus | `ecv1/{device}/{component}/evt/{severity}/{type}` | The delivery ladder, below. |
| `metric` | sink → bus (auto) | `ecv1/{device}/{component}/metric/sinkDeliveries` | See [metrics.md](metrics.md). |
| `state` | sink → bus (auto) | `ecv1/{device}/{component}/state` | The keepalive, carrying each sink's destination connectivity in `instances[]`. |

`state`/`metric`/`cfg`/`log` are library-owned **reserved** classes — a direct publish to them is
rejected.

## Events (`evt` class) — the delivery ladder

Published through the `events()` facade: severity **derives** the channel `evt/{severity}/{type}`,
so the topic and body can never disagree.

- **`evt/info/delivery-started`** — `{sink, key, kind}`. Emitted once per item, before the first
  delivery attempt.
- **`evt/info/delivery-completed`** — `{sink, key, attempts, elapsedMs}`. Emitted once delivery is
  **verified** (not merely `deliver()`-resolved).
- **`evt/warning/delivery-failed`** — `{sink, key, attempt, willRetry: true, nextAttemptInMs}`.
  Emitted on each transient failure that will be retried.
- **`evt/critical/delivery-exhausted`** — `{sink, key, attempts?, reason}` (`attempts` absent for a
  permanent failure, which never retried). Emitted once the item will never be delivered — either a
  permanent classification or the retry budget spent. This is the one event severity in this
  scaffold above `Warning`, and the one to alert on.

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every `heartbeat.intervalSecs` (default
5s). The RUNNING keepalive's `instances[]` array carries one entry **per configured sink** —
`{instance, connected, state, attributes: {destination}}` — present from the moment a sink is
configured, even before its first delivery attempt. `state` is one of `IDLE | ONLINE | BACKOFF |
FAILED` (this sink's own vocabulary); `connected` is the normalized flag (`true` for `IDLE`/`ONLINE`).

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
