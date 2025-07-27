# GGCommons Java Library

A comprehensive Java library for building AWS IoT Greengrass components with built-in configuration management, messaging, metrics, heartbeat monitoring, and logging capabilities.

## Purpose

GGCommons simplifies the development of AWS IoT Greengrass components by providing a unified framework that handles common operational concerns, allowing developers to focus on their core business logic. The library abstracts away the complexity of Greengrass integration while providing enterprise-grade features for monitoring, configuration management, and inter-component communication.

## Key Capabilities

### 🔧 Configuration Management
- **Multiple Sources**: Load configuration from files, environment variables, Greengrass deployment, or IoT Device Shadows
- **Template Variables**: Dynamic value substitution using component, thing, and custom tag variables
- **Runtime Updates**: Hot configuration reloading without component restart
- **Multi-Instance Support**: Manage configuration for components with multiple instances

[📖 Configuration Documentation](doc/configuration.md)

### 📨 Messaging System
- **Dual Protocol Support**: Seamless switching between Greengrass IPC and MQTT
- **Request-Response Pattern**: Built-in support for synchronous communication
- **Topic Filtering**: Advanced subscription patterns with wildcards
- **Message Serialization**: Automatic JSON serialization with metadata headers

[📖 Messaging Documentation](doc/messaging.md)

### 📊 Metrics Collection & Emission
- **Multiple Targets**: Send metrics to CloudWatch, local logs, or messaging topics
- **EMF Format**: Embedded Metric Format for CloudWatch compatibility
- **Batching & Rotation**: Efficient metric batching with configurable file rotation
- **Custom Dimensions**: Automatic component and thing name dimensions

[📖 Metrics Documentation](doc/metric-emission.md)

### 💓 Health Monitoring
- **System Metrics**: CPU, memory, disk, thread, and file descriptor monitoring
- **Configurable Intervals**: Adjustable heartbeat frequency
- **Multiple Outputs**: Send health data via metrics or messaging
- **Resource Tracking**: Built-in system resource monitoring

[📖 Heartbeat Documentation](doc/heartbeat.md)

### 📝 Logging System
- **Log4j2 Integration**: Built on industry-standard logging framework
- **Dynamic Configuration**: Runtime log level and output adjustments
- **Structured Logging**: Consistent formatting with template variable support

[📖 Logging Documentation](doc/logging.md)

## Quick Start

### 1. Add Dependency

Add the GGCommons library to your Maven project:

```xml
<dependency>
    <groupId>com.aws.proserve</groupId>
    <artifactId>ggcommons</artifactId>
    <version>1.2.1-SNAPSHOT</version>
</dependency>
```

### 2. Basic Component Structure

```java
public class MyComponent {
    private GGCommons ggCommons;
    private ConfigManager configManager;
    
    public static void main(String[] args) {
        new MyComponent().run(args);
    }
    
    public void run(String[] args) {
        // Initialize GGCommons with component name and arguments
        ggCommons = new GGCommons("com.example.MyComponent", args);
        configManager = ggCommons.getConfigManager();
        
        // Your component logic here
        startApplication();
    }
    
    private void startApplication() {
        // Access configuration
        JsonObject globalConfig = configManager.getGlobalConfig();
        
        // Process each configured instance
        for (String instanceId : configManager.getInstanceIds()) {
            JsonObject instanceConfig = configManager.getInstanceConfig(instanceId);
            // Start instance-specific processing
        }
    }
}
```

### 3. Configuration File Example

Create a configuration file (e.g., `config.json`):

```json
{
  "logging": {
    "level": "INFO",
    "fileLogging": true,
    "logFilePath": "/var/log/{ComponentName}.log"
  },
  "heartbeat": {
    "intervalSecs": 30,
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

### 4. Run Your Component

```bash
# Development with file configuration
java -jar mycomponent.jar -c FILE ./config.json -t my-thing-name

