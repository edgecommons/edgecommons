# Sample Configurations

> This documents the generated scaffold; rewrite it as you build the component out.

Ready-to-adapt configurations for `<<COMPONENTNAME>>`. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for topics/payloads see
[reference/messaging-interface.md](reference/messaging-interface.md).

> **How config reaches the component.** It reads one JSON document from the `-c/--config` source,
> defaulting by platform: `HOST` → `FILE`, `GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`.

## 1. Minimal local run (HOST + MQTT)

The shipped `test-configs/<<COMPONENTNAME>>.json` shape:

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "factory-1" },
  "messaging": { "local": { "host": "localhost", "port": 1883 } },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "destination": "local",
                 "measures": { "cpu": true, "memory": true } },
  "metricEmission": { "target": "log", "targetConfig": { "logFileName": "{ComponentFullName}.metric.log" } },
  "component": {
    "global": { "publish_interval": 3, "message": "Hello world" },
    "instances": []
  }
}
```

Run it:

```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT \
  -c FILE ./test-configs/<<COMPONENTNAME>>.json -t my-thing
```

| Option | Effect |
|--------|--------|
| `hierarchy` / `identity` | The enterprise hierarchy; the last level's value is always the resolved thing name. |
| `metricEmission.target` | Where the demo `loopTicks` metric goes — `log` writes a local file, `messaging` publishes it to the UNS `metric` class. |
| `component.publish_interval` | Seconds between the scaffold's publish tick (the demo metric/signal/event/status quartet). |
| `component.message` | The starting greeting; the `set-greeting` command overrides it at runtime. |

## 2. Publish the demo metric to the bus instead of a log file

```jsonc
"metricEmission": { "target": "messaging", "targetConfig": { "destination": "local" } }
```

Now `loopTicks` appears on `ecv1/{device}/<<BINNAME>>/metric/loopTicks` alongside the data signal and
events, so all three are visible on one broker subscription.

## 3. Greengrass v2 deployment (IPC)

On `--platform GREENGRASS` there is no messaging block and no config file; config comes from the
deployment's `ComponentConfiguration` (the same shape as `recipe.yaml`'s
`ComponentConfiguration.DefaultConfiguration.ComponentConfig`), and the transport is IPC.

```bash
java -jar <<JARNAME>>-1.0.0.jar --platform GREENGRASS -t my-thing
# package/publish: gdk component build && gdk component publish
```

## 4. Kubernetes (ConfigMap)

Config source defaults to `CONFIGMAP`: the whole ConfigMap is mounted as a directory
(`k8s/configmap.yaml`) so a `kubectl apply` hot-reloads config in place. Identity comes from the
Downward API, so typically no CLI args are needed.
