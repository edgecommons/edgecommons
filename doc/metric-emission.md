# Metric Emission

Metrics are defined once and then emitted by name. The [`MetricEmitter`] (the default
[`MetricService`]) routes each emission to the target selected by
`metricEmission.target`, formatting it as **EMF** (CloudWatch Embedded Metric Format).

Obtain the service from the runtime: `let metrics = gg.metrics();`

## Defining and emitting

```rust
use ggcommons::metrics::MetricBuilder;
use std::collections::HashMap;

metrics.define_metric(
    MetricBuilder::create("requests")
        .add_measure("count", "Count", 60)   // name, unit, storageResolution (secs)
        .build(),
);

let mut values = HashMap::new();
values.insert("count".to_string(), 1.0);
metrics.emit_metric("requests", values).await?;       // buffered where the target batches
metrics.emit_metric_now("requests", values2).await?;  // immediate, bypasses batching
```

- `define_metric` / `is_metric_defined` are pure registry operations (no side
  effects — `is_metric_defined` never emits, fixing the Java H6 bug).
- Emitting an undefined metric logs a warning and is ignored (not an error).
- `flush_metrics()` flushes buffered metrics; `shutdown()` does a final flush.

## Targets

Selected by `metricEmission.target` (default `log`):

| Target | Behavior | Notes |
|--------|----------|-------|
| `log` | Append EMF JSON lines to a file | Size-based rotation (5 backups); `largeFleetWorkaround` double-emits (normal + `coreName=ALL`) |
| `messaging` | Publish EMF over the messaging service | `targetConfig.destination`: `ipc`/`local` or `iotcore` |
| `cloudwatchcomponent` | Publish to the Greengrass CloudWatch component topic | Default topic `cloudwatch/metric/put` |
| `cloudwatch` | Send to CloudWatch via the AWS SDK | Requires the `cloudwatch` cargo feature; batched on an interval |

Selecting `cloudwatch` without the feature, or a messaging target without a messaging
service, is a clear `GgError::Metrics` rather than a silent no-op.

## `targetConfig` keys

| Key | Target(s) | Default |
|-----|-----------|---------|
| `logFileName` | `log` | `/greengrass/v2/logs/{ComponentFullName}.metric.log` |
| `maxFileSize` | `log` | `10MB` |
| `topic` | `messaging`, `cloudwatchcomponent` | `{ThingName}/{ComponentName}/metric` (or `cloudwatch/metric/put`) |
| `destination` | `messaging` | `ipc` |
| `intervalSecs` | `cloudwatch` | `5` (min 1) |

String values support template substitution (`{ThingName}`, `{ComponentName}`,
`{ComponentFullName}`).

## EMF correctness

- `_aws.Timestamp` is in **milliseconds** (fixing the Java H5 seconds bug).
- The ≤10-dimension cap is enforced on the `Metric` itself, not just the builder.

## Live reconfiguration

The emitter is a config-change listener: on hot-reload it rebuilds the target from
the new config (keeping the previous one if the rebuild fails). Example: changing
`metricEmission.targetConfig.logFileName` redirects subsequent metrics to the new
file without a restart.
