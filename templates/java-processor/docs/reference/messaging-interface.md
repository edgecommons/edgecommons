# Reference — Messaging Interface & CLI

> This documents the generated scaffold; rewrite it as you build the component out.

Complete specification of the topics this scaffold consumes and produces, and its CLI flags.

> **Unified Namespace.** Every route's `publishTopic` must name an application class (`data`, `evt`,
> `app`, `cmd`) — the reserved classes (`state`, `metric`, `cfg`, `log`) are library-owned and are
> rejected on a direct publish.

## Envelope

All messages use the EdgeCommons JSON envelope, `{header, identity, tags, body}`. A processor
forwards the **incoming body verbatim** through its pipeline (stages transform it; the archetype
never assumes a `SouthboundSignalUpdate` shape) and **restamps `identity`** with its own config —
`.withConfig(configManager)` — before publishing, so a downstream consumer always knows which
component actually emitted a message, not who originally produced the data it summarizes.

## Topics

| Direction | Topic | Notes |
|---|---|---|
| consume | each route's `subscribe[]` filters | Wildcards allowed (`ecv1/+/+/+/data/#`). |
| produce | each route's `publishTopic` | An application class only (`data`/`evt`/`app`/`cmd`); `target: local` or `northbound` chooses the transport. |
| produce (automatic) | `ecv1/{device}/<<BINNAME>>/state` | The library heartbeat keepalive. |
| produce (automatic) | `ecv1/{device}/<<BINNAME>>/metric/processorThroughput` | With `metricEmission.target: messaging`. |
| consume/produce (automatic) | `ecv1/{device}/<<BINNAME>>/cmd/#` | The built-in `ping`/`reload-config`/`get-configuration` verbs. |

## The self-echo guard

Every message this component consumes is checked against its own `identity.path` +
`identity.component` before it enters a route's queue; a match is dropped silently (it is our own
already-restamped output) rather than being reprocessed and republished in a loop.

## The demo pipeline's output shape

`countPerTick` emits (on the source message's own header name/version, so a consumer sees a
recognizable message type, not a generic wrapper):

```jsonc
"body": { "count": 3, "last": { /* the last matching message's body, verbatim */ } }
```

## Built-in command verbs

`ping`, `reload-config`, `get-configuration` answer on `ecv1/{device}/<<BINNAME>>/cmd/#` with zero
code, as soon as the transport subscription is acknowledged.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `IPC` \| `MQTT [path]` | Defaults from the platform; `IPC` only on GREENGRASS. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `SHADOW` \| `CONFIG_COMPONENT` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name = the device (last hierarchy level) in every UNS topic. |
