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
* **Required**: No (defaults to "GG_CONFIG" if not specified)
* **Dependencies**: None
* **Values**: One of the following sources with optional additional parameters:
  * `FILE [file_path]` - Load configuration from a file
  * `ENV [env_var_name]` - Load configuration from environment variable
  * `SHADOW [shadow_name]` - Load configuration from AWS IoT Device Shadow
  * `GG_CONFIG [component_name] [config_key]` - Load configuration from Greengrass component configuration (default)
  * `CONFIG_COMPONENT` - Load configuration from a configuration component
* **Effect**: Determines how the component obtains its configuration settings

### -m, --mode
* **Description**: Specifies the runtime mode and messaging system for component communication
* **Usage**: `-m <system> [args]` or `--messaging <system> [args]`
* **Required**: No (defaults to "GREENGRASS" if not specified)
* **Dependencies**: None
* **Values**: One of:
  * `GREENGRASS` - Use native Greengrass IPC for messaging (default)
  * `STANDALONE <config_file_path>` - **NEW!** Use dual MQTT clients for non-Greengrass environments
* **Effect**: Determines the runtime environment and messaging architecture

#### GREENGRASS Mode
- Uses native Greengrass v2 IPC communication
- Managed by Greengrass runtime
- Single messaging channel for inter-component communication
- Automatic device provisioning and management

#### STANDALONE Mode (Container-Ready)
- **Dual MQTT clients**: Local broker + AWS IoT Core connectivity
- **Container deployment**: Perfect for Kubernetes, Docker, ECS, etc.
- **Independent subscriptions**: Subscribe to same topic on both clients
- **Flexible authentication**: Certificate-based and username/password
- **Configuration file required**: JSON file defining both MQTT connections

### -t, --thing
* **Description**: Specifies the AWS IoT thing name for the component
* **Usage**: `-t <thing_name>` or `--thing <thing_name>`
* **Required**: No
* **Dependencies**: None
* **Values**: Any valid AWS IoT thing name
* **Effect**: Associates the component with a specific AWS IoT thing identity

## Additional Details

The command line processing is implemented in the GGCommons class using Apache Commons CLI library. Options can be provided when initializing a GGCommons instance through several constructors:

### Component Initialization
```java
// Basic initialization
public GGCommons(String componentName, String[] args)

// Initialization with custom options
public GGCommons(String componentName, String[] args, Options appOptions)

// Initialization with custom options and message reception control
public GGCommons(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
```

The library supports adding custom application-specific options through these constructors. Components built on top of GGCommons can define their own command line options in addition to the core options provided by the library.

### Configuration Processing Flow
1. Command line arguments are parsed using Apache Commons CLI
2. Help is displayed if -h/--help option is present
3. Configuration source is determined (default or from -c option)
4. Messaging system is initialized (default or from -m option)
5. Configuration manager is created and initialized
6. Metric emitter and heartbeat services are started

### Best Practices
* Always provide help text for custom options when extending the library
* Use the most specific constructor that meets your needs
* Consider message reception settings when working with pub/sub patterns
* Validate configuration sources and messaging settings for production deployments