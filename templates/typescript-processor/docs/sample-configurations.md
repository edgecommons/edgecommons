This documents the generated scaffold; rewrite it as you build the component out.

# Sample Configurations

Complete configurations for `<<COMPONENTFULLNAME>>`, from the shipped demo route to a
multi-route variant. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for message shapes see
[reference/messaging-interface.md](reference/messaging-interface.md); for the model behind them,
[explanation.md](explanation.md).

The component loads **one JSON document** from `-c/--config`. The top level may contain
`component` (required) and the standard edgecommons sections `tags`, `hierarchy`, `identity`,
`messaging`, `metricEmission`, `logging`, `heartbeat`.

---

## 1. The shipped demo route (`test-configs/config.json`)

```jsonc
{
  "logging": { "level": "DEBUG" },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "log", "namespace": "edgecommons" },
  "tags": { "site": "factory-1" },
  "component": {
    "global": { "defaults": { "tickMs": 10000, "maxQueue": 256 } },
    "instances": [
      {
        "id": "rollup",
        "subscribe": ["ecv1/+/+/+/data/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/app/summary",
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
| `global.defaults.tickMs` / `maxQueue` | Fallbacks inherited by a route that omits its own `tickMs`/`maxQueue`. |
| `instances[].id` | Stable route id — appears in logs and metrics. |
| `instances[].subscribe` | Topic filters this route consumes. Wildcards allowed. |
| `instances[].publishTopic` | Where the result publishes. Config-template-resolved: `{ThingName}`/`{ComponentName}`/a hierarchy level/a tag may be interpolated. |
| `instances[].target` | `local` (device-local bus) or `northbound` (straight to the northbound broker). |
| `instances[].pipeline` | The stages, in order. `fieldEquals` here keeps only `temperature-1` readings; `countPerTick` accumulates and rolls up on the tick. |
| `instances[].tickMs` | How often the stateful stage (`countPerTick`) emits. |

---

## 2. Two routes: a rollup and a northbound relay

```jsonc
{
  "component": {
    "global": { "defaults": { "tickMs": 10000, "maxQueue": 256 } },
    "instances": [
      {
        "id": "rollup",
        "subscribe": ["ecv1/+/+/+/data/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/app/summary",
        "pipeline": [ { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } }, { "countPerTick": {} } ],
        "tickMs": 10000
      },
      {
        "id": "critical-relay",
        "subscribe": ["ecv1/+/+/+/evt/critical/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/critical-relay/app/relay",
        "target": "northbound",
        "pipeline": []
      }
    ]
  }
}
```

`critical-relay` has an **empty pipeline** — a pass-through republisher that relies on nothing but
the self-echo guard and the identity restamp, forwarding every critical event straight to the
northbound broker so an operator doesn't have to poll the device-local bus for alarms.

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
          defaults: { tickMs: 10000, maxQueue: 256 }
        instances:
          - id: "rollup"
            subscribe: ["ecv1/+/+/+/data/#"]
            publishTopic: "ecv1/{ThingName}/<<BINNAME>>/rollup/app/summary"
            pipeline:
              - fieldEquals: { path: "signal.id", value: "temperature-1" }
              - countPerTick: {}
```

`{ThingName}` resolves per-device at deploy time, so the same deployment configures every device in
the fleet identically while each publishes under its own resolved topic.

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
        "global": { "defaults": { "tickMs": 10000, "maxQueue": 256 } },
        "instances": [
          { "id": "rollup", "subscribe": ["ecv1/+/+/+/data/#"],
            "publishTopic": "ecv1/{ThingName}/<<BINNAME>>/rollup/app/summary",
            "pipeline": [ { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } }, { "countPerTick": {} } ] }
        ]
      }
    }
```
