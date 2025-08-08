# Heartbeat System Documentation

## 1. Overview

The heartbeat system is a component monitoring solution that provides real-time health and status information for Greengrass components. It enables operational visibility by regularly emitting metrics about component health, resource utilization, and system status. This system is essential for:
- Monitoring component health and liveness
- Tracking resource utilization
- Early detection of potential issues
- Facilitating component observability in distributed systems

## 2. Behavior

When a component utilizes the heartbeat system:

- Heartbeats automatically begin at component initialization
- Heartbeats are emitted on a configurable schedule (default: every 30 seconds)
- Each heartbeat can include the following metrics:
  - CPU usage (percentage)
  - Memory usage (megabytes)
  - Thread count
  - Open file count
  - File descriptor count (handles on Windows)
  - Disk usage (total, used, free in gigabytes)
  - System timestamps
  - Component identification information

The system automatically manages the timing and collection of these metrics, requiring minimal intervention once configured.

## 3. Configuration

The heartbeat system supports flexible configuration through JSON, with two main target types and various metric collection options.

### Target Types

1. **Metric Target** (`"type": "metric"`)
   - Emits heartbeats through the metric emission system
   - No additional configuration required
   - For detailed metric emission configuration, see [metric emission documentation](metric-emission.md)

2. **Messaging Target** (`"type": "messaging"`)
   - Publishes heartbeats through the messaging system
   - Configuration options:
     - `destination`: Specifies the message destination ("ipc" by default)
     - `topic`: Topic pattern for message publishing (default: "heartbeat/{ThingName}/{ComponentName}")

### Metric Collection Options

Configure which metrics to collect using the `measures` object:
- `cpu`: CPU usage monitoring (default: true)
- `memory`: Memory usage monitoring (default: true)
- `disk`: Disk usage monitoring (default: false)
- `threads`: Thread count monitoring (default: false)
- `files`: Open file count monitoring (default: false)
- `fds`: File descriptor count monitoring (default: false)

Additional configuration:
- `intervalSecs`: Heartbeat emission interval in seconds (default: 30, minimum: 1)

## 4. Sample Configurations

### Sample 1: Basic Metric Monitoring
```json
{
  "heartbeat": {
    "intervalSecs": 30,
    "measures": {
      "cpu": true,
      "memory": true,
      "disk": false,
      "threads": false,
      "files": false,
      "fds": false
    },
    "targets": [
      {
        "type": "metric"
      }
    ]
  }
}
```
This configuration provides basic component monitoring with CPU and memory metrics emitted every 30 seconds through the metric emission system.

### Sample 2: Comprehensive Monitoring with Multiple Targets
```json
{
  "heartbeat": {
    "intervalSecs": 60,
    "measures": {
      "cpu": true,
      "memory": true,
      "disk": true,
      "threads": true,
      "files": true,
      "fds": true
    },
    "targets": [
      {
        "type": "metric"
      },
      {
        "type": "messaging",
        "config": {
          "destination": "ipc",
          "topic": "heartbeat/{ThingName}/{ComponentName}/detailed"
        }
      }
    ]
  }
}
```
This configuration enables comprehensive monitoring with all available metrics, emitted every 60 seconds to both the metric system and a custom messaging topic.

### Sample 3: High-Frequency Memory Monitoring
```json
{
  "heartbeat": {
    "intervalSecs": 5,
    "measures": {
      "cpu": false,
      "memory": true,
      "disk": false,
      "threads": false,
      "files": false,
      "fds": false
    },
    "targets": [
      {
        "type": "messaging",
        "config": {
          "destination": "ipc",
          "topic": "monitoring/{ThingName}/{ComponentName}/memory"
        }
      }
    ]
  }
}
```
This configuration focuses on memory monitoring with high-frequency updates (every 5 seconds) published to a dedicated messaging topic.

### Sample 4: Multi-Target Resource Monitoring
```json
{
  "heartbeat": {
    "intervalSecs": 15,
    "measures": {
      "cpu": true,
      "memory": true,
      "disk": true,
      "threads": true,
      "files": false,
      "fds": false
    },
    "targets": [
      {
        "type": "metric"
      },
      {
        "type": "messaging",
        "config": {
          "destination": "ipc",
          "topic": "monitoring/component-health"
        }
      },
      {
        "type": "messaging",
        "config": {
          "destination": "iot_core",
          "topic": "device/{ThingName}/health"
        }
      }
    ]
  }
}
```
This configuration monitors CPU, memory, disk, and threads every 15 seconds, publishing to the metric system and two different messaging destinations (local IPC and IoT Core) for comprehensive monitoring coverage.

## 5. Metric Details

### CPU Usage
- **Unit**: Percent
- **Range**: 0-100%
- **Description**: Current CPU utilization of the component process
- **Implementation**: Uses `psutil.Process.cpu_percent()`

