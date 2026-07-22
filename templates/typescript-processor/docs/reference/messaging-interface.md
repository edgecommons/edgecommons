This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Messaging Interface & CLI

What this scaffold subscribes to, publishes, and accepts, and the CLI flags. Addressing follows the
**Unified Namespace (UNS)**: `ecv1/{device}/{component}/{instance}/{class}[/channel]`. For the
model behind the archetype, see [../explanation.md](../explanation.md); for recipes, the
[how-to guides](../how-to-guides.md).

## Envelope

Every message uses the EdgeCommons JSON envelope: `{header, identity, tags, body}`. A processor is
**payload-agnostic** — it never imposes a body shape of its own; whatever it consumes, minus
whatever a stage drops or transforms, is what it republishes. `App.dispatch` restamps the top-level
`identity` to this component's own on every publish (never the producer's) — see
[../explanation.md](../explanation.md#the-identity-restamp).

## Topics

| Class | Direction | Topic | Notes |
|-------|-----------|-------|-------|
| (route-configured) | bus → processor | each route's `subscribe[]` filters | Wildcards allowed. Self-echo (our own identity) is dropped before it reaches a stage. |
| (route-configured) | processor → bus \| northbound | each route's `publishTopic` | Config-template-resolved. `target: local` publishes on the device-local bus; `target: northbound` sends straight to the northbound broker. |
| `metric` | processor → bus (auto) | `ecv1/{device}/{component}/metric/{metricName}` | `processorThroughput`, below. |
| `state` | processor → bus (auto) | `ecv1/{device}/{component}/state` | The keepalive. This component reports no per-instance connectivity — see [../explanation.md](../explanation.md). |

`state`/`metric`/`cfg`/`log` are library-owned **reserved** classes — a direct publish to them is
rejected. This component addresses its own topics only via `subscribe[]`/`publishTopic` in config
and raw `gg.messaging()` — deliberately not the `data()` facade (a processor is payload-agnostic;
see the explanation page for why).

## Events (`evt` class)

- **`evt/warning/publish-failed`** — a route's publish attempt threw. Context carries
  `{route, topic}`.

## Metrics (`metric` class, reserved — automatic)

`processorThroughput` — component-wide (not per-route) counters, emitted every 60 seconds:
`received`, `published`, `dropped`, `errors`. See [metrics.md](metrics.md).

## State keepalive (`state` class, reserved — automatic)

The library's heartbeat publishes the `state` keepalive every `heartbeat.intervalSecs` (default
5s). This scaffold reports **no** instance connectivity (a processor's routes are not connections —
see [../explanation.md](../explanation.md)), so the keepalive carries no `instances[]` section.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `MQTT [path]` \| `IPC` | HOST/K8s use MQTT; the path is the messaging config. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `CONFIGMAP` \| … | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name; the `{device}` token of every UNS topic. |
