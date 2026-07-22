# Sample Configurations

*This documents the generated scaffold; rewrite it as you build the component out.*

The shipped `test-configs/config.json` explained option-by-option, plus a non-trivial variant that
turns on the UNS metric target and a faster tick. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for message shapes see
[reference/messaging-interface.md](reference/messaging-interface.md).

The component loads **one JSON document** from `-c/--config`. The top level carries `component`
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
    "global": { "publish_interval": 3 },
    "instances": [ { "id": "main" } ]
  }
}
```

**What each option does at runtime**

| Option | Effect |
|--------|--------|
| `logging.level` / `rust_format` | Standard edgecommons log level + format string. |
| `hierarchy.levels` / `identity` | Places the component in the UNS enterprise tree: `identity.path = "factory-1/<thing>"`. The last hierarchy level's value is always the resolved Thing name (`-t`). |
| `heartbeat.*` | The `state` keepalive cadence and its `cpu`/`memory` system measures — independent of the demo tick. |
| `metricEmission.target: log` | Routes `loopTicks` to a local log file. Set `target: "messaging"` to see it on the UNS `metric` class instead. |
| `component.global.publish_interval` | Seconds between the scaffold's demo tick (app-status / metric / data / event quartet), `3` here. |
| `component.instances[].id: "main"` | The single instance the scaffold's `data()`/`events()`/`metrics()` facades are bound to. |

Run it:

```bash
cargo run -- --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

---

## 2. A non-trivial variant: faster tick, metrics on the bus

```jsonc
{
  "tags": { "line": "5" },
  "hierarchy": { "levels": ["site", "area", "device"] },
  "identity": { "site": "plant1", "area": "assembly" },
  "logging": { "level": "INFO" },
  "messaging": { "local": { "type": "mqtt", "host": "localhost", "port": 1883, "clientId": "<<BINNAME>>-line5" } },
  "metricEmission": { "target": "messaging" },
  "component": {
    "global": { "publish_interval": 1 },
    "instances": [ { "id": "main", "publish_interval": 1 } ]
  }
}
```

**How this behaves differently from the shipped config**

- **Ticks 3× faster** (`publish_interval: 1` vs. the shipped `3`) — the demo status/metric/data/event
  quartet fires every second.
- **`metricEmission.target: "messaging"`** puts `loopTicks` on the UNS `metric` class instead of a
  log file, so `mosquitto_sub -t 'ecv1/+/+/metric/#' -v` shows it directly.
- **A deeper hierarchy** (`["site", "area", "device"]`) places the component further down the
  enterprise tree — `identity.path = "plant1/assembly/<thing>"` — without changing anything else
  about how the demo behaves.
- **`instances[].publish_interval`** demonstrates the per-instance override the schema promises: with
  a single `main` instance it is redundant with the global value here, but a second instance could
  set its own cadence independently.

Run it the same way, pointing `-c FILE` at this file instead.
