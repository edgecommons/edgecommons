# Reference — Configuration

*This documents the generated scaffold; rewrite it as you build the component out.*

Every configuration option this scaffold itself understands. For *why*, see
[explanation.md](../explanation.md); for tasks, see the [how-to guides](../how-to-guides.md).

## Config source

The component reads one JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`,
`GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This scaffold's own settings live under
`component`; the sibling sections (`tags`, `hierarchy`, `identity`, `topic`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard edgecommons sections owned by the canonical schema and
are **not** redeclared in `config.schema.json`.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `retry` | object | — | Fallback retry policy for a sink that doesn't override it (see below). |
| `maxQueue` | integer | `256` | How many items may be queued for a sink before new ones are dropped and counted. |

## `component.instances[]` (a sink)

A sink models **one delivery pipeline per instance** — what it consumes, where that goes, and how
hard it tries. Sinks are independent: one thread each, so a destination that is retrying cannot stall
another sink's deliveries.

| Key | Type | Required | Definition |
|-----|------|----------|-----------|
| `id` | string | yes | Unique sink id, lower-kebab (`^[a-z0-9]+(?:-[a-z0-9]+)*$`). The `{instance}` token of this sink's UNS event topics **and** the prefix of every destination key it writes — must be stable. |
| `subscribe` | string | yes | The topic filter whose messages this sink delivers (e.g. `ecv1/+/+/+/data/#`). |
| `destination` | object | yes | Where the sink delivers (see below). |
| `retry` | object | instance/global default | Per-sink override of the retry policy. |
| `maxQueue` | integer | instance/global default | Per-sink override of the queue bound. |

## `destination` (a tagged object)

Whatever the backend, two properties are non-negotiable: delivery is idempotent to a **stable** key
(a retry overwrites rather than duplicating), and it is **verified** before the source is released.

| Type | Keys | Definition |
|---|---|---|
| `local` | `path` (string, required) | The root directory delivered objects land under. |

Add a variant here as you implement a backend in `app/dest.py`'s `build_destination()` — the schema
and that function are one contract.

## `retry`

Exponential backoff with full jitter. The jitter is not decoration: without it every sink that lost
the same endpoint retries on the same instant, and an endpoint that's already struggling gets a
synchronized thundering herd on every backoff boundary.

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `baseDelayMs` | integer | `1000` | The first backoff window. Each attempt doubles it, up to `maxDelayMs`. |
| `maxDelayMs` | integer | `900000` | The backoff ceiling, so a long outage does not back off to next week. |
| `giveUpAfterMs` | integer | `3600000` | The **time budget**, not an attempt count. When it's spent, the item is reported `exhausted` — loudly, because that's data that did not arrive. |

## Precedence

`retry` / `maxQueue` resolve: **sink value ▸ `component.global.defaults`**.

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "metricEmission": { "target": "messaging" },
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

## Limitations

- Only the `local` destination type ships. Add your own backend before deploying anywhere the pod's
  local disk is not durable.
- Unknown keys anywhere in a sink, a destination, or a retry policy are **rejected, not ignored**.
- A component with **zero valid sinks** refuses to start rather than idle silently.
