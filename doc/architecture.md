# GGCommons Python Architecture

This document describes the enhanced architecture of the ggcommons Python library, focusing on the dependency injection system, service interfaces, and builder patterns introduced in the realignment with the Java version.

## Overview

The enhanced ggcommons architecture is built around three core principles:

1. **Dependency Injection**: Services are injected rather than directly instantiated
2. **Interface Abstraction**: Core functionality is accessed through well-defined interfaces
3. **Builder Patterns**: Complex objects are constructed using fluent builder APIs

## Core Components

### Service Registry

The `ServiceRegistry` is a simple dependency injection container that manages service instances:

```python
from ggcommons.di import ServiceRegistry
from ggcommons.interfaces import IMessagingService

registry = ServiceRegistry()
registry.register(IMessagingService, messaging_service_impl)
service = registry.get(IMessagingService)
```

**Key Features:**
- Thread-safe operations
- Type-safe service registration and retrieval
- Support for service lifecycle management

### Service Interfaces

Three core service interfaces define the contracts for ggcommons functionality:

#### IConfigurationService
Provides access to component configuration and change notifications:
- Global and instance-specific configuration access
- Template variable resolution
- Configuration change listener management

#### IMessagingService
Abstracts messaging operations across different providers:
- IPC and IoT Core messaging
- Request-response patterns
- Topic subscription and publishing

#### IMetricService
Handles metric definition and emission:
- Metric definition registration
- Batched and immediate metric emission
- Multiple target support (CloudWatch, logs, messaging)

### Service Factory

The `ServiceFactory` creates and registers default service implementations:

```python
from ggcommons.di import ServiceFactory

ServiceFactory.register_default_services(registry, config_manager)
```

## Enhanced GGCommons Class

The main `GGCommons` class now provides:

### Dependency Injection Support
```python
ggcommons = GGCommons("com.example.Component", args)
messaging_service = ggcommons.get_service(IMessagingService)
```

### Service Registration
```python
ggcommons.register_service(IMessagingService, custom_messaging_service)
```

### Builder Pattern Integration
```python
from ggcommons.builders import GGCommonsBuilder

ggcommons = GGCommonsBuilder.create("com.example.Component") \
    .with_args(args) \
    .receive_own_messages(False) \
    .build()
```

## Configuration System

### Enhanced Configuration Manager

The `EnhancedConfigManager` extends the base `ConfigManager` with:

- **JSON Schema Validation**: Automatic validation of configuration against schema
- **Improved Error Handling**: Better error reporting and recovery
- **Lifecycle Management**: Proper initialization and change notification sequencing

### Configuration Validation

Configuration validation is performed using JSON Schema:

```python
from ggcommons.validation import ConfigurationValidator

try:
    ConfigurationValidator.validate(config)
except ConfigurationValidationException as e:
    print(f"Validation failed: {e}")
```

**Validation Features:**
- Comprehensive schema covering all configuration sections
- Detailed error reporting with path information
- Optional validation (can be disabled for flexibility)

## Builder Patterns

### GGCommons Builder
```python
ggcommons = GGCommonsBuilder.create("component.name") \
    .with_args(["--config", "FILE", "config.json"]) \
    .with_app_options(custom_parser) \
    .receive_own_messages(False) \
    .build()
```

### Message Builder
```python
message = MessageBuilder.create("heartbeat", "1.0") \
    .with_payload(data) \
    .with_config(config_manager) \
    .with_correlation_id("12345") \
    .build()
```

### Metric Builder
```python
metric = MetricBuilder.create("cpu_usage") \
    .with_namespace("MyApp/Metrics") \
    .add_measure("usage", "Percent", 1) \
    .add_dimension("instance", "main") \
    .build()
```

## Initialization Sequence

1. **Argument Processing**: Command line arguments are parsed
2. **Configuration Loading**: Configuration is loaded and validated
3. **Service Registry Setup**: Default services are registered
4. **Component Initialization**: Messaging, metrics, and heartbeat are initialized
5. **Service Injection**: Services are injected into components that support it
6. **Initialization Completion**: Configuration change notifications are enabled

## Thread Safety

- **ServiceRegistry**: All operations are thread-safe
- **Configuration Services**: Read operations are thread-safe
- **Messaging Services**: All operations are thread-safe
- **Metric Services**: All operations are thread-safe

## Backward Compatibility

The enhanced architecture maintains full backward compatibility:

- Existing APIs continue to work unchanged
- Deprecation warnings guide migration to new patterns
- Old and new patterns can be mixed during transition

## Testing Support

The architecture enables comprehensive testing through:

- **Mock Services**: Easy injection of mock implementations
- **Service Isolation**: Components can be tested in isolation
- **Configuration Validation**: Schema validation catches configuration errors early

## Performance Considerations

- **Lazy Initialization**: Services are created only when needed
- **Efficient Lookups**: Service registry uses efficient hash-based lookups
- **Minimal Overhead**: Dependency injection adds minimal runtime overhead