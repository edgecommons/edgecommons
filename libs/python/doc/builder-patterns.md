# Builder Patterns

This document describes the builder pattern implementations in edgecommons Python, which provide fluent APIs for constructing complex objects with improved readability and validation.

## Overview

Builder patterns in edgecommons provide:

- **Fluent APIs**: Method chaining for readable object construction
- **Parameter Validation**: Early validation of required and optional parameters
- **Extensibility**: Easy addition of new parameters without breaking changes
- **Immutability**: Objects are fully constructed before use
- **Backward Compatibility**: Existing constructors continue to work with deprecation warnings

## Available Builders

### EdgeCommonsBuilder

Creates EdgeCommons instances with fluent configuration:

```python
from edgecommons import EdgeCommonsBuilder

# Basic usage
edgecommons = EdgeCommonsBuilder.create("com.example.MyComponent") \
    .with_args(["--config", "FILE", "config.json"]) \
    .build()

# Full configuration
edgecommons = EdgeCommonsBuilder.create("com.example.MyComponent") \
    .with_args(args) \
    .with_app_options(custom_parser) \
    .receive_own_messages(False) \
    .initial_ready(False) \
    .configuration_validator("application", validate_candidate) \
    .configuration_validation_timeout(5.0) \
    .configure_commands(lambda inbox: inbox.register("capture", capture)) \
    .build()
```

#### Methods

- `create(component_name)`: Static factory method to create builder
- `with_args(args)`: Set command line arguments
- `with_app_options(parser)`: Set custom ArgumentParser
- `receive_own_messages(flag)`: Set message reception behavior
- `initial_ready(flag)`: Set the app readiness gate before any endpoint starts (default `True`)
- `configuration_validator(name, callback)`: Register a pre-commit `INITIAL`/`RELOAD` validator
- `configuration_validation_timeout(seconds)`: Set the overall validator deadline (default 5, max 60)
- `configure_commands(callback)`: Install component verbs before acknowledged inbox activation
- `build()`: Create the EdgeCommons instance

#### Validation

- Component name cannot be None or empty
- Args list cannot be None (empty list is acceptable)
- App options must be a valid ArgumentParser instance
- Validator names are unique, non-empty strings and callbacks must be callable
- Validation timeouts are positive and no greater than 60 seconds

### MessageBuilder

Creates Message instances with comprehensive configuration:

```python
from edgecommons.builders import MessageBuilder

# Basic message
message = MessageBuilder.create("heartbeat", "1.0") \
    .with_payload({"status": "alive"}) \
    .with_config(config_manager) \
    .build()

# Request-response message
message = MessageBuilder.create("data_request", "1.0") \
    .with_payload({"sensor_id": "temp01"}) \
    .with_config(config_manager) \
    .with_correlation_id("req-12345") \
    .with_reply_to("responses/temp01") \
    .build()

# From existing JSON
message = MessageBuilder.from_object(json_data) \
    .with_config(config_manager) \
    .build()
```

#### Methods

- `create(name, version)`: Static factory method
- `from_object(json_object)`: Create from existing JSON data
- `with_payload(payload)`: Set message payload
- `with_config(config_manager)`: Set configuration for header population
- `with_correlation_id(id)`: Set correlation ID for request-response
- `with_reply_to(topic)`: Set reply-to topic
- `build()`: Create the Message instance

#### Validation

- Message name and version cannot be None or empty
- Payload is required for build()
- Correlation ID and reply-to cannot be empty strings
- Config manager must be valid ConfigManager instance

### MetricBuilder

Creates Metric instances with measures and dimensions:

```python
from edgecommons.builders import MetricBuilder

# Simple metric
metric = MetricBuilder.create("cpu_usage") \
    .add_measure("usage", "Percent", 1) \
    .build()

# Complex metric with namespace and dimensions
metric = MetricBuilder.create("system_performance") \
    .with_namespace("MyApp/System") \
    .with_thing_name("device-001") \
    .with_component_name("monitor") \
    .add_measure("cpu_usage", "Percent", 1) \
    .add_measure("memory_usage", "Megabytes", 1) \
    .add_measure("disk_usage", "Gigabytes", 60) \
    .add_dimension("instance", "primary") \
    .add_dimension("region", "us-east-1") \
    .build()
```

#### Methods

- `create(name)`: Static factory method
- `with_namespace(namespace)`: Set CloudWatch namespace
- `with_thing_name(name)`: Set AWS IoT Thing name
- `with_component_name(name)`: Set component name
- `add_measure(name, unit, resolution)`: Add a metric measure
- `add_dimension(key, value)`: Add a custom dimension
- `build()`: Create the Metric instance

#### Validation

- Metric name cannot be None or empty
- Measure names and units cannot be None or empty
- Storage resolution must be 1 or 60 seconds
- Dimension keys and values cannot be None or empty

## Migration Guide

### From Direct Construction

