# Reference — Messaging Interface & CLI

> This documents the generated scaffold; rewrite it as you build the component out.

Complete specification of the topics this scaffold consumes and produces, and its CLI flags.

## Envelope

All messages use the EdgeCommons JSON envelope, `{header, identity, tags, body}`. A sink's delivery
key is derived from the sink id, the topic leaf, and the envelope's `header.uuid` — never from a
counter or the clock — so a redelivered message with the same `uuid` always overwrites the same
destination object instead of duplicating it.

## Topics

| Direction | Topic | Notes |
|---|---|---|
| consume | the sink's `subscribe` filter | One filter per sink, e.g. `ecv1/+/+/+/data/#`. |
| produce | `ecv1/{device}/<<BINNAME>>/{instance}/evt/{severity}/{type}` | The event ladder, via `getEvents()`. |
| produce (automatic) | `ecv1/{device}/<<BINNAME>>/state` | The library heartbeat keepalive. |
| produce (automatic) | `ecv1/{device}/<<BINNAME>>/metric/sinkDeliveries` | With `metricEmission.target: messaging`. |
| consume/produce (automatic) | `ecv1/{device}/<<BINNAME>>/cmd/#` | The built-in `ping`/`reload-config`/`get-configuration` verbs. |

## The event ladder (`evt` class)

Published through `events().emit(severity, type, message, context)`, which derives the
`evt/{severity}/{type}` channel from the body's own severity + type, so topic and body can never
disagree.

| Event | Severity | When | Context |
|---|---|---|---|
| `delivery-started` | Info | An item entered the loop. | `{sink, key}` |
| `delivery-completed` | Info | Delivered **and verified**. | `{sink, key, attempts, elapsedMs}` |
| `delivery-failed` | Warning | A transient failure; another attempt is scheduled. | `{sink, key, willRetry, nextAttemptInMs}` |
| `delivery-exhausted` | **Critical** | Permanent failure, or the time budget spent. This is data that did not arrive. | `{sink, key, attempts, elapsedMs}` |

Subscribe `ecv1/+/+/+/evt/critical/#` to watch only the events that mean something was lost.

## Built-in command verbs

`ping`, `reload-config`, `get-configuration` answer on `ecv1/{device}/<<BINNAME>>/cmd/#` with zero
code, as soon as the transport subscription is acknowledged. This archetype registers no custom verbs
by default.

## CLI

| Flag | Values | Notes |
|------|--------|-------|
| `--platform` | `GREENGRASS` \| `HOST` \| `KUBERNETES` \| `auto` | Default `auto`. |
| `--transport` | `IPC` \| `MQTT [path]` | Defaults from the platform; `IPC` only on GREENGRASS. |
| `-c/--config` | `FILE <path>` \| `ENV` \| `GG_CONFIG` \| `SHADOW` \| `CONFIG_COMPONENT` \| `CONFIGMAP` | Default from the platform. |
| `-t/--thing` | `<name>` | IoT Thing name = the device (last hierarchy level) in every UNS topic. |
