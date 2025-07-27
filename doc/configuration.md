# Configuration System Documentation

## 1. Overview

The configuration system in ggcommons-java-lib provides a flexible, multi-source configuration management framework for Greengrass components. It supports loading configuration from various sources, template variable substitution, and runtime configuration changes. The system is designed to handle both ggcommons framework settings and application-specific configuration through a unified interface.

Key features include:
- Multiple configuration sources (file, environment, Greengrass deployment, IoT Shadow)
- Template variable substitution with component and tag values
- Runtime configuration change notifications
- Separation of framework and application configuration
- Instance-based configuration for multi-instance components

## 2. Configuration Sources

The configuration system supports multiple sources, specified via command line arguments:

### File Source (`FILE`)
```bash
-c FILE [file_path]
```
- Loads configuration from a JSON file
- Default: looks for configuration file in current directory
- Supports absolute and relative paths
- Best for development and testing

### Environment Source (`ENV`)
```bash
-c ENV [env_var_name]
```
- Loads configuration from environment variable
- Default variable: `GGCOMMONS_CONFIG`
- Configuration must be valid JSON string
- Useful for containerized deployments

### Greengrass Deployment Source (`GG_CONFIG`)
```bash
-c GG_CONFIG [component_name] [config_key]
```
- Loads from Greengrass deployment configuration
- Default: uses current component's configuration
- Optional component_name: load from different component
- Optional config_key: extract specific configuration section
- Standard for production Greengrass deployments

### IoT Shadow Source (`SHADOW`)
```bash
-c SHADOW [shadow_name]
```
- Loads configuration from AWS IoT Device Shadow
- Default: uses unnamed (classic) shadow
- Configuration stored in shadow's desired state
- Enables remote configuration updates

### Component Configuration Source (`CONFIG_COMPONENT`)
```bash
-c CONFIG_COMPONENT
```
- Loads from dedicated configuration management component
- Centralized configuration for multiple components
- Advanced deployment pattern for complex systems

## 3. Configuration Structure

The configuration is organized into distinct sections:

### Framework Sections
These sections are managed by ggcommons and configure framework behavior:

- **`logging`**: Logging system configuration
- **`heartbeat`**: Component health monitoring configuration  
- **`metricEmission`**: Metrics collection and emission configuration
- **`tags`**: Component tagging for organization and templating

### Application Section
The `component` section is reserved for application-specific configuration:

- **`component.global`**: Configuration shared across all component instances
- **`component.instances`**: Array of instance-specific configurations

## 4. Template Variable System

The configuration system supports template variables that are automatically resolved:

### Built-in Variables
- **`{ComponentName}`**: Short component name (e.g., "MyComponent")
- **`{ComponentFullName}`**: Full component name with version (e.g., "com.example.MyComponent-1.0.0")
- **`{ThingName}`**: AWS IoT Thing name associated with the device

### Tag Variables
Any tag defined in the `tags` section can be used as a template variable:
```json
{
  "tags": {
    "site": "factory-1",
    "line": "assembly-a"
  }
}
```
These become available as `{site}` and `{line}` in other configuration values.

### Instance Variables
When processing instance configurations, additional variables are available:
- **`{InstanceId}`**: The ID of the current instance

## 5. Application Configuration Usage

### Accessing Configuration in Code

```java
// Get the ConfigManager instance
ConfigManager configManager = ggCommons.getConfigManager();

// Access global configuration
JsonObject globalConfig = configManager.getGlobalConfig();
String serverUrl = globalConfig.get("serverUrl").getAsString();

// Access instance-specific configuration
Collection<String> instanceIds = configManager.getInstanceIds();
for (String instanceId : instanceIds) {
    JsonObject instanceConfig = configManager.getInstanceConfig(instanceId);
    // Process instance configuration
}

// Access full configuration
JsonObject fullConfig = configManager.getFullConfig();
```

### Configuration Change Notifications

```java
// Implement configuration change listener
public class MyConfigListener implements ConfigurationChangeListener {
    @Override
    public boolean onConfigurationChanged() {
        // Handle configuration changes
        // Return true if handled successfully
        return true;
    }
}

// Register listener
configManager.addConfigChangeListener(new MyConfigListener());
```

### Template Resolution

```java
// Resolve template variables in configuration strings
String resolvedPath = configManager.resolveTemplate("/data/{ThingName}/{site}/logs");
// Result: "/data/device-001/factory-1/logs"
```

## 6. Sample Configurations

### Sample 1: Basic Single-Instance Component
```json
{
  "logging": {
    "level": "INFO"
  },
  "heartbeat": {
    "intervalSecs": 30,
    "targets": [{"type": "metric"}]
  },
  "tags": {
    "environment": "production",
    "region": "us-east-1"
  },
  "component": {
    "global": {
      "serverUrl": "https://api.{region}.example.com",
      "timeout": 5000,
      "retryAttempts": 3
    },
    "instances": [
      {
        "id": "main",
        "database": {
          "host": "db.{environment}.example.com",
          "port": 5432,
          "name": "myapp_{environment}"
        }
      }
    ]
  }
}
```

