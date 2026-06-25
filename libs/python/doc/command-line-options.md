# AWS ProServe Greengrass Commons Command Line Options

This document describes the available command line options provided by the AWS ProServe Greengrass Commons Python library.

## Core Options

### -h, --help
* **Description**: Displays the help message with information about available command line options
* **Usage**: `-h` or `--help`
* **Required**: No
* **Dependencies**: None
* **Effect**: Shows usage information and exits the program

### -c, --config
* **Description**: Specifies the configuration source and related parameters for the component
* **Usage**: `-c <source> [additional args]` or `--config <source> [additional args]`
* **Required**: No (defaults to "GG_CONFIG" if not specified)
* **Dependencies**: None
* **Values**: One of the following sources with optional additional parameters:
  * `FILE [file_path]` - Load configuration from a file
  * `ENV [env_var_name]` - Load configuration from environment variable
  * `SHADOW [shadow_name]` - Load configuration from AWS IoT Device Shadow
  * `GG_CONFIG [component_name] [config_key]` - Load configuration from Greengrass component configuration (default)
  * `CONFIG_COMPONENT` - Load configuration from a configuration component
* **Effect**: Determines how the component obtains its configuration settings

### --platform
* **Description**: Selects the deployment platform — the primary runtime axis (DESIGN-core §2/§3).
  A platform is a named profile (a table of per-subsystem defaults).
* **Usage**: `--platform <platform>`
* **Required**: No (defaults to `auto`, which auto-detects from the environment)
* **Dependencies**: None
* **Values**: One of:
  * `GREENGRASS` - on an AWS IoT Greengrass v2 Nucleus (derives the `IPC` transport)
  * `HOST` - a plain host / container without a Nucleus (derives the `MQTT` transport)
  * `KUBERNETES` - declared but not yet wired (Phase 1); selecting it fails fast
  * `auto` (default) - detect from environment: Nucleus env signals → GREENGRASS;
    projected service-account token / `KUBERNETES_SERVICE_HOST` → KUBERNETES; else HOST
* **Effect**: Picks the per-subsystem defaults (including the default config source and transport)

### --transport
* **Description**: Selects the messaging transport — the secondary runtime axis (DESIGN-core §2).
* **Usage**: `--transport <transport> [messaging_config_path]`
* **Required**: No (defaults to the value derived from the resolved platform)
* **Dependencies**: `IPC` is valid **only** with `--platform GREENGRASS` (the IPC lock); any other
  combination is rejected at startup.
* **Values**: One of:
  * `IPC` - native Greengrass Nucleus IPC
  * `MQTT <messaging_config_path>` - dual MQTT clients (local broker + AWS IoT Core); the JSON
    messaging-config path is required when the MQTT provider is actually built
* **Effect**: Determines the messaging architecture

> **Removed:** the legacy `-m/--mode` flag is gone. `-m GREENGRASS` becomes `--platform GREENGRASS`;
> the old `-m STANDALONE <path>` becomes `--platform HOST --transport MQTT <path>`. Passing
> `-m`/`--mode` now errors with guidance to the new flags.

#### GREENGRASS platform (IPC transport)
- Uses native Greengrass v2 IPC communication
- Managed by Greengrass runtime
- Single messaging channel for inter-component communication
- Automatic device provisioning and management

#### HOST platform with MQTT transport (Container-Ready)
- **Dual MQTT clients**: Local broker + AWS IoT Core connectivity
- **Container deployment**: Perfect for Kubernetes, Docker, ECS, etc.
- **Independent subscriptions**: Subscribe to same topic on both clients
- **Flexible authentication**: Certificate-based and username/password
- **Configuration file required**: JSON file defining both MQTT connections
- **Blocking connections**: Ensures reliable startup with connection confirmation

### -t, --thing
* **Description**: Specifies the AWS IoT thing name for the component
* **Usage**: `-t <thing_name>` or `--thing <thing_name>`
* **Required**: No
* **Dependencies**: None
* **Values**: Any valid AWS IoT thing name
* **Effect**: Associates the component with a specific AWS IoT thing identity

## Usage Examples

### Basic GREENGRASS platform (IPC transport, auto-derived)
```bash
python3 main.py -c GG_CONFIG -t my-thing-name
```

### File-based Configuration
```bash
python3 main.py -c FILE config.json -t my-thing-name
```

### Environment Variable Configuration
```bash
export GGCOMMONS_CONFIG='{"logging": {"level": "DEBUG"}}'
python3 main.py -c ENV -t my-thing-name
```

### HOST platform with Dual MQTT
```bash
python3 main.py --platform HOST --transport MQTT messaging-config.json -c FILE config.json -t my-thing-name
```

### IoT Shadow Configuration
```bash
python3 main.py -c SHADOW my-config-shadow -t my-thing-name
```

## Component Initialization

### Enhanced Builder Pattern
```python
from ggcommons.builders import GGCommonsBuilder

# Basic initialization
ggcommons = GGCommonsBuilder.create("com.example.MyComponent") \
    .with_args(sys.argv[1:]) \
    .build()

# Initialization with custom argument parser
import argparse
parser = argparse.ArgumentParser()
parser.add_argument('--custom-option', help='Custom application option')

ggcommons = GGCommonsBuilder.create("com.example.MyComponent") \
    .with_args(sys.argv[1:]) \
    .with_app_options(parser) \
    .build()
```

