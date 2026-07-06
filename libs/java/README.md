# EdgeCommons Java Library

The **canonical** implementation of the Greengrass Commons library (Maven artifact
`com.mbreissi.edgecommons:edgecommons`) for building AWS IoT Greengrass v2 components with built-in
configuration management, messaging, metrics, heartbeat, logging, credentials (encrypted vault),
parameters (externalized config), and telemetry streaming. It is one of four parallel
implementations (Java, Python, Rust, TypeScript); Java is the reference the others mirror. See the
monorepo root `README.md` for the ecosystem overview.

## Purpose

EdgeCommons simplifies the development of AWS IoT Greengrass components by providing a unified framework that handles common operational concerns, allowing developers to focus on their core business logic. The library abstracts away the complexity of Greengrass integration while providing enterprise-grade features for monitoring, configuration management, and inter-component communication.

**🚀 Run outside Greengrass** - With `--platform HOST` (or `KUBERNETES`) and `--transport MQTT`, run components outside of Greengrass with nearly full functionality! Perfect for Kubernetes, Docker, or any container runtime environment. Maintains dual connectivity to both local MQTT brokers and AWS IoT Core.

## Key Capabilities

### 🔧 Configuration Management
- **Multiple Sources**: Load configuration from files, environment variables, Greengrass deployment, or IoT Device Shadows
- **Template Variables**: Dynamic value substitution using component, thing, and custom tag variables
- **Runtime Updates**: Hot configuration reloading without component restart
- **Multi-Instance Support**: Manage configuration for components with multiple instances

[📖 Configuration Documentation](doc/configuration.md)

### 📨 Messaging System
- **Multi-Runtime Support**: Native Greengrass IPC (`--transport IPC`) or dual MQTT clients (`--transport MQTT`)
- **Dual MQTT Connectivity**: Simultaneous local broker and AWS IoT Core connections under `--platform HOST`/`KUBERNETES`
- **Request-Response Pattern**: Built-in support for synchronous communication
- **Topic Filtering**: Advanced subscription patterns with wildcards
- **Message Serialization**: Automatic JSON serialization with metadata headers
- **Certificate & Username Auth**: Support for both authentication methods on local brokers

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

Add the EdgeCommons library to your Maven project:

```xml
<dependency>
    <groupId>com.mbreissi.edgecommons</groupId>
    <artifactId>edgecommons</artifactId>
    <version>1.3.2-SNAPSHOT</version>
</dependency>
```

### 2. Basic Component Structure

```java
public class MyComponent {
    private EdgeCommons edgeCommons;
    private ConfigManager configManager;
    
    public static void main(String[] args) {
        new MyComponent().run(args);
    }
    
    public void run(String[] args) {
        // Construct via the builder (direct constructors are deprecated).
        edgeCommons = EdgeCommonsBuilder.create("com.example.MyComponent").withArgs(args).build();
        configManager = edgeCommons.getConfigManager();

        // Subsystem accessors (the newer ones return null unless their config section is present):
        var messaging   = edgeCommons.getMessaging();      // MessagingClient
        var metrics     = edgeCommons.getMetrics();        // MetricEmitter
        var credentials = edgeCommons.getCredentials();    // CredentialService or null
        var parameters  = edgeCommons.getParameters();     // ParameterService or null
        var streams     = edgeCommons.getStreams();        // StreamService or null

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
    "measures": { "cpu": true, "memory": true }
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
# Greengrass platform (default - auto-detected) - for AWS IoT Greengrass runtime
java -jar mycomponent.jar -c GG_CONFIG -t my-thing-name

# HOST platform - for Kubernetes, Docker, or any container runtime
java -jar mycomponent.jar --platform HOST --transport MQTT ./standalone-messaging.json -c FILE ./config.json -t my-thing-name
```

### 5. HOST Platform Messaging Configuration

Create a `standalone-messaging.json` file (the `--transport MQTT` payload) for non-Greengrass deployments:

```json
{
  "messaging": {
    "local": {
      "host": "localhost",
      "port": 1883,
      "clientId": "my-component-local",
      "credentials": {
        "username": "mqtt-user",
        "password": "mqtt-pass"
      }
    },
    "northbound": {
      "endpoint": "northbound.mqtt.example.com",
      "port": 8883,
      "clientId": "my-component-northbound",
      "credentials": {
        "certPath": "/path/to/device-cert.pem",
        "keyPath": "/path/to/private-key.pem",
        "caPath": "/path/to/root-ca.pem"
      }
    }
  }
}
```

## Command Line Options

EdgeCommons supports several command line options for configuration and messaging:

