TODO: This file was GenAI generated and needs updating/corrections

# General Configuration Guide

This document provides detailed information about the metric emission targets and their configuration options, based on analyzing the actual implementation code.

The metric emission system is designed to be flexible and support multiple emission targets simultaneously. It handles collecting, formatting, and emitting metrics through various channels including CloudWatch, local logs, and messaging systems.

## Metric Emission Configuration

The system supports multiple metric emission targets for sending metrics, each with specific behaviors and configuration options. The base configuration is controlled through the `metricEmission` section of your configuration file.

### Metric Structure

Metrics in the system consist of:
- Name: Unique identifier for the metric
- Namespace: Logical grouping for metrics (e.g., application name)
- Measures: Key-value pairs of float values representing the actual measurements
- Dimensions: Automatically added metadata such as component name and thing name

### Common Configuration Options

- `target`: (Required) Specifies which metric emission target to use. Valid values:
  - `"cloudWatch"` - Direct CloudWatch metrics emission
  - `"cloudwatchcomponent"` - CloudWatch metrics via component
  - `"log"` - Local file logging
  - `"messaging"` - Message-based metrics
- `namespace`: The namespace for your metrics (Default: "ggcommons")
- `intervalSecs`: The interval between metric emissions (Default: 5 seconds)
- `largeFleetWorkaround`: Boolean flag for optimizing large fleet deployments by reducing API calls (Default: false)

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

#### 1. CloudWatch Target (`"cloudWatch"`)
The CloudWatch target sends metrics directly to Amazon CloudWatch using the AWS SDK.

Implementation details:
- Creates a CloudWatchClient instance for direct AWS API access
- Maintains separate queues for each metric namespace
- Batches metrics and sends them on a configurable interval
- Supports concurrent metric collection via ConcurrentLinkedQueue
- Automatically handles AWS API limits by batching

Configuration options:
- `namespace`: (Required) The CloudWatch namespace for your metrics
- `intervalSecs`: Batching interval for CloudWatch API calls (minimum: 1 second)
- `largeFleetWorkaround`: Enable optimizations for large fleet deployments

Example:
```json
{
  "metricEmission": {
    "target": "cloudWatch",
    "targetConfig": {
      "intervalSecs": 60
    },
    "namespace": "MyApp/Metrics",
    "largeFleetWorkaround": false
  }
}
```

#### 2. CloudWatch Component Target (`"cloudwatchcomponent"`)
This target sends metrics to CloudWatch through a Greengrass component rather than directly.

Implementation details:
- Uses IPC to communicate with CloudWatch component
- Default topic: "cloudwatch/metric/put"
- Does not batch metrics (sends immediately)
- Lighter weight than direct CloudWatch target

Configuration options:
- `topic`: Override the default CloudWatch component topic
- `namespace`: Metrics namespace

Example:
```json
{
  "metricEmission": {
    "target": "cloudwatchcomponent",
    "targetConfig": {
      "topic": "custom/cloudwatch/metrics"
    },
    "namespace": "MyComponent/Metrics"
  }
}
```

#### 3. Log Target (`"log"`)
The Log target writes metrics to a local log file with configurable formatting.

Implementation details:
- Uses Log4j2 for file logging
- Supports template-based file naming
- Configurable log format through logging configuration
- Immediate metric writing (no batching)
- Thread-safe logging implementation

Configuration options:
- `logFileNameTemplate`: Template for log file naming (Default: "/greengrass/v2/logs/{ComponentFullName}.metric.log")
- Template variables supported:
  - {ComponentFullName}
  - {ThingName}
  - {ComponentName}
  - Standard date/time patterns

Example:
```json
{
  "metricEmission": {
    "target": "log",
    "targetConfig": {
      "logFileName": "/custom/path/metrics-%Y-%m-%d.log"
    }
  }
}
```

#### 4. Messaging Target (`"messaging"`)
The Messaging target publishes metrics through the messaging system, supporting both local IPC and IoT Core destinations.

