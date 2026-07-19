# Reference — Configuration

*This documents the generated scaffold; rewrite it as you build the component out.*

Every option `<<COMPONENTNAME>>` itself understands. For *why*, see
[explanation.md](../explanation.md); for tasks, the [how-to guides](../how-to-guides.md); for
worked examples, [sample-configurations.md](../sample-configurations.md).

## Config source

The sink reads one JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`,
`GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under
`component`; the sibling sections (`tags`, `hierarchy`, `identity`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard `edgecommons` sections, owned by the canonical schema
and not redeclared here.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `retry` | object | see below | Fallback retry policy for a sink that does not override it. |
| `maxQueue` | integer | `256` | Fallback queue bound: how many items may be queued for a sink before new ones are dropped and counted. |

## `component.instances[]` (one sink each)

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `id` | string | **required** | Unique sink id; must be lower-kebab (`^[a-z0-9]+(?:-[a-z0-9]+)*$`). Prefixes the destination key, so it must be stable. |
| `subscribe` | string | **required** | The single topic filter whose messages this sink delivers. |
| `destination` | object | **required** | Where they go (below). |
| `retry` | object | `component.global.defaults.retry` | Per-sink override of the retry policy. |
| `maxQueue` | integer | `component.global.defaults.maxQueue` | Per-sink override of the queue bound. |

### `destination`

A tagged object; add a variant here as you implement a backend in `src/dest.rs`.

| `type` | Fields | Definition |
|--------|--------|-----------|
| `local` | `path` (string, required) | The root directory delivered objects land under. |

Whatever the backend, two properties are non-negotiable: delivery is idempotent to a **stable** key
(a retry overwrites, never duplicates), and it is **verified** before the source is released.

### `retry`

Exponential backoff with full jitter.

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `baseDelayMs` | integer | `1000` | The first backoff window. Each attempt doubles it, up to `maxDelayMs`. |
| `maxDelayMs` | integer | `900000` | The backoff ceiling (15 min), so a long outage does not back off to next week. |
| `giveUpAfterMs` | integer | `3600000` | The **time budget** (1 hour), not an attempt count. When spent, the item is reported `delivery-exhausted` — loudly, because that is data that did not arrive. |

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "messaging": { "local": { "type": "mqtt", "host": "localhost", "port": 1883 } },
  "metricEmission": { "target": "messaging" },
  "component": {
    "global": { "defaults": { "maxQueue": 512 } },
    "instances": [
      {
        "id": "archive",
        "subscribe": "ecv1/+/+/+/data/#",
        "destination": { "type": "local", "path": "./out" },
        "retry": { "baseDelayMs": 500, "maxDelayMs": 60000, "giveUpAfterMs": 1800000 }
      }
    ]
  }
}
```

## Limitations

- `additionalProperties: false` throughout, so a typo'd key is caught at deploy time — extend the
  schema in the same change you extend `src/dest.rs`/`src/app.rs`.
- The shipped `local` destination writes to the process's own filesystem — on Kubernetes that means
  a mounted volume, or the data does not survive pod restarts.
