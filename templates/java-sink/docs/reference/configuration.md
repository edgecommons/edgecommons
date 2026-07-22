# Reference — Configuration

> This documents the generated scaffold; rewrite it as you build the component out.

Complete reference for every configuration option this scaffold understands. The machine-readable
source of truth is `config.schema.json`; `SinkConfig.parse` enforces it at runtime, including
rejecting an unknown key.

## Config source

| Platform | Default source | Example |
|----------|----------------|---------|
| `HOST` | `FILE <path>` | `-c FILE ./config.json` |
| `GREENGRASS` | `GG_CONFIG` | the deployment `ComponentConfiguration` |
| `KUBERNETES` | `CONFIGMAP` | a mounted ConfigMap directory (re-read on change) |

Sink settings live under `component`; the sibling sections (`hierarchy`, `identity`, `messaging`,
`logging`, `heartbeat`, `metricEmission`, `credentials`) are standard EdgeCommons sections, owned by
the canonical schema and not redeclared here.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `retry` | object | — | See `retry` below; applied to every sink that does not override it. |
| `maxQueue` | integer | `256` | How many items may be queued for a sink before new ones are dropped and counted. |

## `component.instances[]` (one sink per entry)

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | — | Unique sink id (lower-kebab). The `{instance}` UNS topic segment, and it prefixes the destination key — must be stable. |
| `subscribe` | string | yes | — | The topic filter whose messages this sink delivers. |
| `destination` | object | yes | — | Where the sink delivers (below). |
| `retry` | object | no | inherits `global.defaults` | Per-sink override of the retry policy. |
| `maxQueue` | integer | no | inherits `global.defaults` | Per-sink override of the queue bound. |

### `destination`

A tagged object. Two properties are non-negotiable whatever the backend: delivery is idempotent to a
stable key, and it is verified before the source is released.

| `type` | Keys | Definition |
|---|---|---|
| `local` | `path` | The root directory delivered objects land under. Objects are written to a temp file and renamed into place. |

Add a variant here as you implement a backend in `Destination.build` — see
[How-to — Add a destination](../how-to-guides.md#add-a-destination).

### `retry`

Exponential backoff with full jitter.

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `baseDelayMs` | integer | `1000` | The first backoff window. Each attempt doubles it, up to `maxDelayMs`. |
| `maxDelayMs` | integer | `900000` | The backoff ceiling, so a long outage does not back off to next week. |
| `giveUpAfterMs` | integer | `3600000` | The time budget, not an attempt count. When spent, the item is reported exhausted. |

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "factory-1" },
  "component": {
    "global": { "defaults": { "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 }, "maxQueue": 256 } },
    "instances": [
      {
        "id": "archive",
        "subscribe": "ecv1/+/+/+/data/#",
        "destination": { "type": "local", "path": "./out" },
        "retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 }
      }
    ]
  }
}
```

## Ignored / rejected keys

`config.schema.json` sets `additionalProperties: false` throughout, and `SinkConfig.parse` rejects an
unknown key at runtime for the same reason — a typo in a sink's config is how data quietly goes to
the wrong place.