#### Before
```python
# Old EdgeCommons construction
edgecommons = EdgeCommons("com.example.Component", args, options, False)

# Old Message construction
message = Message.build_from_config("heartbeat", "1.0", payload, config_manager)

# Old Metric construction
metric = Metric("cpu_usage", "MyApp/Metrics")
metric.add_measure(Measure("usage", "Percent", 1))
```

#### After
```python
# New EdgeCommons construction
edgecommons = EdgeCommonsBuilder.create("com.example.Component") \
    .with_args(args) \
    .with_app_options(options) \
    .receive_own_messages(False) \
    .build()

# New Message construction
message = MessageBuilder.create("heartbeat", "1.0") \
    .with_payload(payload) \
    .with_config(config_manager) \
    .build()

# New Metric construction
metric = MetricBuilder.create("cpu_usage") \
    .with_namespace("MyApp/Metrics") \
    .add_measure("usage", "Percent", 1) \
    .build()
```

### Gradual Migration

Old constructors continue to work but show deprecation warnings:

```python
import warnings

# This will work but show a deprecation warning
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    message = Message.build_from_config("heartbeat", "1.0", payload, config_manager)
```

## Advanced Usage

### Conditional Building

```python
builder = MessageBuilder.create("sensor_data", "1.0") \
    .with_payload(sensor_data) \
    .with_config(config_manager)

# Add correlation ID only for requests
if is_request:
    builder = builder.with_correlation_id(generate_correlation_id())

# Add reply-to only if needed
if needs_reply:
    builder = builder.with_reply_to(f"responses/{sensor_id}")

message = builder.build()
```

### Builder Reuse

```python
# Create base builder
base_builder = MetricBuilder.create("system_metrics") \
    .with_namespace("MyApp/System") \
    .with_thing_name(thing_name) \
    .with_component_name(component_name)

# Create specific metrics from base
cpu_metric = base_builder \
    .add_measure("cpu_usage", "Percent", 1) \
    .add_dimension("type", "cpu") \
    .build()

memory_metric = base_builder \
    .add_measure("memory_usage", "Megabytes", 1) \
    .add_dimension("type", "memory") \
    .build()
```

### Custom Validation

```python
class ValidatedMessageBuilder(MessageBuilder):
    def with_payload(self, payload):
        # Custom validation
        if not isinstance(payload, dict):
            raise ValueError("Payload must be a dictionary")
        if 'timestamp' not in payload:
            payload['timestamp'] = int(time.time())
        return super().with_payload(payload)
```

## Error Handling

### Validation Errors

Builders perform validation at each step and during build():

```python
try:
    message = MessageBuilder.create("", "1.0")  # Empty name
except ValueError as e:
    print(f"Validation error: {e}")

try:
    message = MessageBuilder.create("test", "1.0").build()  # Missing payload
except ValueError as e:
    print(f"Build error: {e}")
```

### Common Errors

1. **Empty Required Fields**: Name, version, payload cannot be empty
2. **Invalid Parameters**: Storage resolution must be 1 or 60
3. **Missing Dependencies**: Config manager required for certain features
4. **Type Mismatches**: Parameters must be correct types

## Best Practices

### Builder Design

1. **Fluent Interface**: Always return `self` from configuration methods
2. **Validation**: Validate parameters immediately when set
3. **Immutability**: Don't modify builder state after build()
4. **Clear Errors**: Provide descriptive error messages

### Usage Patterns

1. **Method Chaining**: Use method chaining for readability
2. **Required First**: Set required parameters before optional ones
3. **Validate Early**: Call build() to validate configuration
4. **Reuse Builders**: Create base builders for common configurations

### Testing

```python
def test_message_builder():
    # Test valid construction
    message = MessageBuilder.create("test", "1.0") \
        .with_payload({"data": "test"}) \
        .build()
    
    assert message.header.name == "test"
    assert message.payload["data"] == "test"
    
    # Test validation
    with pytest.raises(ValueError):
        MessageBuilder.create("", "1.0")  # Empty name
        
    with pytest.raises(ValueError):
        MessageBuilder.create("test", "1.0").build()  # Missing payload
```

## Performance Considerations

- **Object Creation**: Builders create objects only once during build()
- **Validation Overhead**: Validation adds minimal overhead
- **Memory Usage**: Builders hold references until build() is called
- **Thread Safety**: Builders are not thread-safe; use separate instances per thread

## IDE Support

Modern IDEs provide excellent support for builder patterns:

- **Auto-completion**: Method chaining shows available options
- **Type Checking**: Static analysis catches type errors
- **Documentation**: Hover help shows parameter descriptions
- **Refactoring**: Easy to rename methods across codebase

## Future Extensions

The builder pattern makes it easy to add new features:

```python
# Future MessageBuilder extensions
message = MessageBuilder.create("sensor_data", "2.0") \
    .with_payload(data) \
    .with_config(config_manager) \
    .with_encryption(encryption_key) \     # Future feature
    .with_compression(True) \              # Future feature
    .with_priority(Priority.HIGH) \        # Future feature
    .build()
```

New methods can be added without breaking existing code, making the API extensible and maintainable.
