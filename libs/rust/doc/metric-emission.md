# Metric Emission

Metrics are defined once and then emitted by name. The [`MetricEmitter`] (the default
[`MetricService`]) routes each emission to the **effective** target — `explicit
metricEmission.target ▸ platform-profile default ▸ "log"` — formatting it as **EMF**
(CloudWatch Embedded Metric Format) for the push targets. On the **KUBERNETES** platform the
profile default is the pull-based [`prometheus`](#prometheus-pull-based-feature-metrics-prometheus)
target; on GREENGRASS/HOST the default stays `log` (precedence FR-MET-1 / FR-RT-3).

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
| `messaging` | Publish EMF in a `Metric` message envelope on the library-owned UNS metric topic `ecv1[/{site}]/{device}/{component}/main/metric/{metricName}` (via the reserved-publish seam; the legacy `targetConfig.topic` override is removed) | `targetConfig.destination`: `ipc`/`local` or `iotcore` |
| `cloudwatchcomponent` | Publish a `{request:{namespace,metricData}}` PutMetricData message **per measure** | Fixed topic `cloudwatch/metric/put` (external Greengrass component contract — unchanged by the UNS, D‑U21) |
| `cloudwatch` | Send to CloudWatch via the AWS SDK (`PutMetricData`) | Requires the `cloudwatch` cargo feature; batched on an interval. **Validated on-device.** |
| `prometheus` | **Pull-based**: maintain an in-process registry and serve it as OpenMetrics text at an HTTP `/metrics` endpoint | Requires the `metrics-prometheus` cargo feature; the **default on KUBERNETES**. See below. |

Selecting `cloudwatch` (or `prometheus`) without its feature, or a messaging target without
a messaging service, is a clear `GgError::Metrics` rather than a silent no-op.

### `prometheus` (pull-based, feature `metrics-prometheus`)

The `prometheus` target **inverts the metric lifecycle** (FR-MET-2). It does not push anywhere —
**Prometheus scrapes it**:

- `emit_metric` / `emit_metric_now` only **update the in-process registry** (latest-value gauges).
  They are identical (there is no batching to bypass) and send nothing over the network.
- `flush_metrics()` is a **delivery no-op** — a scrape pulls the current values.
- `shutdown()` (and `Drop`) **stops the HTTP listener** so no port/thread leaks.

This inversion is local to this target; `log`/`messaging`/`cloudwatch`/`cloudwatchcomponent` keep
their push semantics unchanged.

The server binds `0.0.0.0:<port>` (default `9090`) and serves `<path>` (default `/metrics`) with a
valid `Content-Type` (`text/plain; version=0.0.4`, from the client lib's `TextEncoder` — Prometheus
3.x rejects a blank type). It uses the community [`prometheus`](https://crates.io/crates/prometheus)
crate (there is no Prometheus-org official Rust client); the heavy `process`/`push` collectors are
not enabled.

**Rust feature gating of the KUBERNETES default.** The k8s profile default resolves to `prometheus`
**only when the `metrics-prometheus` feature is compiled in**. Without it, a k8s build gracefully
falls back to `log` (with a warning) so a feature-less build still runs; an **explicit**
`metricEmission.target = "prometheus"` without the feature is a clear `GgError::Metrics` error
(matching the `cloudwatch`-without-feature behavior).

**Dimension → label mapping (FR-MET-3, locked for four-way parity).** For each measure in an emitted
metric, one gauge is registered/updated:

- **gauge name** = `sanitize(lowercase("{namespace}_{measureName}"))` — replace every char not
  matching `[a-z0-9_]` with `_`, and prefix `_` if the result starts with a digit. `namespace`
  defaults to `ggcommons`.
- **labels** = the metric's dimensions (`category` = metric name, `coreName`, `component`, plus any
  custom dimensions). Each label **name** is sanitized to `[a-zA-Z_][a-zA-Z0-9_]*` (invalid chars →
  `_`, `_`-prefixed if it starts with a digit; case preserved, so `coreName` stays `coreName`); the
  label **value** is used as-is.
- The gauge is **set** to the measure's float value on each emit (latest-value semantics).
- The Greengrass `largeFleetWorkaround` (`coreName="ALL"` duplicate) is a CloudWatch-ism with no
  Prometheus analog and is **not** applied.

The label-name set for a given gauge name is fixed at first registration (the same constraint the
Java/Python/TS Prometheus clients impose). A later emit mapping to the same gauge name with a
*different* label-name set is dropped with a warning.

## `targetConfig` keys

| Key | Target(s) | Default |
|-----|-----------|---------|
| `logFileName` | `log` | platform-aware: `/greengrass/v2/logs/{ComponentFullName}.metric.log` on GREENGRASS; `./logs/{ComponentFullName}.metric.log` on HOST/KUBERNETES |
| `maxFileSize` | `log` | `10MB` |
| `destination` | `messaging` | `ipc` |
| `intervalSecs` | `cloudwatch` | `5` (min 1) |
| `port` | `prometheus` | `9090` |
| `path` | `prometheus` | `/metrics` |

> The `logFileName` default is **platform-aware**: HOST and KUBERNETES (which lack the Greengrass
> `/greengrass/v2/logs` directory) default to a local `./logs/...` path; an explicit `logFileName`
> always overrides it. The `log` target is fail-soft — if the file cannot be opened it warns and
> drops metrics rather than failing.

> The legacy `targetConfig.topic` override is **removed** (UNS hard cut — the drift knob is gone):
> the `messaging` target's topic is always the UNS-minted
> `ecv1[/{site}]/{device}/{component}/main/metric/{metricName}`, and `cloudwatchcomponent` always
> publishes to `cloudwatch/metric/put`.

String values support template substitution (`{ThingName}`, `{ComponentName}`,
`{ComponentFullName}`).

## EMF correctness

- `_aws.Timestamp` is in **milliseconds** since the Unix epoch, as required by the
  official CloudWatch Embedded Metric Format specification ("Values MUST be expressed
  as the number of milliseconds after Jan 1, 1970 00:00:00 UTC"). The Java target
  divides by 1000 (seconds), which deviates from the spec; Rust and Python follow it.
- The `cloudwatchcomponent` target's `metricData.timestamp` is in **seconds** — that
  is the Greengrass CloudWatch Metrics component's PutMetricData contract, which is
  distinct from EMF's `_aws.Timestamp`. Its `dimensions` array excludes `coreName`
  (the component supplies it implicitly).
- The ≤10-dimension cap is enforced on the `Metric` itself, not just the builder.

## Direct `cloudwatch` target on a Greengrass core

The direct `cloudwatch` target uses the AWS SDK's default credential chain. On a
Greengrass core that means the **Token Exchange Service** credentials, which requires:

1. The component recipe **must depend on `aws.greengrass.TokenExchangeService`** —
   that is what makes the Nucleus inject `AWS_CONTAINER_CREDENTIALS_FULL_URI` (plus
   `AWS_CONTAINER_AUTHORIZATION_TOKEN`) into the component's environment. Without the
   dependency the SDK finds no credentials.
2. The core's **token-exchange IAM role** must allow `cloudwatch:PutMetricData`.

Validated end-to-end on a live core: heartbeat measures (`cpu_usage`, `memory_usage`)
appeared in CloudWatch at the heartbeat cadence with no dropped batches.

## Live reconfiguration

The emitter is a config-change listener: on hot-reload it rebuilds the target from
the new config (keeping the previous one if the rebuild fails). Example: changing
`metricEmission.targetConfig.logFileName` redirects subsequent metrics to the new
file without a restart.
