This documents the generated scaffold; rewrite it as you build the component out.

# Sample Configurations

Complete configurations for `<<COMPONENTFULLNAME>>`, from the shipped dev config to a
multi-level UNS hierarchy. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for message shapes see
[reference/messaging-interface.md](reference/messaging-interface.md); for the model behind them,
[explanation.md](explanation.md).

The component loads **one JSON document** from `-c/--config`. The top level may contain
`component` and the standard edgecommons sections `tags`, `hierarchy`, `identity`, `messaging`,
`metricEmission`, `logging`, `heartbeat`.

---

## 1. The shipped dev config (`test-configs/config.json`)

```jsonc
{
  "logging": { "level": "DEBUG" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "log", "namespace": "edgecommons" },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
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
| `hierarchy.levels` / `identity` | Places the component in the UNS enterprise tree; the last level (`device`) is always the resolved Thing name. |
| `component.global.publish_interval` | Seconds between the scaffold's own publish tick — not read by the demo code today (`TICK_INTERVAL_MS` in `src/app.ts` is a fixed constant); wire it up as you build out real logic. |
| `component.instances[].id` | The scaffold declares a single instance, `main` — the one the demo facades are bound to. |
| `metricEmission.target: log` | Routes `loopTicks` to a rotating local log file instead of the bus — the default so a first local run needs no broker for metrics specifically (data/evt still need one). |

---

## 2. Publishing metrics onto the UNS (instead of a log file)

```jsonc
{ "metricEmission": { "target": "messaging" } }
```

With `target: messaging`, `loopTicks` publishes on `ecv1/{device}/{component}/metric/loopTicks`
instead of a log file.

---

## 3. A deeper UNS hierarchy

```jsonc
{
  "hierarchy": { "levels": ["site", "area", "line", "device"] },
  "identity": { "site": "plant1", "area": "pumphouse", "line": "5" }
}
```

The resolved Thing name is still the last level (`device`); every topic's `identity.path` becomes
`plant1/pumphouse/5/{device}` — a fleet consumer still subscribes the same six wildcards
regardless of how deep the hierarchy is.

---

## 4. Greengrass v2 deployment (IPC)

```yaml
ComponentConfiguration:
  DefaultConfiguration:
    ComponentConfig:
      logging: { level: "INFO" }
      heartbeat: { intervalSecs: 5, measures: { cpu: true, memory: true } }
      metricEmission: { target: "log" }
      component:
        global: { publish_interval: 3 }
        instances: [ { id: "main" } ]
```

---

## 5. Kubernetes (ConfigMap)

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
      "component": { "global": { "publish_interval": 3 }, "instances": [ { "id": "main" } ] }
    }
```
