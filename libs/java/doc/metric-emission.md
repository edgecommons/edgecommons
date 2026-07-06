# Metric Emission Configuration Guide

This document provides detailed information about the metric emission targets and their configuration options in the EdgeCommons Java library.

The metric emission system supports multiple targets for sending metrics, each with specific behaviors and configuration options. Metrics are formatted using the Embedded Metric Format (EMF) for CloudWatch compatibility.

## Metric Emission Configuration

### Metric Structure

Metrics in the system consist of:
- **Name**: Unique identifier for the metric
- **Namespace**: Logical grouping for metrics (defaults to "edgecommons")
- **Measures**: Named measurements with values, units, and storage resolution
- **Dimensions**: Key-value pairs for metric categorization (automatically includes component and thing names)

### Common Configuration Options

- **`target`**: Specifies which metric emission target to use. Valid values:
  - `"cloudwatch"` - Direct CloudWatch metrics emission with batching
  - `"cloudwatchcomponent"` - CloudWatch metrics via Greengrass component
  - `"log"` - Local file logging with rotation support
  - `"messaging"` - Message-based metrics via IPC or IoT Core
  - `"prometheus"` - **Pull-based** in-process registry exposed as OpenMetrics text at an HTTP
    `/metrics` endpoint (the **default on the KUBERNETES platform**; see "Prometheus Target" below)

  **Target precedence (FR-RT-3).** `target` is no longer strictly required. When it is omitted, the
  effective target is resolved as: **explicit `metricEmission.target`** ▸ **platform-profile default**
  (`prometheus` on the KUBERNETES platform; nothing on GREENGRASS/HOST) ▸ **library default `"log"`**.
  So a KUBERNETES pod with no `metricEmission.target` set gets the prometheus target automatically,
  while GREENGRASS and HOST keep defaulting to `log`. An explicit `target` always wins.
- **`namespace`**: The namespace for your metrics (Default: "edgecommons")
- **`largeFleetWorkaround`**: Boolean flag that creates aggregate metrics by replacing "coreName" dimension with "ALL" (Default: false)

### Template Variables

The system supports template variables in configuration strings that are automatically replaced:
- `{ComponentName}`: Name of the Greengrass component
- `{ComponentFullName}`: Full name including version of the component
- `{ThingName}`: Name of the IoT thing

Templates can be used in:
- Log file names
- Topic names
- Namespaces

### Available Targets

#### 1. CloudWatch Target (`"cloudwatch"`)
Sends metrics directly to Amazon CloudWatch using the AWS SDK with batching for efficiency.

**Implementation details:**
- Uses CloudWatchClient for direct AWS API access
- Maintains separate concurrent queues for each metric namespace
- Batches metrics and sends them on a configurable interval using a Timer
- Supports immediate emission via `emitMetricNow()` method
- Handles AWS API limits through batching (max 20 metrics per request)
- Preserves metric timestamps from when they were created

**Configuration options:**
- **`intervalSecs`**: Batching interval for CloudWatch API calls (Default: 5 seconds, minimum: 1 second)

**Example:**
```json
{
  "metricEmission": {
    "target": "cloudwatch",
    "namespace": "MyApp/Metrics",
    "targetConfig": {
      "intervalSecs": 60
    },
    "largeFleetWorkaround": false
  }
}
```

#### 2. CloudWatch Component Target (`"cloudwatchcomponent"`)
Sends metrics to CloudWatch through the Greengrass CloudWatch Metrics component via IPC.

**Implementation details:**
- Uses MessagingClient for IPC communication with CloudWatch component
- Topic: `cloudwatch/metric/put` — the external AWS Greengrass component contract (fixed; the
  former `targetConfig.topic` override was removed with the UNS hard cut, D-U21)
- Sends each measure as a separate message (no batching)
- Does not support `largeFleetWorkaround` due to component limitations
- Lighter weight than direct CloudWatch target

**Example:**
```json
{
  "metricEmission": {
    "target": "cloudwatchcomponent",
    "namespace": "MyComponent/Metrics"
  }
}
```

#### 3. Log Target (`"log"`)
Writes metrics to a local log file using Log4j2 with EMF format and file rotation support.

**Implementation details:**
- Uses Log4j2 RollingFileAppender for file logging with size-based rotation
- Metrics are written in EMF (Embedded Metric Format) for CloudWatch compatibility
- Supports template-based file naming with variable substitution
- Immediate metric writing (no batching)
- Thread-safe logging implementation
- Automatic file rotation when size limit is reached
- Keeps up to 5 historical files (`.1`, `.2`, etc.)

