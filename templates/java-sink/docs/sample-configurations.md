# Sample Configurations

> This documents the generated scaffold; rewrite it as you build the component out.

Ready-to-adapt configurations for `<<COMPONENTNAME>>`. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for topics/payloads see
[reference/messaging-interface.md](reference/messaging-interface.md).

## 1. The shipped demo sink

`test-configs/config.json`:

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "factory-1" },
  "component": {
    "global": {
      "defaults": { "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 }, "maxQueue": 256 }
    },
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

| Option | Effect |
|--------|--------|
| `id` | The `{instance}` UNS topic segment for this sink; also prefixes the delivery key, so it must be stable. |
| `subscribe` | The single topic filter this sink consumes. |
| `destination.type` / `path` | The reference backend: a root directory; objects are written to a temp file and renamed into place. |
| `retry.baseDelayMs` / `maxDelayMs` | The backoff window and its ceiling. |
| `retry.giveUpAfterMs` | The time budget before a failure is reported exhausted. |

## 2. A tighter retry budget for latency-sensitive data

```jsonc
"retry": { "baseDelayMs": 500, "maxDelayMs": 30000, "giveUpAfterMs": 120000 }
```

Gives up after two minutes instead of an hour — appropriate when stale data delivered late is worse
than data that is reported lost and re-driven by an upstream retry of its own.

## 3. Two independent sinks

```jsonc
"instances": [
  { "id": "archive", "subscribe": "ecv1/+/+/+/data/#", "destination": { "type": "local", "path": "./out/data" } },
  { "id": "alarms",  "subscribe": "ecv1/+/+/+/evt/critical/#", "destination": { "type": "local", "path": "./out/alarms" } }
]
```

Each sink is independent — its own worker thread, its own bounded queue, its own retry state — so a
struggling destination for one never backs up the other.

## Where settings resolve from (precedence)

`retry` and `maxQueue` resolve per-sink ▸ `component.global.defaults` ▸ the built-in default
(`baseDelayMs: 1000`, `maxDelayMs: 900000`, `giveUpAfterMs: 3600000`, `maxQueue: 256`).
