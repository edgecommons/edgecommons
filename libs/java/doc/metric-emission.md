# Metric Emission Configuration Guide

This document provides detailed information about the metric emission targets and their configuration options in the ggcommons-java-lib.

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
  - `"cloudwatchcomponent"` - CloudWatch metrics via Greengrass component
  - `"log"` - Local file logging with rotation support
  - `"messaging"` - Message-based metrics via IPC or IoT Core
- **`namespace`**: The namespace for your metrics (Default: "ggcommons")
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
- Default topic: "cloudwatch/metric/put"
- Sends each measure as a separate message (no batching)
- Does not support `largeFleetWorkaround` due to component limitations
- Lighter weight than direct CloudWatch target

**Configuration options:**
- **`topic`**: Override the default CloudWatch component topic (supports templates)

**Example:**
```json
{
  "metricEmission": {
    "target": "cloudwatchcomponent",
    "namespace": "MyComponent/Metrics",
    "targetConfig": {
      "topic": "custom/cloudwatch/metrics"
    }
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

#### 4. Messaging Target (`"messaging"`)
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
      "destination": "cloud"
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
      "topic": "telemetry/{ThingName}/metrics",
      "destination": "cloud"
    }
  }
}
```

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