### Configuration Source (`-c, --config`)
- `FILE [path]` - Load from JSON file (default: current directory)
- `ENV [var_name]` - Load from environment variable (default: EDGECOMMONS_CONFIG)
- `GG_CONFIG [component] [key]` - Load from Greengrass deployment (the default on the GREENGRASS platform)
- `SHADOW [name]` - Load from IoT Device Shadow
- `CONFIG_COMPONENT` - Load from configuration management component

The default source comes from the resolved platform profile (GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).

### Platform (`--platform`)
- `GREENGRASS` - Greengrass runtime; uses Greengrass IPC by default
- `HOST` - bare host / Docker; uses MQTT by default
- `KUBERNETES` - Kubernetes; uses MQTT by default (declared now; full wiring lands in a later phase)
- `auto` - auto-detect the platform (default)

### Transport (`--transport`)
- `IPC` - Greengrass IPC (only valid on `--platform GREENGRASS`)
- `MQTT [config_file_path]` - dual MQTT clients (local broker + AWS IoT Core) for non-Greengrass environments
  - Enables deployment to Kubernetes, Docker, or any container runtime
  - Maintains connectivity to both local MQTT broker and AWS IoT Core
  - Nearly full functionality outside of Greengrass
- The default transport is derived from the platform (`GREENGRASS`⇒`IPC`, `HOST`/`KUBERNETES`⇒`MQTT`)

> **Migration:** the legacy `-m/--mode` flag has been removed. Use `-m GREENGRASS` → `--platform GREENGRASS`,
> and `-m STANDALONE <path>` → `--platform HOST --transport MQTT <path>`.

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
// Define a custom metric (use MetricBuilder; the direct Metric constructor is deprecated)
Metric metric = MetricBuilder.create("data_processed")
    .addMeasure("count", "Count", 1)
    .addMeasure("size_bytes", "Bytes", 1)
    .build();
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

## Example Component

A worked, runnable example component built on this library lives at
[`examples/java/`](../../examples/java) in this monorepo (the Java counterpart of the Python, Rust,
and TypeScript skeletons). It demonstrates configuration management, messaging (publish +
request/reply), metric emission, heartbeat, and the standard component lifecycle. Use the
`edgecommons` CLI (`edgecommons create-component -l JAVA …`) to scaffold a new component from the Java
template.

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

## Deployment Options

### AWS IoT Greengrass (Traditional)
- Full native integration with Greengrass v2 runtime
- Uses Greengrass IPC for inter-component communication
- Automatic device provisioning and management

### HOST / KUBERNETES Platform
- **Kubernetes**: Deploy as pods with ConfigMaps and Secrets
- **Docker**: Run in containers with volume mounts for configuration
- **Container Runtimes**: ECS, EKS, AKS, GKE, or any container platform
- **Edge Computing**: Industrial IoT gateways, edge servers
- **Development**: Local development without Greengrass installation

## Requirements

- **Java**: 25 (the library compiles to Java 25; the streaming subsystem uses the FFM/Panama
  native binding — run components with `--enable-native-access=ALL-UNNAMED`)
- **AWS IoT Greengrass**: 2.0 or higher (for the GREENGRASS platform)
- **MQTT Broker**: Any MQTT 3.1.1 compatible broker (for the HOST/KUBERNETES platforms / `--transport MQTT`)
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

#### Configuration
- **GREENGRASS platform**: Use Greengrass deployment configuration for production
- **HOST/KUBERNETES platform**: Use file-based configuration with ConfigMaps/Secrets in K8s
- Implement configuration change listeners for dynamic updates
- Leverage template variables for environment-specific configuration

#### Messaging
- **Dual Subscriptions**: Under `--transport MQTT`, you can subscribe to the same topic on both local and IoT Core
- **Authentication**: Use certificates for production, username/password for development
- **Topic Design**: Use the UNS grammar `ecv1/{device}/{component}/{instance}/{class}[/channel]` via `gg.getUns()` (builder + validator); it works identically over Greengrass IPC and MQTT. The `state`/`metric`/`cfg`/`log` classes are library-owned (a raw publish to them is rejected).

#### Monitoring
- Monitor component health through the UNS `state` keepalives — subscribe `ecv1/+/+/+/state` (system measures arrive as the `sys` metric via the configured metric target)
- Use structured logging with appropriate log levels
- Configure metrics emission for your target environment (CloudWatch, local logs, etc.)

#### Deployment
- **Development**: Use `--platform HOST --transport MQTT` with a local MQTT broker
- **Production**: Choose between the GREENGRASS and HOST/KUBERNETES platforms based on your infrastructure
- **Hybrid**: Run some components on Greengrass, others in K8s with `--platform KUBERNETES`/`HOST`

## License

Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
SPDX-License-Identifier: Apache-2.0