# Production with Greengrass deployment configuration
java -jar mycomponent.jar -c GG_CONFIG -t my-thing-name
```

## Command Line Options

GGCommons supports several command line options for configuration and messaging:

### Configuration Source (`-c, --config`)
- `FILE [path]` - Load from JSON file (default: current directory)
- `ENV [var_name]` - Load from environment variable (default: GGCOMMONS_CONFIG)
- `GG_CONFIG [component] [key]` - Load from Greengrass deployment (default)
- `SHADOW [name]` - Load from IoT Device Shadow
- `CONFIG_COMPONENT` - Load from configuration management component

### Messaging Provider (`-m, --messaging`)
- `IPC` - Use Greengrass IPC (default)
- `MQTT <host> <port>` - Use MQTT broker for development

### Thing Name (`-t, --thing`)
- Specify the AWS IoT Thing name (optional, auto-detected in Greengrass)

## Advanced Features

### Messaging with Request-Response

```java
// Subscribe to requests
MessagingClient.subscribe("requests/process", this::handleRequest, 1);

// Send request and wait for response
Message request = Message.buildFromConfig("ProcessData", "1.0", payload, configManager);
Message response = MessagingClient.request("requests/process", request)
    .get(5000, TimeUnit.MILLISECONDS);
```

### Custom Metrics

```java
// Define a custom metric
Metric metric = new Metric("data_processed");
metric.addMeasure(new Measure("count", "Count", 1));
metric.addMeasure(new Measure("size_bytes", "Bytes", 1));
MetricEmitter.defineMetric(metric);

// Emit metric values
Map<String, Float> values = Map.of(
    "count", 100.0f,
    "size_bytes", 1024.0f
);
MetricEmitter.emitMetric("data_processed", values);
```

### Configuration Change Handling

```java
public class MyConfigListener implements ConfigurationChangeListener {
    @Override
    public boolean onConfigurationChanged() {
        // Reload configuration and restart services
        reloadConfiguration();
        return true;
    }
}

// Register listener
configManager.addConfigChangeListener(new MyConfigListener());
```

## Example Components

Learn from these real-world examples:

### 1. Java Component Skeleton
**Location**: `c:\users\mbreissi\src\java\java-component-skeleton`

A simple sample component demonstrating basic GGCommons usage patterns:
- Basic configuration management
- Simple messaging patterns
- Metric emission examples
- Standard component lifecycle

### 2. GGOpcUaBridge
**Location**: `c:\users\mbreissi\src\java\GGOpcUaBridge`

A production-grade component for connecting to OPC-UA servers:
- Multi-instance configuration for multiple OPC-UA servers
- Complex subscription management
- Advanced error handling and reconnection logic
- Performance monitoring and metrics
- Security configuration examples

## Building and Packaging

### Maven Build
```bash
# Build library
mvn clean package

# Skip tests during build
mvn clean package -DskipTests

# Install to local repository
mvn clean install
```

### Shaded JAR
The library uses Maven Shade Plugin to create a self-contained JAR with all dependencies included, suitable for Greengrass deployment.

## Requirements

- **Java**: 11 or higher
- **AWS IoT Greengrass**: 2.0 or higher (for production deployment)
- **Maven**: 3.6 or higher (for building)

## Dependencies

Key dependencies included:
- AWS IoT Device SDK for Java
- Apache Log4j2 for logging
- Eclipse Paho MQTT Client
- Google Gson for JSON processing
- AWS SDK for CloudWatch

## Support and Contributing

### Documentation
- [Configuration System](doc/configuration.md) - Multi-source configuration management
- [Messaging System](doc/messaging.md) - IPC and MQTT communication
- [Metrics System](doc/metric-emission.md) - Metrics collection and emission
- [Heartbeat System](doc/heartbeat.md) - Component health monitoring
- [Logging System](doc/logging.md) - Structured logging configuration
- [Command Line Options](doc/command-line-options.md) - CLI reference

### Getting Help
- Review the example components for implementation patterns
- Check the documentation for detailed configuration options
- Enable DEBUG logging for troubleshooting: `"logging": {"level": "DEBUG"}`

### Best Practices
- Use file-based configuration for development and testing
- Use Greengrass deployment configuration for production
- Implement configuration change listeners for dynamic updates
- Monitor component health through heartbeat metrics
- Use structured logging with appropriate log levels
- Leverage template variables for environment-specific configuration

## License

Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
SPDX-License-Identifier: Apache-2.0