### Legacy Initialization (Still Supported)
```python
import ggcommons
import argparse

def main():
    parser = argparse.ArgumentParser()
    
    # Initialize GGCommons (legacy method)
    args, config_manager, heartbeat = ggcommons.init(
        "com.example.MyComponent", 
        parser
    )
    
    # Your component logic here
    start_application(config_manager)
```

## Configuration Processing Flow

1. **Argument Parsing**: Command line arguments are parsed using Python's argparse
2. **Help Display**: Help is displayed if -h/--help option is present
3. **Configuration Source**: Configuration source is determined (default or from -c option)
4. **Platform/Transport Resolution**: The platform and transport are resolved (from `--platform`/`--transport`, else env auto-detection, else profile defaults), then messaging is initialized on the resolved transport
5. **Service Registry**: Dependency injection container is created and populated
6. **Configuration Manager**: Configuration manager is created and initialized
7. **Framework Services**: Metric emitter and heartbeat services are started

## MQTT transport configuration

When using the `MQTT` transport (e.g. the `HOST` platform), you must provide a separate messaging configuration file:

### Dual MQTT Configuration
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

### IoT Core Only Configuration
```json
{
  "messaging": {
    "iotCore": {
      "endpoint": "your-endpoint.iot.us-east-1.amazonaws.com",
      "port": 8883,
      "clientId": "my-device",
      "credentials": {
        "certPath": "/certs/device-cert.pem",
        "keyPath": "/certs/private-key.pem",
        "caPath": "/certs/root-ca.pem"
      }
    }
  }
}
```

## Advanced Usage

### Custom Application Options
```python
import argparse
from ggcommons.builders import GGCommonsBuilder

def main():
    # Create custom argument parser
    parser = argparse.ArgumentParser(description='My Custom Component')
    parser.add_argument('--data-source', help='Data source URL')
    parser.add_argument('--batch-size', type=int, default=100, help='Batch size for processing')
    parser.add_argument('--dry-run', action='store_true', help='Run in dry-run mode')
    
    # Initialize GGCommons with custom options
    ggcommons = GGCommonsBuilder.create("com.example.MyComponent") \
        .with_args(sys.argv[1:]) \
        .with_app_options(parser) \
        .build()
    
    # Access parsed arguments
    args = ggcommons.get_args()  # If this method exists
    
    # Your component logic here
    start_application(args.data_source, args.batch_size, args.dry_run)
```

### Environment-Specific Configurations
```bash
# Development
python3 main.py -c FILE dev-config.json -t dev-device

# Staging  
python3 main.py -c FILE staging-config.json -t staging-device

# Production (Greengrass)
python3 main.py -c GG_CONFIG -t prod-device

# Production (Kubernetes)
python3 main.py --platform HOST --transport MQTT /config/messaging.json -c FILE /config/app-config.json -t prod-device
```

## Best Practices

### Command Line Design
- Always provide help text for custom options when extending the library
- Use descriptive option names that clearly indicate their purpose
- Provide sensible defaults for optional parameters
- Validate configuration sources and messaging settings for production deployments

### Configuration Management
- Use file-based configuration for development and testing
- Use Greengrass deployment configuration for production Greengrass deployments
- Use environment variables for containerized deployments
- Consider IoT Shadow configuration for remote configuration updates

### Platform / Transport Selection
- **`--platform GREENGRASS` (IPC transport)**: Use for traditional Greengrass deployments
- **`--platform HOST` (dual-MQTT transport)**: Use for Kubernetes, Docker, or any container runtime
- Test both combinations during development to ensure compatibility
- Document the platform/transport requirements for your component

### Security Considerations
- Protect configuration files containing sensitive information
- Use appropriate file permissions for certificate files
- Avoid hardcoding credentials in command line arguments
- Use environment variables or secure configuration sources for sensitive data

## Error Handling

### Common Command Line Errors
- **Invalid configuration source**: Check source name spelling and availability
- **Missing MQTT messaging-config file**: Ensure the messaging configuration file exists and is readable
- **Invalid thing name**: Verify thing name follows AWS IoT naming conventions
- **Permission errors**: Check file permissions for configuration and certificate files

### Debugging Command Line Issues
```bash
# Enable debug logging to see argument parsing
python3 main.py -c FILE config.json -t my-thing --help

# Test configuration loading
python3 -c "
import sys
from ggcommons.builders import GGCommonsBuilder
try:
    ggcommons = GGCommonsBuilder.create('test').with_args(sys.argv[1:]).build()
    print('Configuration loaded successfully')
except Exception as e:
    print(f'Configuration error: {e}')
" -c FILE config.json
```

## Migration from Java

The Python command line interface is designed to be identical to the Java version:

### Java Command
```bash
java -jar component.jar --platform HOST --transport MQTT messaging.json -c FILE config.json -t my-device
```

### Python Equivalent
```bash
python3 main.py --platform HOST --transport MQTT messaging.json -c FILE config.json -t my-device
```

This ensures seamless migration between Java and Python implementations with identical deployment scripts and configuration management.