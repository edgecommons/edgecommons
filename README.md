# GGCommons Python Library

A comprehensive Python library for building AWS IoT Greengrass components with built-in configuration management, messaging, metrics, heartbeat monitoring, and logging capabilities.

## Purpose

GGCommons simplifies the development of AWS IoT Greengrass components by providing a unified framework that handles common operational concerns, allowing developers to focus on their core business logic. The library abstracts away the complexity of Greengrass integration while providing enterprise-grade features for monitoring, configuration management, and inter-component communication.

**🚀 NEW: STANDALONE Mode** - Run components outside of Greengrass with nearly full functionality! Perfect for Kubernetes, Docker, or any container runtime environment. Maintains dual connectivity to both local MQTT brokers and AWS IoT Core.

**🚀 NEW: Enhanced Architecture** - Now featuring dependency injection, builder patterns, and comprehensive configuration validation! Improved testability and maintainability while maintaining full backward compatibility.

## Key Capabilities

### 🔧 Configuration Management
- **Multiple Sources**: Load configuration from files, environment variables, Greengrass deployment, or IoT Device Shadows
- **Template Variables**: Dynamic value substitution using component, thing, and custom tag variables
- **Runtime Updates**: Hot configuration reloading without component restart
- **Multi-Instance Support**: Manage configuration for components with multiple instances
- **JSON Schema Validation**: Comprehensive validation with detailed error reporting

[📖 Configuration Documentation](doc/configuration.md)

### 📨 Messaging System
- **Multi-Runtime Support**: Native Greengrass IPC or STANDALONE mode with dual MQTT clients
- **Dual MQTT Connectivity**: Simultaneous local broker and AWS IoT Core connections in STANDALONE mode
- **Request-Response Pattern**: Built-in support for synchronous communication
- **Topic Filtering**: Advanced subscription patterns with wildcards
- **Message Serialization**: Automatic JSON serialization with metadata headers
- **Certificate & Username Auth**: Support for both authentication methods on local brokers
- **Service Interface**: Clean abstraction for testing and modularity

[📖 Messaging Documentation](doc/messaging.md)

### 📊 Metrics Collection & Emission
- **Multiple Targets**: Send metrics to CloudWatch, local logs, or messaging topics
- **EMF Format**: Embedded Metric Format for CloudWatch compatibility
- **Custom Dimensions**: Automatic component and thing name dimensions
- **Builder Pattern**: Fluent API for metric definition
- **Service Interface**: Testable metric emission

[📖 Metrics Documentation](doc/metric-emission.md)

### 💓 Health Monitoring
- **System Metrics**: CPU, memory, disk, thread, and file descriptor monitoring
- **Configurable Intervals**: Adjustable heartbeat frequency
- **Multiple Outputs**: Send health data via metrics or messaging
- **Resource Tracking**: Built-in system resource monitoring

[📖 Heartbeat Documentation](doc/heartbeat.md)

### 📝 Logging System
- **Python Logging Integration**: Built on Python's standard logging framework
- **Enhanced Configuration**: File logging with rotation and per-logger levels
- **Dynamic Configuration**: Runtime log level and output adjustments
- **Structured Logging**: Consistent formatting with template variable support

[📖 Logging Documentation](doc/logging.md)

### 🏗️ Enhanced Architecture
- **Dependency Injection**: Service registry for loose coupling and testability
- **Builder Patterns**: Fluent APIs for object construction
- **Service Interfaces**: Clean abstractions for core functionality
- **Configuration Validation**: JSON schema validation with detailed error messages

## Quick Start

### 1. Install Dependencies

Install the GGCommons library and its dependencies:

```bash
pip install -r requirements.txt
```

### 2. Basic Component Structure (Enhanced API)

