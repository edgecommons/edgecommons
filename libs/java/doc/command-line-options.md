TODO: This file was GenAI generated and needs updating/corrections

# AWS ProServe Greengrass Commons Command Line Options

This document describes the available command line options provided by the AWS ProServe Greengrass Commons library.

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
* **Required**: No (default comes from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
* **Dependencies**: None
* **Values**: One of the following sources with optional additional parameters:
  * `FILE [file_path]` - Load configuration from a file
  * `ENV [env_var_name]` - Load configuration from environment variable
  * `SHADOW [shadow_name]` - Load configuration from AWS IoT Device Shadow
  * `GG_CONFIG [component_name] [config_key]` - Load configuration from Greengrass component configuration (the default on the GREENGRASS platform)
  * `CONFIG_COMPONENT` - Load configuration from a configuration component
* **Effect**: Determines how the component obtains its configuration settings

### --platform
* **Description**: Selects the runtime platform the component runs on
* **Usage**: `--platform <PLATFORM>`
* **Required**: No (defaults to `auto`, which auto-detects the platform)
* **Dependencies**: None
* **Values**: One of:
  * `GREENGRASS` - AWS IoT Greengrass runtime (default transport: `IPC`)
  * `HOST` - bare host / Docker / container runtime (default transport: `MQTT`)
  * `KUBERNETES` - Kubernetes (default transport: `MQTT`; declared now, full wiring lands in a later phase)
  * `auto` - auto-detect the platform (default)
* **Effect**: Determines the runtime environment and the default transport

### --transport
* **Description**: Selects the messaging transport for component communication
* **Usage**: `--transport <TRANSPORT> [config_file_path]`
* **Required**: No (defaults to the transport derived from `--platform`)
* **Dependencies**: `IPC` is only valid on `--platform GREENGRASS`
* **Values**: One of:
  * `IPC` - native Greengrass IPC (only valid on `--platform GREENGRASS`)
  * `MQTT <config_file_path>` - dual MQTT clients for non-Greengrass environments
* **Effect**: Determines the messaging architecture

> **Migration from `-m/--mode`:** the legacy `-m/--mode` flag has been **removed** and now errors with
> guidance. Translate `-m GREENGRASS` → `--platform GREENGRASS`, and
> `-m STANDALONE <path>` → `--platform HOST --transport MQTT <path>`.

#### GREENGRASS platform (transport: IPC)
- Uses native Greengrass v2 IPC communication
- Managed by Greengrass runtime
- Single messaging channel for inter-component communication
- Automatic device provisioning and management

#### HOST / KUBERNETES platform (transport: MQTT, Container-Ready)
- **Dual MQTT clients**: Local broker + AWS IoT Core connectivity
- **Container deployment**: Perfect for Kubernetes, Docker, ECS, etc.
- **Independent subscriptions**: Subscribe to same topic on both clients
- **Flexible authentication**: Certificate-based and username/password
- **Configuration file required**: `--transport MQTT <messaging_config.json>` defining both MQTT connections

### -t, --thing
* **Description**: Specifies the AWS IoT thing name for the component
* **Usage**: `-t <thing_name>` or `--thing <thing_name>`
* **Required**: No
* **Dependencies**: None
* **Values**: Any valid AWS IoT thing name
* **Effect**: Associates the component with a specific AWS IoT thing identity

## Additional Details

The command line processing is implemented in the EdgeCommons class using Apache Commons CLI library. Options can be provided when initializing a EdgeCommons instance through several constructors:

### Component Initialization
```java
// Basic initialization
public EdgeCommons(String componentName, String[] args)

// Initialization with custom options
public EdgeCommons(String componentName, String[] args, Options appOptions)

// Initialization with custom options and message reception control
public EdgeCommons(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
```

The library supports adding custom application-specific options through these constructors. Components built on top of EdgeCommons can define their own command line options in addition to the core options provided by the library.

### Configuration Processing Flow
1. Command line arguments are parsed using Apache Commons CLI
2. Help is displayed if -h/--help option is present
3. Configuration source is determined (default or from -c option)
4. Platform and transport are resolved (defaults or from --platform/--transport), then messaging is initialized
5. Configuration manager is created and initialized
6. Metric emitter and heartbeat services are started

### Best Practices
* Always provide help text for custom options when extending the library
* Use the most specific constructor that meets your needs
* Consider message reception settings when working with pub/sub patterns
* Validate configuration sources and messaging settings for production deployments