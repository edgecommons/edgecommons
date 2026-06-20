# Metric Emission Configuration Guide

This document provides detailed information about the metric emission targets and their configuration options in the ggcommons-python-lib.

The metric emission system supports multiple targets for sending metrics, each with specific behaviors and configuration options. Metrics are formatted using the Embedded Metric Format (EMF) for CloudWatch compatibility.

## Metric Emission Configuration

### Metric Structure

Metrics in the system consist of:
- **Name**: Unique identifier for the metric
- **Namespace**: Logical grouping for metrics (defaults to "ggcommons")
- **Measures**: Named measurements with values, units, and storage resolution
- **Dimensions**: Key-value pairs for metric categorization (automatically includes component and thing names)

### Common Configuration Options

- **`target`**: (Required) Specifies which metric emission target to use. Valid values:
  - `"cloudwatch"` - Direct CloudWatch metrics emission with batching
  - `"log"` - Local file logging with rotation support
  - `"messaging"` - Message-based metrics via IPC or IoT Core
- **`namespace`**: The namespace for your metrics (Default: "ggcommons")

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
- **`logFileName`**: Template for log file naming (Default: "/greengrass/v2/logs/{ComponentFullName}.metric.log")
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
- Uses MessagingClient for both IPC and IoT Core publishing
- Default topic template: "{ThingName}/{ComponentName}/metric"
- Uses QoS AT_LEAST_ONCE for IoT Core publishing
- Immediate message publishing (no batching)
- Messages formatted as EMF with Message wrapper including version and metadata
- Supports template variable substitution in topic names

**Configuration options:**
- **`topic`**: Override the default topic template (supports template variables)
- **`destination`**: Specify message destination (Default: "ipc")
  - `"ipc"`: Local Greengrass IPC communication
  - Any other value: Publish to IoT Core

**Example:**
```json
{
  "metricEmission": {
    "target": "messaging",
    "namespace": "MyApp/Metrics",
    "targetConfig": {
      "topic": "metrics/{ThingName}/{ComponentName}",
      "destination": "iot_core"
    }
  }
}
```

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
      "topic": "telemetry/{ThingName}/metrics",
      "destination": "iot_core"
    }
  }
}
```

## Using Metrics in Code

### Enhanced Builder Pattern
```python
from ggcommons.builders import MetricBuilder
from ggcommons.interfaces import IMetricService

# Get metric service through dependency injection
metric_service = ggcommons.get_service(IMetricService)

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
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.measure import Measure

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
      "ggcommons.metrics": "DEBUG"
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