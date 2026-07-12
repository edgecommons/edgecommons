# Configuration

Configuration is loaded from a source selected by `-c/--config`, validated against an
embedded JSON schema, deserialized into a typed [`Config`] snapshot, and published
through `arc_swap::ArcSwap` so readers always see a consistent, atomically-swapped
view.

## Schema

The schema (`resources/edgecommons-config-schema.json`, embedded with `include_str!`)
matches the Java/Python/TypeScript schema. Known top-level sections are strict
(`additionalProperties: false`); component-specific keys pass through only in
`component.global` and `component.instances[]`.

```
logging:        { level, rust_format, fileLogging: { enabled, filePath }, loggers, globalControl, publish }
metricEmission: { target, namespace, largeFleetWorkaround, targetConfig: {...} }
heartbeat:      { enabled, intervalSecs, measures: { cpu, memory, disk, threads, files, fds }, destination }
hierarchy:      { levels: [ "site", ..., "device" ] }    # UNS hierarchy — last level = the node (thing name)
identity:       { <level>: <value>, ... }                # values for every level except the last
topic:          { includeRoot }                          # UNS topic-building options
messaging:      { local{qos}, northbound{qos}, requestTimeoutSeconds }
tags:           { <key>: <value>, ... }
component:      { global: {...}, instances: [ { id, ... }, ... ] }
```

`logging.publish` is the optional structured log-bus publisher. When enabled, Rust publishes
`edgecommons.log.v1` records to `ecv1/{device}/{component}/main/log/{level}` through the reserved `log`
class seam. It is disabled by default; native capture observes `tracing` events.

Validation fails **closed**: an invalid document is a hard error at startup, and an
invalid hot-reloaded document is rejected with the previous snapshot kept.

## Pre-commit component validation

Register synchronous application validators on the builder. They run after schema validation for
both `Initial` and `Reload`, before the one atomic snapshot swap. Each callback receives owned copies
of the candidate and redacted prior snapshot; rejection, failure, or deadline expiry preserves the
exact prior generation and skips applied-config listeners.

```rust
use edgecommons::prelude::*;
use std::time::Duration;

let builder = EdgeCommonsBuilder::new("com.example.Camera")
    .initial_ready(false)
    .configuration_validator("camera", |candidate, redacted_current, phase| {
        let _ = (candidate, redacted_current, phase);
        Ok(ConfigurationValidationResult::accept())
    })?
    .configuration_validation_timeout(Duration::from_secs(5))?;
```

The timeout is one overall generation deadline (default 5 seconds, maximum 60). At most four
validator callbacks run process-wide. A callback that ignores timeout retains its worker permit
until it exits, preventing repeated reloads from accumulating threads. Inspect
`gg.config_generation()` and `gg.last_candidate_validation_errors()` for lifecycle diagnostics.

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
library schema-validates and runs candidate validators, builds a fresh `Config`, swaps it atomically,
and only then notifies registered listeners:

```rust
use edgecommons::config::ConfigurationChangeListener;
use std::sync::Arc;

gg.add_config_change_listener(my_listener); // Arc<dyn ConfigurationChangeListener>
```

Internally the metric target and the logging level reconfigure themselves on reload
via the same listener mechanism; the heartbeat reads the live snapshot each tick.
A misbehaving listener cannot abort the others.

### Transactional runtime reload

If a component-owned runtime must transition with the configuration generation, install exactly
one `ConfigurationApplyListener` coordinator with `add_config_apply_listener`. Its
`prepare_configuration_apply` method receives the candidate snapshot and returns a
`PreparedConfigurationApply` transaction. Preparation must not change the live runtime.

Core serializes the lifecycle, invokes `commit` while the prior Core snapshot remains active, and
stores the candidate snapshot only after `commit` succeeds. If commit reports an error, Core
fully awaits `rollback` and retains the prior snapshot and generation. `commit` and `rollback`
must use their own bounded stages and return only after the component runtime has either changed
successfully or been restored; Core intentionally does not cancel a destructive transition
midway.

## Template substitution

`config::template::resolve` substitutes `{ThingName}`, `{ComponentName}`,
`{ComponentFullName}`, and tag keys into string values (used for log file paths and
topics). Missing values are handled explicitly rather than panicking.
