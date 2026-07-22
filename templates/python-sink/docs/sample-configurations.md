# Sample Configurations

*This documents the generated scaffold; rewrite it as you build the component out.*

The scaffold ships one working config, `test-configs/config.json`, plus the MQTT
`standalone-messaging.json` for local HOST runs. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for the delivery model see
[explanation.md](explanation.md).

## `test-configs/config.json` — one sink, local-filesystem destination

```jsonc
{
  "logging": { "level": "INFO", "python_format": "..." },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "log", "namespace": "edgecommons", "targetConfig": { "logFileName": "{ComponentFullName}.metric.log" } },
  "tags": { "site": "factory-1" },
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

**What each option does at runtime**

| Option | Effect |
|--------|--------|
| `component.global.defaults.retry` | Fallback retry policy for a sink that doesn't override it: `baseDelayMs` (first backoff window), `maxDelayMs` (backoff ceiling), `giveUpAfterMs` (the time budget — not an attempt count). |
| `component.global.defaults.maxQueue` | Fallback queue bound — how many items may wait for this sink's thread before new ones are dropped and counted. |
| `instances[].id` | The sink's UNS instance token (`archive`) **and** the prefix of every destination key it writes. Must stay stable — changing it sends every future redelivery somewhere new. |
| `subscribe` | The single topic filter this sink delivers. Here, the whole `data` class. |
| `destination.type: local` | Delivers to the local filesystem, rooted at `destination.path`. Writes to a temp file and renames atomically; lands at a deterministic key so a redelivery overwrites. |
| `retry.baseDelayMs` | First backoff window (ms); each attempt doubles it, up to `maxDelayMs`. |
| `retry.giveUpAfterMs` | The time budget. When it's spent, the item is reported `exhausted` — loudly, because that's data that did not arrive. |

## `test-configs/standalone-messaging.json` — the HOST/MQTT broker

```json
{ "messaging": { "local": { "host": "localhost", "port": 1883, "clientId": "<<BINNAME>>-local" } } }
```

Passed as the `--transport MQTT <path>` argument, independent of the `-c FILE ...` component config.

## Adding a second sink

Sinks are independent — add another entry to `component.instances[]` with its own `id`, `subscribe`,
and `destination`. A destination that is retrying can never stall another sink's deliveries (each
gets its own thread and its own bounded queue).
