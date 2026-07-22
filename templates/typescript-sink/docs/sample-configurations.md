This documents the generated scaffold; rewrite it as you build the component out.

# Sample Configurations

Complete configurations for `<<COMPONENTFULLNAME>>`, from the shipped local-filesystem sink to a
multi-sink variant. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for message shapes see
[reference/messaging-interface.md](reference/messaging-interface.md); for the model behind them,
[explanation.md](explanation.md).

The component loads **one JSON document** from `-c/--config`. The top level may contain
`component` (required) and the standard edgecommons sections `tags`, `hierarchy`, `identity`,
`messaging`, `metricEmission`, `logging`, `heartbeat`.

---

## 1. The shipped local sink (`test-configs/config.json`)

```jsonc
{
  "logging": { "level": "DEBUG" },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "log", "namespace": "edgecommons" },
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
| `instances[].id` | Stable sink id — appears in logs, metrics context, and prefixes every delivered key (`keyFor`). |
| `instances[].subscribe` | The single topic filter whose messages this sink delivers. |
| `instances[].destination` | Where they go — a tagged object (`type` + backend-specific fields). |
| `instances[].retry.baseDelayMs` / `maxDelayMs` | The backoff curve's start and ceiling. |
| `instances[].retry.giveUpAfterMs` | The time budget before a delivery is reported `delivery-exhausted`. |
| `instances[].maxQueue` | How many items may queue for this sink before new ones are dropped and counted. |

---

## 2. Two sinks with different retry budgets

```jsonc
{
  "component": {
    "global": { "defaults": { "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 }, "maxQueue": 256 } },
    "instances": [
      { "id": "archive", "subscribe": "ecv1/+/+/+/data/#", "destination": { "type": "local", "path": "./out" } },
      { "id": "alarms", "subscribe": "ecv1/+/+/+/evt/critical/#", "destination": { "type": "local", "path": "./alarms" },
        "retry": { "baseDelayMs": 500, "maxDelayMs": 60000, "giveUpAfterMs": 600000 } }
    ]
  }
}
```

`alarms` gives up after 10 minutes instead of an hour — a critical-event sink that can't reach its
destination is something you want to know about quickly, not after the default hour-long budget.

---

## 3. Greengrass v2 deployment (IPC)

```yaml
ComponentConfiguration:
  DefaultConfiguration:
    ComponentConfig:
      logging: { level: "INFO" }
      heartbeat: { intervalSecs: 5, measures: { cpu: true, memory: true } }
      metricEmission: { target: "log" }
      component:
        global:
          defaults: { retry: { baseDelayMs: 1000, giveUpAfterMs: 3600000 }, maxQueue: 256 }
        instances:
          - id: "archive"
            subscribe: "ecv1/+/+/+/data/#"
            destination: { type: "local", path: "/greengrass/v2/work/<<BINNAME>>/out" }
```

---

## 4. Kubernetes (ConfigMap)

`k8s/configmap.yaml` mounts the config as a directory; `CONFIGMAP` hot-reloads on `kubectl apply`.
With `--platform auto`, KUBERNETES is detected from the ServiceAccount token and identity resolves
from the Downward API:

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
        "global": { "defaults": { "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 }, "maxQueue": 256 } },
        "instances": [
          { "id": "archive", "subscribe": "ecv1/+/+/+/data/#", "destination": { "type": "local", "path": "/data/out" } }
        ]
      }
    }
```

A local-path destination on Kubernetes needs a mounted volume backing `path` — the shipped
`k8s/deployment.yaml` does not mount one by default; add a `PersistentVolumeClaim` before pointing a
real deployment at `local`, or replace the destination with a networked backend.
