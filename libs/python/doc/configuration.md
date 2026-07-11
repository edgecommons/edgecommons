# Configuration System Documentation

## 1. Overview

The configuration system in EdgeCommons Python library provides a flexible, multi-source configuration management framework for Greengrass components. It supports loading configuration from various sources, template variable substitution, and runtime configuration changes. The system is designed to handle both edgecommons framework settings and application-specific configuration through a unified interface.

Key features include:
- Multiple configuration sources (file, environment, Greengrass deployment, IoT Shadow)
- Template variable substitution with component and tag values
- Runtime configuration change notifications
- Separation of framework and application configuration
- Instance-based configuration for multi-instance components
- Side-effect-free pre-commit validators with atomic generation activation

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
- Default variable: `EDGECOMMONS_CONFIG`
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

The rendezvous with the config server rides the UNS command grammar (Flow A of the config
addressing, UNS-CANONICAL-DESIGN §4.3 / D-U19):

| Flow | Topic | Direction |
|---|---|---|
| get-configuration (request/reply) | `ecv1/{device}/config/main/cmd/get-configuration` | component -> config server |
| set-config push (fire-and-forget `cmd`, no `reply_to`) | `ecv1/{device}/{component}/main/cmd/set-config` | config server -> component |

- **`config` is a reserved-by-convention logical component name** — the config server is the
  *sole subscriber* of the `get-configuration` rendezvous under it. Do not name a component
  `config`.
- `{device}` is the resolved thing name and `{component}` the short component name, both passed
  through the normative token sanitizer (`/ \ + #`, control characters and `..` become `_`).
- **The requester self-identifies in the request body** with `{"component": "<short name>"}`.
  The GET runs during config bootstrap — *before* the `ConfigManager` (and the component
  identity) exists — so the envelope carries **no** `identity` element; the server must route on
  the body field. The server replies via the envelope's `reply_to` with the configuration as the
  message body. The request keeps the framework request deadline and is retried up to 3 times
  (a fresh request per attempt).
- **A pushed `set-config`** is a notification-style command (a `cmd` without `reply_to`)
  delivered to the component's *own* inbox; its body is the complete new configuration, applied
  exactly like a hot reload (schema-validated, reject-and-keep).

**Server side (convention, not implemented by this library):** an external config-manager
component must subscribe to `ecv1/{device}/config/main/cmd/get-configuration`, reply to each
request with the requesting component's configuration as the body, and push configuration
changes as `set-config` commands to each component's inbox
`ecv1/{device}/{component}/main/cmd/set-config`.

## 3. Configuration Structure

The configuration is organized into distinct sections:

### Framework Sections
These sections are managed by edgecommons and configure framework behavior:

- **`logging`**: Logging system configuration
- **`heartbeat`**: Component health monitoring — a UNS `state` keepalive plus system measures as the `sys` metric (see [heartbeat.md](heartbeat.md))
- **`hierarchy`** / **`identity`** / **`topic`**: the UNS enterprise hierarchy (`hierarchy.levels`, last level = the device), the per-level identity values (all levels except the last; the device value is always the resolved thing name), and `topic.includeRoot` (default `false`; effective only with a multi-level hierarchy)
- **`messaging`**: `requestTimeoutSeconds` (default 30, `0` disables — the framework request deadline); generic component messaging config does not define MQTT Last-Will
- **`metricEmission`**: Metrics collection and emission configuration
- **`tags`**: Component tagging for organization and templating

`logging.publish` is the optional structured log-bus publisher. When enabled, Python publishes
`edgecommons.log.v1` records to `ecv1/{device}/{component}/main/log/{level}` through the reserved `log`
class seam. It is disabled by default; `captureNative` observes stdlib `logging` records and
`captureConsole` requests stdout/stderr capture where supported.

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

```python
from edgecommons import EdgeCommonsBuilder

# Initialize EdgeCommons
edgecommons = EdgeCommonsBuilder.create("com.example.MyComponent") \
    .with_args(args) \
    .build()

# Get the concrete configuration manager
config_service = edgecommons.get_config_manager()

# Access global configuration
global_config = config_service.get_global_config()
server_url = global_config.get("serverUrl")

# Access instance-specific configuration
instance_ids = config_service.get_instance_ids()
for instance_id in instance_ids:
    instance_config = config_service.get_instance_config(instance_id)
    # Process instance configuration

# Access full configuration
full_config = config_service.get_full_config()
```

