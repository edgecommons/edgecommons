TODO: This file was GenAI generated and needs updating/corrections

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
- Heartbeats are emitted on a configurable schedule (default: every 5 seconds)
- Each heartbeat can include the following metrics:
  - CPU usage (percentage)
  - Memory usage (megabytes)
  - Thread count
  - Open file count
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
     - `topic`: Topic pattern for message publishing (default: "ggcommons/{ThingName}/{ComponentName}/heartbeat")

### Metric Collection Options

Configure which metrics to collect using the `measures` object:
- `cpu`: CPU usage monitoring (default: true)
- `memory`: Memory usage monitoring (default: true)
- `threads`: Thread count monitoring (default: false)
- `files`: Open file count monitoring (default: false)

Additional configuration:
- `intervalSecs`: Heartbeat emission interval in seconds (default: 5, minimum: 1)

## 4. Sample Configurations

### Sample 1: Basic Metric Monitoring
```json
{
    "intervalSecs": 5,
    "measures": {
        "cpu": true,
        "memory": true,
        "threads": false,
        "files": false
    },
    "targets": [
        {
            "type": "metric"
        }
    ]
}
```
This configuration provides basic component monitoring with CPU and memory metrics emitted every 5 seconds through the metric emission system.

### Sample 2: Comprehensive Monitoring with Multiple Targets
```json
{
    "intervalSecs": 30,
    "measures": {
        "cpu": true,
        "memory": true,
        "threads": true,
        "files": true
    },
    "targets": [
        {
            "type": "metric"
        },
        {
            "type": "messaging",
            "config": {
                "destination": "ipc",
                "topic": "ggcommons/{ThingName}/{ComponentName}/detailed-heartbeat"
            }
        }
    ]
}
```
This configuration enables comprehensive monitoring with all available metrics, emitted every 30 seconds to both the metric system and a custom messaging topic.

### Sample 3: High-Frequency Memory Monitoring
```json
{
    "intervalSecs": 1,
    "measures": {
        "cpu": false,
        "memory": true,
        "threads": false,
        "files": false
    },
    "targets": [
        {
            "type": "messaging",
            "config": {
                "destination": "ipc",
                "topic": "ggcommons/{ThingName}/{ComponentName}/memory-monitor"
            }
        }
    ]
}
```
This configuration focuses on memory monitoring with high-frequency updates (every second) published to a dedicated messaging topic.

### Sample 4: Multi-Target Resource Monitoring
```json
{
    "intervalSecs": 15,
    "measures": {
        "cpu": true,
        "memory": true,
        "threads": true,
        "files": false
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
                "destination": "cloud",
                "topic": "device/{ThingName}/health"
            }
        }
    ]
}
```
This configuration monitors CPU, memory, and threads every 15 seconds, publishing to the metric system and two different messaging destinations (local IPC and cloud) for comprehensive monitoring coverage.