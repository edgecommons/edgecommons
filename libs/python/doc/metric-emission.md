# Metric Emission Configuration Guide

This document provides detailed information about the metric emission targets and their configuration options in the EdgeCommons Python library.

The metric emission system supports multiple targets for sending metrics, each with specific behaviors and configuration options. Metrics are formatted using the Embedded Metric Format (EMF) for CloudWatch compatibility.

## Metric Emission Configuration

### Metric Structure

Metrics in the system consist of:
- **Name**: Unique identifier for the metric
- **Namespace**: Logical grouping for metrics (defaults to "edgecommons")
- **Measures**: Named measurements with values, units, and storage resolution
- **Dimensions**: Key-value pairs for metric categorization (automatically includes component and thing names)

### Common Configuration Options

- **`target`**: Specifies which metric emission target to use. When omitted, the effective target
  follows the precedence *explicit `target` ▸ platform-profile default (`prometheus` on KUBERNETES) ▸
  library default `log`* (FR-MET-4 / FR-RT-3). Valid values:
  - `"cloudwatch"` - Direct CloudWatch metrics emission with batching
  - `"log"` - Local file logging with rotation support (the library default)
  - `"messaging"` - Message-based metrics via IPC or IoT Core
  - `"cloudwatchcomponent"` - Hand off to a CloudWatch publisher component over messaging
  - `"prometheus"` - **Pull-based** in-process registry served as OpenMetrics/Prometheus text over
    HTTP (the default on the KUBERNETES platform). See the dedicated section below.
- **`namespace`**: The namespace for your metrics (Default: "edgecommons")

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
- Uses boto3 CloudWatch client for direct AWS API access
- Maintains queues for batching metrics by namespace
- Batches metrics and sends them on a configurable interval
- Supports immediate emission via `emit_metric_now()` method
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
    }
  }
}
```

#### 2. Log Target (`"log"`)
Writes metrics to a local log file using Python logging with EMF format and file rotation support.

**Implementation details:**
- Uses Python's RotatingFileHandler for file logging with size-based rotation
- Metrics are written in EMF (Embedded Metric Format) for CloudWatch compatibility
- Supports template-based file naming with variable substitution
- Immediate metric writing (no batching)
- Thread-safe logging implementation
- Automatic file rotation when size limit is reached
- Keeps up to 5 historical files (`.1`, `.2`, etc.)

**Configuration options:**
- **`logFileName`**: Template for log file naming. The default is **platform-aware**:
  `/greengrass/v2/logs/{ComponentFullName}_metric.log` on `GREENGRASS`, and a local
  `./logs/{ComponentFullName}_metric.log` on `HOST`/`KUBERNETES` (which lack the Greengrass logs
  directory). An explicit value always overrides the default. The target is **fail-soft**: if the
  file cannot be opened it logs a warning and drops file metrics rather than aborting initialization
  (parity with Java/Rust/TypeScript).
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

#### 3. Messaging Target (`"messaging"`)
Publishes metrics through the messaging system in EMF format, supporting both local IPC and IoT Core destinations.

**Implementation details:**
- Uses MessagingClient for both local/IPC and IoT Core publishing
- Topic: the library-owned **UNS metric topic**
  `ecv1/{device}/{component}/main/metric/{metricName}` (rooted form when `topic.includeRoot` is
  true; the metric name is sanitized as a channel token). The former `targetConfig.topic` override
  was removed with the UNS hard cut — the `metric` class is reserved and published through the
  library-internal `MessagingClient._publish_reserved*` seam (see [messaging.md](messaging.md)).
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
      "destination": "iot_core"
    }
  }
}
```

#### 4. Prometheus Target (`"prometheus"`)
A **pull-based** target (FR-MET-1/2/3): instead of pushing an EMF datum on each emit, it maintains an
in-process registry of latest-value gauges and serves it as OpenMetrics/Prometheus text over HTTP for
a scraper to pull. This is the **default metric target on the KUBERNETES platform** (the `prometheus`
client lib is `prometheus-client`, an install dependency of the library).

**Inverted lifecycle (FR-MET-2) — different from the push targets above:**
- `emit_metric()` and `emit_metric_now()` both **only update the in-process registry** (set the gauge
  for the emitted label-set). They never deliver anywhere and never make a network call, so a metric
  emit can never block on the cloud. (There is no batching and no flush — the "batched" and
  "immediate" paths are identical here.)
- Delivery happens when a Prometheus scraper performs a `GET` against the exposition endpoint — the
  *pull*. A "flush" is therefore a no-op with respect to delivery.
- `close()` (invoked by `MetricEmitter.shutdown()` / `gg.shutdown()`) **stops the HTTP listener**, so
  no port/thread leaks.

The push targets (`log`/`messaging`/`cloudwatch`/`cloudwatchcomponent`) are unchanged — they still
push EMF on emit. Only the `prometheus` target inverts the lifecycle.

**Dimension → label mapping (FR-MET-3, identical across all four languages):**
- gauge **name** = `sanitize(lowercase("{namespace}_{measureName}"))`, where `namespace` defaults to
  `edgecommons` and `sanitize` replaces every char not matching `[a-z0-9_]` with `_` and prefixes a
  leading digit with `_` (Prometheus metric-name rules).
- **labels** = the metric's dimensions (`coreName`, `category` = the metric name, `component`, plus
  any custom dimensions). Each label *name* is sanitized to `[a-zA-Z_][a-zA-Z0-9_]*` (invalid chars →
  `_`, leading digit prefixed with `_`); each label *value* is used as-is.