```python
from ggcommons import GGCommonsBuilder
from ggcommons.interfaces import IMessagingService, IConfigurationService

class MyComponent:
    def __init__(self):
        self.ggcommons = None
        self.messaging_service = None
        self.config_service = None
    
    def main(self, args):
        # Initialize GGCommons with enhanced builder pattern
        self.ggcommons = GGCommonsBuilder.create("com.example.MyComponent") \
            .with_args(args) \
            .build()
        
        # Get services through dependency injection
        self.messaging_service = self.ggcommons.get_service(IMessagingService)
        self.config_service = self.ggcommons.get_service(IConfigurationService)
        
        # Your component logic here
        self.start_application()
    
    def start_application(self):
        # Access configuration
        global_config = self.config_service.get_global_config()
        
        # Process each configured instance
        for instance_id in self.config_service.get_instance_ids():
            instance_config = self.config_service.get_instance_config(instance_id)
            # Start instance-specific processing

if __name__ == "__main__":
    import sys
    MyComponent().main(sys.argv[1:])
```

### 3. Legacy Component Structure (Backward Compatible)

```python
import ggcommons
import argparse

def main():
    parser = argparse.ArgumentParser()
    
    # Initialize GGCommons (legacy method - still supported)
    args, config_manager, heartbeat = ggcommons.init(
        "com.example.MyComponent", 
        parser
    )
    
    # Your component logic here
    start_application(config_manager)

def start_application(config_manager):
    # Access configuration
    global_config = config_manager.get_global_config()
    
    # Process each configured instance
    for instance_id in config_manager.get_instance_ids():
        instance_config = config_manager.get_instance_config(instance_id)
        # Start instance-specific processing

if __name__ == "__main__":
    main()
```

### 4. Configuration File Example

Create a configuration file (e.g., `config.json`):

```json
{
  "logging": {
    "level": "INFO",
    "format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "/var/log/{ComponentName}.log",
      "maxFileSize": "10MB",
      "backupCount": 5
    }
  },
  "heartbeat": {
    "intervalSecs": 30,
    "measures": {
      "cpu": true,
      "memory": true,
      "disk": false
    },
    "targets": [{"type": "metric"}]
  },
  "metricEmission": {
    "target": "cloudwatch",
    "namespace": "MyApplication"
  },
  "tags": {
    "environment": "production",
    "site": "factory-1"
  },
  "component": {
    "global": {
      "serverUrl": "https://api.example.com",
      "timeout": 5000
    },
    "instances": [
      {
        "id": "main",
        "database": {
          "host": "db.{environment}.local",
          "port": 5432
        }
      }
    ]
  }
}
```

### 5. Run Your Component

```bash
# Greengrass mode (default) - for AWS IoT Greengrass runtime
python3 main.py -c GG_CONFIG -t my-thing-name

# STANDALONE mode - for Kubernetes, Docker, or any container runtime
python3 main.py -m STANDALONE ./standalone-messaging.json -c FILE ./config.json -t my-thing-name
```

### 6. STANDALONE Mode Configuration

Create a `standalone-messaging.json` file for non-Greengrass deployments:

```json
{
  "messaging": {
    "local": {
      "type": "mqtt",
      "host": "localhost",
      "port": 1883,
      "clientId": "my-component-local",
      "credentials": {
        "username": "mqtt-user",
        "password": "mqtt-pass"
      }
    },
    "iotCore": {
      "endpoint": "your-endpoint.iot.us-east-1.amazonaws.com",
      "port": 8883,
      "clientId": "my-component-iotcore",
      "credentials": {
        "certPath": "/certs/device-cert.pem",
        "keyPath": "/certs/private-key.pem",
        "caPath": "/certs/root-ca.pem"
      }
    }
  }
}
```

## Command Line Options

GGCommons supports several command line options for configuration and messaging:

### Configuration Source (`-c, --config`)
- `FILE [path]` - Load from JSON file
- `ENV [var_name]` - Load from environment variable (default: GGCOMMONS_CONFIG)
- `GG_CONFIG [component] [key]` - Load from Greengrass deployment (default)
- `SHADOW [name]` - Load from IoT Device Shadow
- `CONFIG_COMPONENT` - Load from configuration management component

### Runtime Mode (`-m, --mode`)
- `GREENGRASS` - Use Greengrass IPC (default)
- `STANDALONE <config_file_path>` - **NEW!** Use dual MQTT clients for non-Greengrass environments
  - Enables deployment to Kubernetes, Docker, or any container runtime
  - Maintains connectivity to both local MQTT broker and AWS IoT Core
  - Nearly full functionality outside of Greengrass

