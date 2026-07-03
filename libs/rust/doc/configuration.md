# Configuration

Configuration is loaded from a source selected by `-c/--config`, validated against an
embedded JSON schema, deserialized into a typed [`Config`] snapshot, and published
through `arc_swap::ArcSwap` so readers always see a consistent, atomically-swapped
view.

## Schema

The schema (`resources/ggcommons-config-schema.json`, embedded with `include_str!`)
matches the Java/Python schema. All sections are optional and `additionalProperties`
is permissive, so component-specific keys pass through untouched.

```
logging:        { level, format, fileLogging: { enabled, filePath }, loggers, globalControl }
metricEmission: { target, namespace, largeFleetWorkaround, targetConfig: {...} }
heartbeat:      { enabled, intervalSecs, measures: { cpu, memory, disk, threads, files, fds }, destination }
hierarchy:      { levels: [ "site", ..., "device" ] }    # UNS hierarchy — last level = the node (thing name)
identity:       { <level>: <value>, ... }                # values for every level except the last
topic:          { includeRoot }                          # UNS topic-building options
messaging:      { local, iotCore, requestTimeoutSeconds, lwt }
tags:           { <key>: <value>, ... }
component:      { global: {...}, instances: [ { id, ... }, ... ] }
```

Validation fails **closed**: an invalid document is a hard error at startup, and an
invalid hot-reloaded document is rejected with the previous snapshot kept.

## Reading config

```rust
let cfg = gg.config();                 // Arc<Config>
let level = &cfg.thing_name;           // resolved identity
let interval = cfg.global().get("publish_interval");  // component.global subtree
```

`Config` exposes:

- `thing_name`, `component_name` — resolved identity.
- `parsed` — typed view of the known sections (`logging`, `heartbeat`,
  `metric_emission`, `tags`, `component`).
- `raw` — the original JSON document, retained for template substitution over
  arbitrary keys.
- `global()` — the `component.global` subtree.
- `instance_ids()` / `instance(id)` — multi-instance access (see below).

## Multi-instance components

A component can declare multiple instances under `component.instances`, each with an
`id` and instance-specific keys:

```json
{
  "component": {
    "global": { "publish_interval": 3 },
    "instances": [
      { "id": "lineA", "sensor": "/dev/ttyUSB0" },
      { "id": "lineB", "sensor": "/dev/ttyUSB1" }
    ]
  }
}
```

Iterate instances and read per-instance config:

```rust
for id in cfg.instance_ids() {
    if let Some(inst) = cfg.instance(&id) {
        let sensor = inst.get("sensor").and_then(|v| v.as_str());
        // spawn per-instance work...
    }
}
```

## Hot reload

The `FILE` source watches the file (via `notify`) and emits updates. On change the
library validates the new document, builds a fresh `Config`, swaps it atomically, and
then notifies registered listeners:

```rust
use ggcommons::config::ConfigurationChangeListener;
use std::sync::Arc;

gg.add_config_change_listener(my_listener); // Arc<dyn ConfigurationChangeListener>
```

Internally the metric target and the logging level reconfigure themselves on reload
via the same listener mechanism; the heartbeat reads the live snapshot each tick.
A misbehaving listener cannot abort the others.

## Template substitution

`config::template::resolve` substitutes `{ThingName}`, `{ComponentName}`,
`{ComponentFullName}`, and tag keys into string values (used for log file paths and
topics). Missing values are handled explicitly rather than panicking.