**Configuration options:**
- **`logFileName`**: Template for log file naming. The default is **platform-aware**:
  `/greengrass/v2/logs/{ComponentFullName}.metric.log` on `GREENGRASS`, and a local
  `./logs/{ComponentFullName}.metric.log` on `HOST`/`KUBERNETES` (which lack the Greengrass logs
  directory). An explicit value always overrides the default. The target is fail-soft: if the file
  cannot be opened it logs a warning and drops file metrics rather than failing startup.
- **`maxFileSize`**: Maximum file size before rotation (Default: "10MB")

**Template variables supported:**
- `{ComponentFullName}`: Full component name including version
- `{ComponentName}`: Component name only
- `{ThingName}`: IoT Thing name

**Example:**
```json
{
  "metricEmission": {
    "target": "log",
    "namespace": "MyApp/Metrics",
    "targetConfig": {
      "logFileName": "/custom/path/{ComponentName}.metrics.log",
      "maxFileSize": "50MB"
    }
  }
}
```

#### 4. Messaging Target (`"messaging"`)
Publishes metrics through the messaging system in EMF format, supporting both local IPC and IoT Core destinations.

**Implementation details:**
- Uses MessagingClient for both local/IPC and IoT Core publishing
- Topic: the library-owned **UNS metric topic**
  `ecv1/{device}/{component}/main/metric/{metricName}` (rooted form when `topic.includeRoot` is
  true; the metric name is sanitized as a channel token). The former `targetConfig.topic` override
  was removed with the UNS hard cut — the `metric` class is reserved and published through the
  library-internal `ReservedPublisher` seam (see [messaging.md](messaging.md)).
- Uses QoS AT_LEAST_ONCE for IoT Core publishing
- Immediate message publishing (no batching)
- Messages formatted as EMF with Message wrapper including version and metadata

**Configuration options:**
- **`destination`**: Specify message destination (Default: "ipc")
  - `"ipc"` / `"local"`: local bus (Greengrass IPC or the local MQTT broker)
  - `"iotcore"` / `"iot_core"`: publish to AWS IoT Core

**Example:**
```json
{
  "metricEmission": {
    "target": "messaging",
    "namespace": "MyApp/Metrics",
    "targetConfig": {
      "destination": "iotcore"
    }
  }
}
```

#### 5. Prometheus Target (`"prometheus"`)
A **pull-based** target — the default on the KUBERNETES platform. It maintains an in-process metric
registry and serves it as OpenMetrics/Prometheus text over HTTP at `path` (default `/metrics`) on
`port` (default `9090`), bound on `0.0.0.0`. Backed by the official `io.prometheus:simpleclient`
client (bundled in the shaded JAR); the exposition is written by the client's `TextFormat` writer,
which sets a valid `Content-Type` (Prometheus 3.x rejects a blank type).

**Inverted lifecycle (FR-MET-2) — important.** Unlike every other target (log/messaging/cloudwatch/
cloudwatchcomponent), which *push* on each emit, the prometheus target *inverts* the lifecycle:

- `emitMetric` **and** `emitMetricNow` only **update the registry** — they do **not** push anywhere.
- `flushMetrics()` is a **no-op** w.r.t. delivery — a Prometheus scrape *pulls* the current values.
- `close()` (via `EdgeCommons.shutdown()` / SIGTERM) **stops the HTTP listener**, releasing the port
  and its daemon thread.

> **Caveat:** a component relying on `emitMetricNow`/`flushMetrics` to flush-before-exit gets **nothing
> delivered** under the prometheus target until the next scrape. The push targets are unchanged.

**Dimension → label mapping (FR-MET-3, identical across all four languages).** For each measure in an
emitted metric a `Gauge` is registered/updated with **latest-value** semantics (a scrape reads the
current value):

- **Gauge name** = `sanitize(lowercase("{namespace}_{measureName}"))`, where `namespace` defaults to
  `edgecommons`. Sanitization replaces every character not matching `[a-z0-9_]` with `_`, and prefixes
  `_` if the result starts with a digit (Prometheus metric-name rules).