### Thing Name (`-t, --thing`)
- Specify the AWS IoT Thing name (optional, auto-detected in Greengrass)

## Advanced Features

### Enhanced Messaging with Builder Pattern

```python
from ggcommons.messaging import MessageBuilder
from ggcommons.interfaces import IMessagingService

# Get messaging service
messaging = ggcommons.get_service(IMessagingService)

# Subscribe to messages
messaging.subscribe("requests/process", self.handle_request, 1)

# Create and send message with builder pattern
message = MessageBuilder.create("ProcessData", "1.0") \
    .with_payload(payload) \
    .with_config(config_manager) \
    .with_correlation_id("req-123") \
    .build()

messaging.publish("requests/process", message)
```

### Custom Metrics with Builder Pattern

```python
from ggcommons.metrics import MetricBuilder
from ggcommons.interfaces import IMetricService

# Define a custom metric with builder pattern
metric = MetricBuilder.create("data_processed") \
    .with_namespace("MyApp/Metrics") \
    .add_measure("count", "Count", 1) \
    .add_measure("size_bytes", "Bytes", 1) \
    .add_dimension("instance", "main") \
    .build()

# Get metric service and define metric
metric_service = ggcommons.get_service(IMetricService)
metric_service.define_metric(metric)

# Emit metric values
values = {
    "count": 100.0,
    "size_bytes": 1024.0
}
metric_service.emit_metric("data_processed", values)
```

### Configuration Change Handling

```python
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener

class MyConfigListener(ConfigurationChangeListener):
    def on_configuration_change(self, configuration):
        # Reload configuration and restart services
        self.reload_configuration()
        return True

# Register listener
config_service.add_config_change_listener(MyConfigListener())
```

### Configuration Validation

```python
from ggcommons.validation import ConfigurationValidator, ConfigurationValidationException

try:
    ConfigurationValidator.validate(config)
    print("Configuration is valid")
except ConfigurationValidationException as e:
    print(f"Configuration validation failed: {e}")
    for error in e.validation_errors:
        print(f"  - {error['message']} at {error['path']}")
```

## Local Development and Testing

Local testing is possible with the setup of a local MQTT server that acts as a local instance of Greengrass IPC. The MQTT messaging provider also allows for mocking request/response patterns between components to validate behavior.

> :warning: If your component has a hard dependency on other components, ensure that these components are also running in "local" mode.

### Quickstart

#### Set up local MQTT broker