Implementation details:
- Supports both IPC and IoT Core publishing
- Default topic template: "{ThingName}/{ComponentName}/metric"
- Uses QoS 1 (at least once delivery) for IoT Core
- Immediate message publishing (no batching)
- Messages include version and metadata

Configuration options:
- `topic`: Override the default topic template
- `destination`: Specify message destination (Default: "ipc")
  - "ipc": Local Greengrass IPC communication
  - Any other value: Publish to IoT Core
  
Example:
```json
{
  "metricEmission": {
    "target": "messaging",
    "targetConfig": {
      "topic": "metrics/{ThingName}/data",
      "destination": "cloud"
    }
  }
}
```

## Multiple Instance Configuration Example

The following example demonstrates how to configure multiple instances with different metric emission strategies. This is provided as an illustration of the configuration structure only.

```json
{
  "instances": {
    "critical-metrics": {
      "metricEmission": {
        "target": "cloudWatch",
        "namespace": "Critical/Metrics",
        "targetConfig": {
          "intervalSecs": 30
        }
      }
    },
    "debug-metrics": {
      "metricEmission": {
        "target": "log",
        "targetConfig": {
          "logFileName": "debug-metrics-%Y-%m-%d.log"
        }
      }
    },
    "realtime-metrics": {
      "metricEmission": {
        "target": "messaging",
        "targetConfig": {
          "topic": "metrics/realtime/{ComponentName}",
          "destination": "cloud"
        }
      }
    }
  }
}
```

### Understanding Multiple Instances

The "instances" configuration pattern shown above is an example that demonstrates the system's ability to handle different metric emission configurations for different components or purposes:

- Each instance can have completely independent configurations
- Useful for:
  - Sending critical metrics directly to CloudWatch while logging debug metrics locally
  - Using different namespaces for different component types
  - Implementing different emission strategies based on metric importance
  - Testing new configurations alongside existing ones

Implementation Note: Each instance maintains its own metric target instance and configuration, ensuring complete isolation between different metric streams.

## Performance Considerations and Troubleshooting

### Batching and Rate Limits

The system handles different rate limits and batching strategies based on the target:

1. CloudWatch Direct:
   - Metrics are batched and sent every `intervalSecs`
   - Each namespace maintains a separate queue
   - Maximum of 20 metrics per batch (AWS API limit)
   - Uses concurrent queue for thread safety

2. CloudWatch Component:
   - No batching - immediate transmission
   - Relies on component's own rate limiting
   - Lower overhead than direct CloudWatch

3. Log Target:
   - Immediate writing to log file
   - Performance limited by disk I/O
   - Uses Log4j2 for efficient logging

4. Messaging:
   - Immediate transmission
   - IPC: Limited by local processing
   - IoT Core: Subject to IoT Core limits

### Common Issues and Solutions

1. High Memory Usage:
   - Check batching interval for CloudWatch target
   - Consider using CloudWatch Component instead of direct
   - Verify metric emission frequency

2. Missing Metrics:
   - Verify namespace configuration
   - Check log files for errors
   - Ensure proper IAM permissions for CloudWatch

3. Performance Issues:
   - Enable `largeFleetWorkaround` for big deployments
   - Increase `intervalSecs` for less frequent emission
   - Consider using local logging for debug metrics

4. Template Resolution:
   - Verify thing name is properly configured
   - Check component name registration
   - Validate template syntax

### Best Practices

1. Target Selection:
   - Use CloudWatch Component for most cases
   - Direct CloudWatch for custom batching needs
   - Local logging for debugging
   - Messaging for real-time monitoring

2. Configuration:
   - Set appropriate intervals based on metric importance
   - Use namespaces to organize metrics
   - Leverage template variables for dynamic naming
   - Configure batching based on metric volume

3. Resource Usage:
   - Monitor memory usage with large metric volumes
   - Consider disk space for log target
   - Watch for API throttling with CloudWatch
   - Use appropriate QoS for messaging