- **Labels** = the metric's dimensions (which already include `category` (= metric name), `coreName`
  (= thing name), `component` (= component name), plus any custom dimensions). Each label **name** is
  sanitized to `[a-zA-Z_][a-zA-Z0-9_]*` (invalid chars → `_`, prefix `_` if it starts with a digit;
  **case is preserved**). The label **value** is used as-is.
- The gauge for that label-set is **set** to the measure's float value on each emit.

A gauge's label-name set is fixed at first registration; if the same gauge name is later emitted with a
different label-name set (the same measure carrying different dimensions), that emit is logged and
skipped rather than throwing.

**Configuration options:**
- **`port`**: HTTP port for the `/metrics` endpoint (Default: `9090`, range 1–65535)
- **`path`**: HTTP path for the OpenMetrics exposition (Default: `/metrics`)

**Example:**
```json
{
  "metricEmission": {
    "target": "prometheus",
    "namespace": "MyApp",
    "targetConfig": {
      "port": 9090,
      "path": "/metrics"
    }
  }
}
```

Scrape wiring (a Prometheus Operator `ServiceMonitor`/`PodMonitor`) is a deployment concern handled by
the Helm chart, not library config. On a feature-less Rust build the KUBERNETES default falls back to
`log` (the `metrics-prometheus` cargo feature is required for the Rust target); Java/Python/TS always
ship the client, so their KUBERNETES default is unconditionally `prometheus`.

## Configuration Examples

### Basic Configuration
```json
{
  "metricEmission": {
    "target": "log",
    "namespace": "MyApplication"
  }
}
```

### CloudWatch with Custom Batching
```json
{
  "metricEmission": {
    "target": "cloudwatch",
    "namespace": "Production/MyApp",
    "targetConfig": {
      "intervalSecs": 30
    },
    "largeFleetWorkaround": true
  }
}
```

### Log with File Rotation
```json
{
  "metricEmission": {
    "target": "log",
    "namespace": "Development/Debug",
    "targetConfig": {
      "logFileName": "/var/log/{ComponentName}-metrics.log",
      "maxFileSize": "100MB"
    }
  }
}
```

### Messaging to IoT Core
```json
{
  "metricEmission": {
    "target": "messaging",
    "namespace": "Telemetry/Sensors",
    "targetConfig": {
      "destination": "iotcore"
    }
  }
}
```
Metrics publish to `ecv1/{device}/{component}/main/metric/{metricName}` on IoT Core.

## EMF (Embedded Metric Format)

All targets use the EMF format for CloudWatch compatibility. The EMF structure includes:
- Timestamp in Unix epoch seconds
- CloudWatch metadata with namespace, dimensions, and metric definitions
- Dimension values as top-level properties
- Measure values as top-level properties
- AWS-specific metadata in `_aws` object

## Performance Considerations

### Target-Specific Behavior

**CloudWatch Direct:**
- Batches metrics by namespace using Timer-based emission
- Each namespace maintains a ConcurrentLinkedQueue
- Sends up to 20 metrics per API call (AWS limit)
- Memory usage scales with the batching interval and metric volume

**CloudWatch Component:**
- Immediate transmission, no batching
- Lower memory footprint
- Relies on component's rate limiting
- Cannot use `largeFleetWorkaround`

**Log Target:**
- Immediate writing with file rotation
- Performance limited by disk I/O
- File rotation prevents unlimited disk usage
- Thread-safe through Log4j2

**Messaging:**
- Immediate transmission
- IPC: Local processing limits
- IoT Core: Subject to IoT Core throttling

### Best Practices

1. **Target Selection:**
   - Use of `cloudwatchcomponent` is no longer recommended due to component limitations
   - Use `cloudwatch` for low volume emission or if custom batching intervals are needed
   - Use `log` for local debugging and development, and in conjunction with the [Greengrass LogManager component](https://docs.aws.amazon.com/greengrass/v2/developerguide/log-manager-component.html) for efficient log uploads to CloudWatch
   - Use `messaging` for real-time monitoring or custom processing

2. **Configuration:**
   - Set `intervalSecs` based on metric volume and latency requirements
   - Use meaningful namespaces for metric organization
   - Configure appropriate file rotation sizes for log target
   - Enable `largeFleetWorkaround` for large deployments to reduce CloudWatch costs

3. **Monitoring:**
   - Monitor memory usage with high-volume metrics
   - Watch for CloudWatch API throttling
   - Check disk space when using log target
   - Verify IAM permissions for CloudWatch targets