### Sample 2: Multi-Instance Data Collector
```json
{
  "logging": {
    "level": "DEBUG",
    "fileLogging": true,
    "logFilePath": "/var/log/{ComponentName}-{site}.log"
  },
  "tags": {
    "site": "factory-north",
    "department": "manufacturing"
  },
  "metricEmission": {
    "target": "messaging",
    "targetConfig": {
      "topic": "metrics/{site}/{ComponentName}",
      "destination": "ipc"
    }
  },
  "component": {
    "global": {
      "dataRetentionDays": 30,
      "compressionEnabled": true,
      "uploadInterval": 300
    },
    "instances": [
      {
        "id": "line-1",
        "source": {
          "type": "modbus",
          "host": "plc-line1.{site}.local",
          "port": 502,
          "unitId": 1
        },
        "publishTopic": "{ThingName}/{ComponentName}/{InstanceId}/data",
        "samplingRate": 1000
      },
      {
        "id": "line-2", 
        "source": {
          "type": "modbus",
          "host": "plc-line2.{site}.local",
          "port": 502,
          "unitId": 2
        },
        "publishTopic": "{ThingName}/{ComponentName}/{InstanceId}/data",
        "samplingRate": 2000
      }
    ]
  }
}
```

### Sample 3: OPC-UA Gateway Component
```json
{
  "logging": {
    "level": "INFO",
    "globalControl": true,
    "loggers": {
      "com.mycompany.opcua": "DEBUG"
    }
  },
  "heartbeat": {
    "intervalSecs": 60,
    "measures": {
      "cpu": true,
      "memory": true,
      "connections": true
    },
    "targets": [
      {"type": "metric"},
      {
        "type": "messaging",
        "config": {
          "topic": "health/{site}/{ComponentName}",
          "destination": "iot_core"
        }
      }
    ]
  },
  "tags": {
    "site": "plant-a",
    "area": "production",
    "criticality": "high"
  },
  "component": {
    "global": {
      "security": {
        "certificatePath": "/opt/certs/{site}-client.pem",
        "keyPath": "/opt/certs/{site}-client.key",
        "trustStorePath": "/opt/certs/ca-{area}.pem"
      },
      "reconnectInterval": 5000,
      "maxReconnectAttempts": 10
    },
    "instances": [
      {
        "id": "server-1",
        "connectionInfo": {
          "url": "opc.tcp://server1.{site}.local:4840",
          "security": {
            "mode": "SignAndEncrypt",
            "policy": "Basic256Sha256"
          }
        },
        "subscriptions": [
          {
            "id": "temperature-sensors",
            "nodeIds": ["ns=2;s=Temp1", "ns=2;s=Temp2"],
            "publishInterval": 1000,
            "publishTopic": "{ThingName}/{ComponentName}/{InstanceId}/temperature"
          }
        ]
      },
      {
        "id": "server-2",
        "connectionInfo": {
          "url": "opc.tcp://server2.{site}.local:4840",
          "security": {
            "mode": "None"
          }
        },
        "subscriptions": [
          {
            "id": "pressure-sensors",
            "nodeIds": ["ns=3;s=Pressure1"],
            "publishInterval": 2000,
            "publishTopic": "{ThingName}/{ComponentName}/{InstanceId}/pressure"
          }
        ]
      }
    ]
  }
}
```

### Sample 4: Development Configuration with File Source
```json
{
  "logging": {
    "level": "TRACE",
    "format": "%d{HH:mm:ss.SSS} [%level] %logger{36} - %msg%n",
    "fileLogging": true,
    "logFilePath": "./logs/dev-{ComponentName}.log"
  },
  "heartbeat": {
    "intervalSecs": 5,
    "targets": [{"type": "messaging", "config": {"destination": "ipc"}}]
  },
  "metricEmission": {
    "target": "log",
    "targetConfig": {
      "logFileName": "./logs/metrics-{ComponentName}.log",
      "maxFileSize": "10MB"
    }
  },
  "tags": {
    "environment": "development",
    "developer": "john.doe"
  },
  "component": {
    "global": {
      "debugMode": true,
      "mockExternalServices": true,
      "dataDirectory": "./data/{developer}"
    },
    "instances": [
      {
        "id": "test-instance",
        "simulationMode": true,
        "dataSource": "mock",
        "outputPath": "./output/{InstanceId}"
      }
    ]
  }
}
```

## 7. Best Practices

### Configuration Organization
- Use the `global` section for settings shared across all instances
- Use `instances` for instance-specific configuration
- Leverage template variables for environment-specific values
- Group related settings into nested objects

### Template Variables
- Use descriptive tag names that reflect your deployment structure
- Avoid spaces and special characters in tag values used as templates
- Test template resolution in development environments
- Document custom template variables for your team

### Configuration Management
- Use file source for development and testing
- Use Greengrass deployment configuration for production
- Consider shadow source for remote configuration updates
- Implement configuration validation in your application

### Change Handling
- Always implement configuration change listeners for dynamic updates
- Validate configuration changes before applying them
- Provide fallback behavior for invalid configurations
- Log configuration changes for troubleshooting

### Security Considerations
- Avoid storing sensitive data directly in configuration
- Use AWS Secrets Manager or similar for credentials
- Validate all configuration inputs to prevent injection attacks
- Restrict file permissions on configuration files

## 8. Troubleshooting

### Common Issues
- **Configuration not loading**: Check source specification and file permissions
- **Template variables not resolving**: Verify tag definitions and variable syntax
- **Instance configuration not found**: Check instance ID spelling and array structure
- **Changes not applied**: Ensure configuration change listeners are registered

### Debugging Configuration
- Enable DEBUG logging for `com.aws.proserve.ggcommons.config` package
- Use `getFullConfig()` to inspect the complete loaded configuration
- Test template resolution with `resolveTemplate()` method
- Verify configuration source with `configProvider.getConfigSource()`

### Validation
- Validate JSON syntax before deployment
- Test configuration with different template variable values
- Verify all required configuration sections are present
- Check for circular dependencies in template variables