- the gauge for that label-set is **set** to the measure's float value on each emit (latest-value
  gauge semantics).

The exposition binds `0.0.0.0` on the configured `port` (default `9090`) and serves the configured
`path` (default `/metrics`); any other path returns `404`. The `Content-Type` is the client lib's
`CONTENT_TYPE_LATEST` (`text/plain; version=0.0.4; charset=utf-8`) — a valid, non-blank type that
Prometheus 3.x accepts.

**Configuration options:**
- **`port`**: HTTP port for the `/metrics` endpoint (Default: 9090).
- **`path`**: HTTP path for the OpenMetrics exposition (Default: "/metrics").

**Example:**
```json
{
  "metricEmission": {
    "target": "prometheus",
    "namespace": "MyApp/Metrics",
    "targetConfig": {
      "port": 9090,
      "path": "/metrics"
    }
  }
}
```

On the KUBERNETES platform the section can be omitted entirely — the platform-profile default selects
`prometheus` with the default port/path. An explicit `target` (e.g. `"log"`) always overrides the
platform default.

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
    }
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
      "destination": "iot_core"
    }
  }
}
```
Metrics publish to `ecv1/{device}/{component}/main/metric/{metricName}` on IoT Core.

## Using Metrics in Code

### Enhanced Builder Pattern
```python
from edgecommons.builders import MetricBuilder
from edgecommons.interfaces import IMetricService

# Get metric service through dependency injection
metric_service = edgecommons.get_service(IMetricService)

# Define a custom metric with builder pattern
metric = MetricBuilder.create("data_processed") \
    .with_namespace("MyApp/Processing") \
    .add_measure("count", "Count", 1) \
    .add_measure("size_bytes", "Bytes", 1) \
    .add_dimension("instance", "main") \
    .build()

# Define the metric in the system
metric_service.define_metric(metric)

# Emit metric values
values = {
    "count": 100.0,
    "size_bytes": 1024.0
}
metric_service.emit_metric("data_processed", values)
```

### Legacy MetricEmitter (Still Supported)
```python
from edgecommons.metrics.metric_emitter import MetricEmitter
from edgecommons.metrics.metric import Metric
from edgecommons.metrics.measure import Measure

# Define a metric
metric = Metric("performance_metric")
metric.add_measure(Measure("latency", "Milliseconds", 1))
metric.add_measure(Measure("throughput", "Count", 1))

MetricEmitter.define_metric(metric)

# Emit values
values = {
    "latency": 150.0,
    "throughput": 50.0
}
MetricEmitter.emit_metric("performance_metric", values)
```

## EMF (Embedded Metric Format)

All targets use the EMF format for CloudWatch compatibility. The EMF structure includes:
- Timestamp in Unix epoch milliseconds
- CloudWatch metadata with namespace, dimensions, and metric definitions
- Dimension values as top-level properties
- Measure values as top-level properties
- AWS-specific metadata in `_aws` object

Example EMF output:
```json
{
  "_aws": {
    "Timestamp": 1640995200000,
    "CloudWatchMetrics": [
      {
        "Namespace": "MyApp/Metrics",
        "Dimensions": [["ComponentName", "ThingName"]],
        "Metrics": [
          {"Name": "count", "Unit": "Count"},
          {"Name": "size_bytes", "Unit": "Bytes"}
        ]
      }
    ]
  },
  "ComponentName": "MyComponent",
  "ThingName": "my-device",
  "count": 100.0,
  "size_bytes": 1024.0
}
```

## Performance Considerations

### Target-Specific Behavior

**CloudWatch Direct:**
- Batches metrics by namespace using timer-based emission
- Each namespace maintains a queue for batching
- Sends up to 20 metrics per API call (AWS limit)
- Memory usage scales with the batching interval and metric volume

**Log Target:**
- Immediate writing with file rotation
- Performance limited by disk I/O
- File rotation prevents unlimited disk usage
- Thread-safe through Python logging

**Messaging:**
- Immediate transmission
- IPC: Local processing limits
- IoT Core: Subject to IoT Core throttling

### Best Practices

1. **Target Selection:**
   - Use `cloudwatch` for production CloudWatch integration
   - Use `log` for local debugging and development
   - Use `messaging` for real-time monitoring or custom processing

2. **Configuration:**
   - Set `intervalSecs` based on metric volume and latency requirements
   - Use meaningful namespaces for metric organization
   - Configure appropriate file rotation sizes for log target

3. **Monitoring:**
   - Monitor memory usage with high-volume metrics
   - Watch for CloudWatch API throttling
   - Check disk space when using log target
   - Verify IAM permissions for CloudWatch targets

## Troubleshooting

### Common Issues
- **Metrics not appearing in CloudWatch**: Check IAM permissions and network connectivity
- **Log files not rotating**: Verify file permissions and disk space
- **High memory usage**: Reduce batching interval or metric volume
- **Missing metrics**: Check metric definition and emission calls

### Debug Configuration
```json
{
  "logging": {
    "level": "DEBUG",
    "loggers": {
      "edgecommons.metrics": "DEBUG"
    }
  },
  "metricEmission": {
    "target": "log",
    "namespace": "Debug/Metrics",
    "targetConfig": {
      "logFileName": "./debug-metrics.log"
    }
  }
}
```

### Validation
- Test metric emission in development environments
- Verify EMF format compliance for CloudWatch compatibility
- Monitor metric emission performance and resource usage
- Validate template variable resolution in configuration