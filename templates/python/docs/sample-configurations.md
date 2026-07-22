# Sample Configurations

*This documents the generated scaffold; rewrite it as you build the component out.*

The scaffold ships two working configs in `test-configs/`, plus the MQTT `standalone-messaging.json`
for local HOST runs. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for the demonstrated surface see
[explanation.md](explanation.md).

## `test-configs/config_1.json` — a fuller dev config

```jsonc
{
  "logging": { "level": "INFO", "python_format": "..." },
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "site1" },
  "heartbeat": {
    "enabled": true, "intervalSecs": 1,
    "measures": { "cpu": true, "memory": true, "disk": false, "files": true, "threads": true, "fds": false },
    "destination": "local"
  },
  "metricEmission": {
    "target": "log", "namespace": "edgecommons",
    "targetConfig": { "logFileName": "{ComponentFullName}.metric.log" }
  },
  "tags": { "appId": "IntelliJIdea", "site": "site1", "shop": "shop1", "line": "line1" },
  "component": { "global": { "publish_interval": 2 }, "instances": [] }
}
```

**What each option does at runtime**

| Option | Effect |
|--------|--------|
| `hierarchy.levels` / `identity` | Places the component in the UNS enterprise tree. The last level (`device`) is always the resolved thing name, supplied at runtime by `-t`. |
| `heartbeat.intervalSecs` | How often the `state` keepalive publishes. `1` here is deliberately fast for local iteration; a real deployment usually runs slower (`5`, as in `recipe.yaml`). |
| `heartbeat.measures` | Which system measures ride the keepalive's `sys` metric — `cpu`/`memory` are common; `files`/`threads`/`fds` add process-resource visibility. |
| `metricEmission.target: log` | Routes `loopTicks` to a rotating log file instead of the bus. Switch to `messaging` to see it on `ecv1/+/+/metric/#` (see `config_2.json`, or edit this field directly). |
| `component.global.publish_interval` | Seconds between the scaffold's `app`/metric/data/event quartet. Lower = more frequent publishes. |
| `component.instances: []` | No instances declared — the scaffold runs fine with none; `instance_connectivity()` reports an empty list and `status` answers exactly as `ping`. |

## `test-configs/config_2.json` — the minimal form

```jsonc
{
  "logging": { "level": "INFO", "python_format": "..." },
  "component": { "global": { "publish_interval": 3 }, "instances": [] }
}
```

Everything the library needs beyond `component` has a sensible default: no `hierarchy` means a
single-level tree (`["device"]`), so topics are `ecv1/{thing}/<<BINNAME>>/...` with no enterprise
path prefix. This is the config to reach for when you just want to see the loop run.

## `test-configs/standalone-messaging.json` — the HOST/MQTT broker

```json
{ "messaging": { "local": { "host": "localhost", "port": 1883, "clientId": "<<BINNAME>>-local" } } }
```

Passed as the `--transport MQTT <path>` argument (not `-c`) — it names the broker the transport
connects to, independent of the `-c FILE ...` component config. Point `host`/`port` at your broker;
`clientId` should be unique per running instance so two processes don't fight over one MQTT session.

## Adding your own instance-scoped config

Once you add real `component.instances[]` entries, extend `config.schema.json`'s
`$defs/instance` with the keys you read (see [reference/configuration.md](reference/configuration.md)) —
keep `additionalProperties: false` so a typo'd key is caught at deploy time rather than silently
ignored.
