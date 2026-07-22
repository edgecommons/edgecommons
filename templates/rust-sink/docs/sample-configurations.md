# Sample Configurations

*This documents the generated scaffold; rewrite it as you build the component out.*

The shipped `test-configs/config.json` explained option-by-option, plus a non-trivial
multi-sink variant. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for message/event shapes see
[reference/messaging-interface.md](reference/messaging-interface.md).

The sink loads **one JSON document** from `-c/--config`. The top level carries `component` (this
scaffold's own config) plus the standard `edgecommons` sections: `tags`, `hierarchy`, `identity`,
`messaging`, `metricEmission`, `logging`, `heartbeat`.

---

## 1. The shipped `test-configs/config.json`

```jsonc
{
  "logging": { "level": "DEBUG", "rust_format": "{timestamp} [{level}] [{component}] {target} - {message}" },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "log", "namespace": "edgecommons" },
  "tags": { "site": "factory-1" },
  "component": {
    "global": { "defaults": { "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 } } },
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
| `component.global.defaults.retry` | Fallback retry policy for a sink that does not set its own. |
| `instances[].id` | The `{instance}` token of this sink's connectivity report; also prefixes its delivery key (`archive/...`) and its log lines. |
| `instances[].subscribe` | The single topic filter whose messages this sink delivers — here, every `data` message fleet-wide. |
| `instances[].destination.type: local` | Delivers to a directory on this device. |
| `instances[].destination.path` | The root directory delivered objects land under, relative to the process's working directory. |
| `instances[].retry.baseDelayMs` | The first backoff window (doubles each attempt, capped at `maxDelayMs`). |
| `instances[].retry.maxDelayMs` | The backoff ceiling — `900000` ms (15 min), so a long outage does not back off to next week. |
| `instances[].retry.giveUpAfterMs` | The time budget — `3600000` ms (1 hour) — not an attempt count. |
| `instances[].maxQueue` | Not set — defaults to `256`. |

Run it:

```bash
cargo run -- --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

---

## 2. A non-trivial variant: two sinks, different urgency

A fast-give-up "alerts" sink alongside the patient "archive" sink, sharing the process but with very
different retry postures:

```jsonc
{
  "tags": { "line": "5" },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "plant1" },
  "logging": { "level": "INFO" },
  "messaging": {
    "local": { "type": "mqtt", "host": "localhost", "port": 1883, "clientId": "<<BINNAME>>-line5" }
  },
  "metricEmission": { "target": "messaging" },
  "component": {
    "global": { "defaults": { "maxQueue": 512 } },
    "instances": [
      {
        "id": "archive",
        "subscribe": "ecv1/+/+/+/data/#",
        "destination": { "type": "local", "path": "/var/lib/<<BINNAME>>/archive" },
        "retry": { "baseDelayMs": 2000, "maxDelayMs": 900000, "giveUpAfterMs": 86400000 }
      },
      {
        "id": "alerts",
        "subscribe": "ecv1/+/+/+/evt/critical/#",
        "destination": { "type": "local", "path": "/var/lib/<<BINNAME>>/alerts" },
        "retry": { "baseDelayMs": 250, "maxDelayMs": 5000, "giveUpAfterMs": 30000 },
        "maxQueue": 64
      }
    ]
  }
}
```

**How this behaves differently from the shipped config**

- **Two independent delivery tasks.** `archive` and `alerts` each own their queue and destination
  health — a backlog on one does not affect the other's connectivity report.
- **`archive` is patient**: a 24-hour give-up (`giveUpAfterMs: 86400000`) appropriate for
  high-volume telemetry where a delayed archive is still useful days later.
- **`alerts` gives up fast** (30 seconds): a critical alarm that cannot be delivered within 30 seconds
  is better escalated another way than queued indefinitely — its small `maxQueue: 64` reflects that
  it expects low, bursty volume, not steady telemetry.
- **`alerts` retries faster and with a lower ceiling** (`baseDelayMs: 250`, `maxDelayMs: 5000`) —
  appropriate for a destination expected to recover quickly (a local disk hiccup) rather than a
  prolonged outage.
- **`metricEmission.target: "messaging"`** puts `sinkDeliveries` on the UNS `metric` class instead of
  a log file.

Run it the same way, pointing `-c FILE` at this file instead.