### Memory Usage
- **Unit**: Megabytes (MB)
- **Description**: Resident Set Size (RSS) memory usage
- **Implementation**: Uses `psutil.Process.memory_info().rss / 1,000,000`

### Disk Usage
- **Units**: Gigabytes (GB)
- **Metrics**: 
  - `disk_total`: Total disk space
  - `disk_used`: Used disk space
  - `disk_free`: Available disk space
- **Implementation**: Uses `shutil.disk_usage()` converted to GB

### Thread Count
- **Unit**: Count
- **Description**: Number of threads in the component process
- **Implementation**: Uses `len(psutil.Process.threads())`

### Open Files
- **Unit**: Count
- **Description**: Number of open file handles
- **Implementation**: Uses `len(psutil.Process.open_files())`

### File Descriptors
- **Unit**: Count
- **Description**: Number of file descriptors (Linux/Mac) or handles (Windows)
- **Implementation**: Uses `psutil.Process.num_fds()` or `psutil.Process.num_handles()`

## 6. Integration with Other Systems

### Metric Emission Integration
The heartbeat system integrates seamlessly with the metric emission system. When using the "metric" target, heartbeats are processed through the same metric emission pipeline as custom application metrics.

### Messaging Integration
When using messaging targets, heartbeats are published as structured messages through the messaging system, supporting both local IPC and IoT Core destinations.

### Configuration Change Support
The heartbeat system responds to configuration changes through the configuration change listener system, allowing dynamic reconfiguration without component restart.

## 7. Usage in Code

### Accessing Heartbeat Service
```python
from ggcommons.builders import GGCommonsBuilder
from ggcommons.interfaces import IConfigurationService

# Initialize GGCommons (heartbeat starts automatically)
ggcommons = GGCommonsBuilder.create("com.example.MyComponent") \
    .with_args(args) \
    .build()

# Heartbeat is automatically initialized and started
# No additional code required for basic operation
```

### Custom Heartbeat Handling
```python
from ggcommons.heartbeat.heartbeat import Heartbeat
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener

class CustomHeartbeatListener(ConfigurationChangeListener):
    def on_configuration_change(self, configuration):
        # Handle heartbeat configuration changes
        print("Heartbeat configuration updated")
        return True

# The heartbeat system is automatically managed by GGCommons
# Custom listeners can be added for configuration changes
config_service = ggcommons.get_service(IConfigurationService)
config_service.add_config_change_listener(CustomHeartbeatListener())
```

## 8. Best Practices

### Interval Selection
- **Development**: Use shorter intervals (5-10 seconds) for debugging
- **Production**: Use longer intervals (30-60 seconds) to reduce overhead
- **High-frequency monitoring**: Only enable specific metrics needed
- **Resource-constrained environments**: Increase intervals to reduce CPU/network usage

### Metric Selection
- **Basic monitoring**: Enable CPU and memory only
- **Comprehensive monitoring**: Enable all metrics for troubleshooting
- **Performance-sensitive**: Disable file and thread counting for better performance
- **Disk monitoring**: Enable only if disk usage is a concern

### Target Configuration
- **Development**: Use messaging targets for real-time visibility
- **Production**: Use metric targets for CloudWatch integration
- **Hybrid**: Use both targets for comprehensive monitoring
- **Network-constrained**: Prefer local messaging over IoT Core

### Template Variables
- Use `{ThingName}` and `{ComponentName}` in topic patterns
- Leverage custom tags for environment-specific routing
- Test template resolution in development environments

## 9. Troubleshooting

### Common Issues
- **Heartbeats not appearing**: Check target configuration and metric emission setup
- **High CPU usage**: Reduce heartbeat frequency or disable expensive metrics
- **Memory leaks**: Monitor memory usage trends over time
- **Network issues**: Check IoT Core connectivity for messaging targets

### Debug Configuration
```json
{
  "logging": {
    "level": "DEBUG",
    "loggers": {
      "ggcommons.heartbeat": "DEBUG"
    }
  },
  "heartbeat": {
    "intervalSecs": 5,
    "measures": {
      "cpu": true,
      "memory": true
    },
    "targets": [
      {
        "type": "messaging",
        "config": {
          "destination": "ipc",
          "topic": "debug/heartbeat"
        }
      }
    ]
  }
}
```

### Monitoring Heartbeat Health
- Monitor heartbeat message timestamps for component liveness
- Set up alerts for missing heartbeats in production
- Use heartbeat data for capacity planning and resource optimization
- Track trends in resource usage over time

### Performance Considerations
- File and thread counting can be expensive on some systems
- Disk usage monitoring involves filesystem operations
- Network publishing adds latency and bandwidth usage
- Consider the trade-off between monitoring detail and system performance