1. Install [Docker Engine](https://docs.docker.com/engine/install/).
2. Install [MQTTX desktop (and optionally CLI) client](https://mqttx.app/downloads).
3. Run `docker run -d --name emqx -p 1883:1883 -p 8083:8083 -p 8084:8084 -p 8883:8883 -p 18083:18083 emqx/emqx:latest`
4. Open the MQTTX desktop app and connect to `localhost`.

#### Run your component locally

1. For STANDALONE mode with dual broker support:
```bash
python3 main.py -m STANDALONE standalone-messaging.json -c FILE config.json -t my-device
```

2. Navigate to the MQTTX desktop app and subscribe to `heartbeat/+/+` and you should see your component's heartbeat messages.
3. If your component is configured to publish messages, subscribe to the relevant topics to view them.
4. To send messages to your component, publish messages to the relevant topics using MQTTX.

## Testing with Enhanced Architecture

### Unit Testing with Mock Services

```python
import unittest
from unittest.mock import Mock
from ggcommons.interfaces import IMessagingService, IConfigurationService
from ggcommons.di import ServiceRegistry

class TestMyComponent(unittest.TestCase):
    def setUp(self):
        # Create mock services
        self.mock_messaging = Mock(spec=IMessagingService)
        self.mock_config = Mock(spec=IConfigurationService)
        
        # Create service registry with mocks
        self.registry = ServiceRegistry()
        self.registry.register(IMessagingService, self.mock_messaging)
        self.registry.register(IConfigurationService, self.mock_config)
        
        # Create component with mocked services
        self.component = MyComponent(self.registry)
        
    def test_send_data(self):
        # Test component behavior
        self.component.send_data({"value": 42})
        
        # Verify mock was called correctly
        self.mock_messaging.publish.assert_called_once()
```

## Migration Guide

The enhanced ggcommons maintains full backward compatibility. You can migrate gradually:

### Phase 1: Update Initialization
```python
# Old way (still works)
# args, config_manager, heartbeat = ggcommons.init("MyComponent", parser)

# New way
from ggcommons import GGCommonsBuilder
ggcommons = GGCommonsBuilder.create("MyComponent").with_args(args).build()
```

### Phase 2: Use Service Interfaces
```python
# Access services through interfaces
messaging = ggcommons.get_service(IMessagingService)
config = ggcommons.get_service(IConfigurationService)
```

### Phase 3: Adopt Builder Patterns
```python
# Use builders for object construction
from ggcommons.messaging import MessageBuilder
message = MessageBuilder.create("data", "1.0") \
    .with_payload(data) \
    .with_config(config_manager) \
    .build()
```

## Deployment Options

### AWS IoT Greengrass (Traditional)
- Full native integration with Greengrass v2 runtime
- Uses Greengrass IPC for inter-component communication
- Automatic device provisioning and management

### STANDALONE Mode (NEW!)
- **Kubernetes**: Deploy as pods with ConfigMaps and Secrets
- **Docker**: Run in containers with volume mounts for configuration
- **Container Runtimes**: ECS, EKS, AKS, GKE, or any container platform
- **Edge Computing**: Industrial IoT gateways, edge servers
- **Development**: Local development without Greengrass installation

## Requirements

- **Python**: 3.8 or higher
- **AWS IoT Greengrass**: 2.0 or higher (for Greengrass mode)
- **MQTT Broker**: Any MQTT 3.1.1 compatible broker (for STANDALONE mode)
- **pip**: For dependency management

## Dependencies

Key dependencies included:
- `awsiotsdk` - AWS IoT Device SDK for Python
- `paho-mqtt` - Eclipse Paho MQTT Client
- `jsonschema` - JSON Schema validation (optional)
- `psutil` - System and process utilities

## Support and Contributing

### Documentation
- [Configuration System](doc/configuration.md) - Multi-source configuration management
- [Messaging System](doc/messaging.md) - IPC and MQTT communication
- [Metrics System](doc/metric-emission.md) - Metrics collection and emission
- [Heartbeat System](doc/heartbeat.md) - Component health monitoring
- [Logging System](doc/logging.md) - Structured logging configuration
- [Command Line Options](doc/command-line-options.md) - CLI reference

### Getting Help
- Review the documentation for detailed configuration options
- Check the migration guide for upgrading existing components
- Enable DEBUG logging for troubleshooting: `"logging": {"level": "DEBUG"}`

### Best Practices

#### Configuration
- **Greengrass Mode**: Use Greengrass deployment configuration for production
- **STANDALONE Mode**: Use file-based configuration with ConfigMaps/Secrets in K8s
- Implement configuration change listeners for dynamic updates
- Leverage template variables for environment-specific configuration
- Enable configuration validation to catch errors early

#### Messaging
- **Dual Subscriptions**: In STANDALONE mode, you can subscribe to the same topic on both local and IoT Core
- **Authentication**: Use certificates for production, username/password for development
- **Topic Design**: Design topics to work across both Greengrass IPC and MQTT
- **Blocking Connections**: Connections and subscriptions wait for confirmation before proceeding

#### Monitoring
- Monitor component health through heartbeat metrics
- Use structured logging with appropriate log levels
- Configure metrics emission for your target environment (CloudWatch, local logs, etc.)

#### Deployment
- **Development**: Use STANDALONE mode with local MQTT broker
- **Production**: Choose between Greengrass or STANDALONE based on your infrastructure
- **Hybrid**: Run some components in Greengrass, others in K8s with STANDALONE mode

#### Testing
- Use mock services for unit testing
- Test configuration validation with invalid configs
- Verify builder pattern validation and error handling
- Test service interface contracts

## License

Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
SPDX-License-Identifier: Apache-2.0