This documents the generated scaffold; rewrite it as you build the component out.

# Sample Configurations

Complete configurations for `<<COMPONENTFULLNAME>>`, from the shipped dev config to a
multi-device variant — with an explanation of what each option changes at runtime. For the
exhaustive option list see [reference/configuration.md](reference/configuration.md); for message
shapes see [reference/messaging-interface.md](reference/messaging-interface.md); for the model
behind them, [explanation.md](explanation.md).

The component loads **one JSON document** from `-c/--config`. The top level may contain
`component` (required) and the standard edgecommons sections `tags`, `hierarchy`, `identity`,
`messaging`, `metricEmission`, `logging`, `heartbeat`.

---

## 1. The shipped dev config (`test-configs/config.json`)

```jsonc
{
  "logging": { "level": "DEBUG" },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "log", "namespace": "edgecommons" },
  "tags": { "site": "factory-1" },
  "component": {
    "global": {
      "defaults": { "pollIntervalMs": 5000 },
      "timeouts": { "connectMs": 5000, "reconnectBackoffMinMs": 1000, "reconnectBackoffMaxMs": 60000 },
      "healthThresholds": { "staleSignalSecs": 30 }
    },
    "instances": [
      {
        "id": "device-1",
        "adapter": "sim",
        "connection": { "endpoint": "sim://device-1" },
        "pollIntervalMs": 5000,
        "writes": { "allow": [] }
      }
    ]
  }
}
```

**What each option does at runtime**

| Option | Effect |
|--------|--------|
| `hierarchy.levels` / `identity` | Places the component in the UNS enterprise tree (the envelope's `identity`); the last level (`device`) is always the resolved Thing name. |
| `component.global.healthThresholds.staleSignalSecs` | A signal with no update for longer than this counts toward `southbound_health.staleSignals`. |
| `instances[].id` | The `{instance}` token of this device's topics, and the `instance` field every `sb/*` command resolves against. |
| `instances[].adapter` | Which backend `backendFor()` resolves — `sim` is the only one this scaffold ships. |
| `instances[].connection.endpoint` | Passed to the backend's `connect()`. The simulator only requires it to be non-empty; a real protocol reads whatever else it needs from the (deliberately open) `connection` object. |
| `instances[].pollIntervalMs` | How often the device loop reads and publishes. |
| `instances[].writes.allow` | The per-instance write allow-list, checked before any device I/O. Empty ⇒ read-only. |

---

## 2. Two devices, one writable

```jsonc
{
  "component": {
    "global": { "defaults": { "pollIntervalMs": 5000 }, "healthThresholds": { "staleSignalSecs": 30 } },
    "instances": [
      { "id": "device-1", "adapter": "sim", "connection": { "endpoint": "sim://device-1" }, "pollIntervalMs": 5000, "writes": { "allow": [] } },
      { "id": "device-2", "adapter": "sim", "connection": { "endpoint": "sim://device-2" }, "pollIntervalMs": 2000, "writes": { "allow": ["temperature-1"] } }
    ]
  }
}
```

With two instances, every `sb/*` command needs a body `instance` field (`BAD_ARGS` if missing,
`NO_SUCH_INSTANCE` if unknown). `device-2` additionally accepts writes to `temperature-1` — the
allow-list is per instance, so `device-1` stays read-only regardless.

---

## 3. Publishing metrics onto the UNS (instead of a log file)

```jsonc
{ "metricEmission": { "target": "messaging" } }
```

With `target: messaging`, `southbound_health` and the two operational families publish on
`ecv1/{device}/{component}/metric/{metricName}` instead of a local log file — the default `log`
target writes `*.metric.log` locally, useful for a first local run without a broker.

---

## 4. Greengrass v2 deployment (IPC)

On Greengrass, config is the component's `ComponentConfig` and messaging uses IPC — no
`messaging` section, no broker. This is the `recipe.yaml`
`DefaultConfiguration.ComponentConfig` shape; override `connection`/`writes` per device at deploy
time.

```yaml
ComponentConfiguration:
  DefaultConfiguration:
    ComponentConfig:
      logging: { level: "INFO" }
      heartbeat: { intervalSecs: 5, measures: { cpu: true, memory: true } }
      metricEmission: { target: "log" }
      component:
        global:
          defaults: { pollIntervalMs: 5000 }
          healthThresholds: { staleSignalSecs: 30 }
        instances:
          - id: "device-1"
            adapter: "sim"
            connection: { endpoint: "sim://device-1" }
            pollIntervalMs: 5000
            writes: { allow: [] }
```

---

## 5. Kubernetes (ConfigMap)

`k8s/configmap.yaml` mounts the config as a directory; `CONFIGMAP` hot-reloads on `kubectl apply`.
With `--platform auto`, KUBERNETES is detected from the ServiceAccount token, identity resolves
from the Downward API, and no CLI args are needed:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: <<BINNAME>>-config
data:
  config.json: |-
    {
      "messaging": { "local": { "type": "mqtt", "host": "emqx.default.svc.cluster.local", "port": 1883 } },
      "metricEmission": { "target": "prometheus", "targetConfig": { "port": 9090, "path": "/metrics" } },
      "component": {
        "global": { "defaults": { "pollIntervalMs": 5000 }, "healthThresholds": { "staleSignalSecs": 30 } },
        "instances": [
          { "id": "device-1", "adapter": "sim", "connection": { "endpoint": "sim://device-1" }, "pollIntervalMs": 5000, "writes": { "allow": [] } }
        ]
      }
    }
```

`metricEmission.target: prometheus` exposes `southbound_health` and the operational families as
OpenMetrics text at `:9090/metrics` — the default metric target on KUBERNETES — instead of routing
them onto the bus.