### Configuration Change Notifications

```python
from edgecommons.config.manager.configuration_change_listener import ConfigurationChangeListener

class MyConfigListener(ConfigurationChangeListener):
    def on_configuration_change(self, configuration):
        # Handle configuration changes
        # Return True if handled successfully
        return True

# Register listener
config_service.add_config_change_listener(MyConfigListener())
```

Applied listeners run only after an accepted generation is current. They receive a defensive copy;
a listener cannot recursively apply another generation from inside its callback.

### Pre-commit candidate validation

Register component-specific validators on `EdgeCommonsBuilder`, before the selected config provider
loads or starts a watcher/subscription:

```python
from edgecommons import (
    ConfigurationValidationPhase,
    ConfigurationValidationResult,
    EdgeCommonsBuilder,
)

def validate_cameras(candidate, redacted_current, phase):
    instances = candidate.get("component", {}).get("instances", [])
    if not any(instance.get("enabled", True) for instance in instances):
        return ConfigurationValidationResult.reject(
            "NO_ENABLED_CAMERA", "at least one enabled camera is required"
        )
    return ConfigurationValidationResult.accept()

gg = (
    EdgeCommonsBuilder.create("com.example.CameraAdapter")
    .configuration_validator("camera", validate_cameras)
    .configuration_validation_timeout(5.0)
    .build()
)
```

For every `INITIAL` load and `RELOAD`, the canonical schema is checked first. Validators then receive
independent deep copies of the candidate and the redacted prior generation (`None` for `INITIAL`). The
deadline is one overall deadline for all validators: 5 seconds by default and at most 60 seconds.
Rejection, timeout, or failure retains the exact prior generation and invokes neither applied listeners
nor effective-config publication. Successful activation installs one complete snapshot atomically;
`get_generation()` starts at 1 and increments only for accepted generations. Stable errors from the
last rejection are available through `get_last_candidate_validation_errors()`.

### Template Resolution

```python
# Resolve template variables in configuration strings
resolved_path = config_service.resolve_template("/data/{ThingName}/{site}/logs")
# Result: "/data/device-001/factory-1/logs"
```

## 6. Sample Configurations

### Sample 1: Basic Single-Instance Component
```json
{
  "logging": {
    "level": "INFO"
  },
  "heartbeat": {
    "intervalSecs": 30
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
    "fileLogging": {
      "enabled": true,
      "filePath": "/var/log/{ComponentName}-{site}.log"
    }
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
      "edgecommons.opcua": "DEBUG"
    }
  },
  "heartbeat": {
    "intervalSecs": 60,
    "measures": {
      "cpu": true,
      "memory": true
    },
    "destination": "northbound"
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
    "level": "DEBUG",
    "python_format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "./logs/dev-{ComponentName}.log"
    }
  },
  "heartbeat": {
    "intervalSecs": 5
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

## 8. Configuration Validation

The EdgeCommons configuration system includes automatic validation using JSON Schema to ensure configuration correctness and provide better error messages.

### Validation Features
- Required and optional properties validation
- Data type checking
- Valid value ranges and enums
- Default value application
- Detailed error messages with property paths

### Using Validation
```python
from edgecommons.validation import ConfigurationValidator, ConfigurationValidationException

try:
    ConfigurationValidator.validate(config)
    print("Configuration is valid")
except ConfigurationValidationException as e:
    print(f"Configuration validation failed: {e}")
    for error in e.validation_errors:
        print(f"  - {error['message']} at {error['path']}")
```

## 9. Troubleshooting

### Common Issues
- **Configuration not loading**: Check source specification and file permissions
- **Template variables not resolving**: Verify tag definitions and variable syntax
- **Instance configuration not found**: Check instance ID spelling and array structure
- **Changes not applied**: Ensure configuration change listeners are registered
- **Validation errors**: Check configuration against expected schema

### Debugging Configuration
- Enable DEBUG logging for `edgecommons.config` package
- Use `get_full_config()` to inspect the complete loaded configuration
- Test template resolution with `resolve_template()` method
- Verify configuration source loading
- Validate configuration structure before deployment

### Validation
- Validate JSON syntax before deployment
- Test configuration with different template variable values
- Verify all required configuration sections are present
- Check for circular dependencies in template variables
