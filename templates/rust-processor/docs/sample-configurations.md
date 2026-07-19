# Sample Configurations

*This documents the generated scaffold; rewrite it as you build the component out.*

The shipped `test-configs/config.json` explained option-by-option, plus a non-trivial
multi-route variant. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for message shapes see
[reference/messaging-interface.md](reference/messaging-interface.md).

The processor loads **one JSON document** from `-c/--config`. The top level carries `component`
(this scaffold's own config) plus the standard `edgecommons` sections: `tags`, `hierarchy`,
`identity`, `messaging`, `metricEmission`, `logging`, `heartbeat`.

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
    "global": { "defaults": { "tickMs": 10000 } },
    "instances": [
      {
        "id": "rollup",
        "subscribe": ["ecv1/+/+/+/data/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/data/summary",
        "target": "local",
        "pipeline": [
          { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } },
          { "countPerTick": {} }
        ],
        "tickMs": 10000
      }
    ]
  }
}
```

**What each option does at runtime**

| Option | Effect |
|--------|--------|
| `component.global.defaults.tickMs` | Fallback stage-tick cadence for a route that does not set its own. |
| `instances[].id` | The `{instance}` token of this route's `state`/metric surface; also its log prefix. |
| `instances[].subscribe` | Topic filters this route consumes — here, every `data` message on the whole fleet. |
| `instances[].publishTopic` | Where the transformed result goes. A processor is payload-agnostic, so this is a plain config string, not a signal-id-derived topic. |
| `instances[].target: local` | Publishes on the device-local bus. |
| `instances[].pipeline[0].fieldEquals` | Keeps only messages whose `body.signal.id` equals `"temperature-1"`; drops everything else. |
| `instances[].pipeline[1].countPerTick` | Accumulates kept messages; emits `{"count": N, "last": <last body>}` once per tick. |
| `instances[].tickMs` | Per-route override of the global default (here, equal to it — 10 s). |
| `instances[].maxQueue` | Not set — defaults to `256` (`component.schema.json`'s default). |

Run it:

```bash
cargo run -- --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

---

## 2. A non-trivial variant: two routes, one northbound

Two independent routes from the same process — a fast local rollup and a slower northbound alarm
summary:

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
    "global": { "defaults": { "tickMs": 5000, "maxQueue": 512 } },
    "instances": [
      {
        "id": "temps",
        "subscribe": ["ecv1/+/+/+/data/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/temps/data/summary",
        "target": "local",
        "pipeline": [
          { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } },
          { "countPerTick": {} }
        ],
        "tickMs": 2000
      },
      {
        "id": "alarms",
        "subscribe": ["ecv1/+/+/+/evt/critical/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/alarms/data/summary",
        "target": "northbound",
        "pipeline": [ { "countPerTick": {} } ],
        "maxQueue": 128
      }
    ]
  }
}
```

**How this behaves differently from the shipped config**

- **Two independent tasks.** `temps` and `alarms` run in separate `tokio` tasks with their own
  queues — a burst on one cannot cause the other to drop messages.
- **`temps` ticks 5× faster** than `alarms` (`tickMs: 2000` overrides the `global.defaults` value of
  `5000`; `alarms` inherits the default).
- **`alarms` has no filter stage** — every `evt/critical/#` message it subscribes to is counted, none
  are dropped by a `fieldEquals` stage first (an empty-filter route is a legitimate "count
  everything" pipeline).
- **`alarms` has a smaller queue** (`maxQueue: 128` vs. the `512` global default) — appropriate for a
  route expected to see occasional bursts of critical alarms rather than steady high-rate telemetry.
- **`alarms.target: "northbound"`** publishes the rollup with `Qos::AtLeastOnce` over the northbound
  broker connection instead of the local bus — appropriate for a low-rate, actionable summary a
  cloud-side system should see.
- **`metricEmission.target: "messaging"`** puts `processorThroughput` on the UNS `metric` class
  instead of a log file, so `mosquitto_sub -t 'ecv1/+/+/+/metric/#' -v` shows it directly.

Run it the same way, pointing `-c FILE` at this file instead.
