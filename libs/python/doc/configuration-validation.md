# Configuration Validation

This document describes the configuration validation system in ggcommons Python, which uses JSON Schema to ensure configuration correctness and provide helpful error messages.

## Overview

Configuration validation provides:

- **Schema-based Validation**: JSON Schema validation for comprehensive checking
- **Detailed Error Reporting**: Clear error messages with path information
- **Optional Validation**: Can be disabled for flexibility during development
- **Section Validation**: Validate individual configuration sections
- **Early Error Detection**: Catch configuration errors during startup

## Configuration Validator

### Basic Usage

```python
from ggcommons.validation import ConfigurationValidator, ConfigurationValidationException

# Validate complete configuration
config = {
    "logging": {"level": "INFO"},
    "heartbeat": {"intervalSecs": 30},
    "component": {"global": {}}
}

try:
    ConfigurationValidator.validate(config)
    print("Configuration is valid")
except ConfigurationValidationException as e:
    print(f"Validation failed: {e}")
    for error in e.validation_errors:
        print(f"  - {error['message']} at {error['path']}")
```

### Section Validation

```python
# Validate specific sections
heartbeat_config = {
    "intervalSecs": 30,
    "measures": {"cpu": True, "memory": True},
    "destination": "local"
}

try:
    ConfigurationValidator.validate_section(heartbeat_config, "heartbeat")
    print("Heartbeat configuration is valid")
except ConfigurationValidationException as e:
    print(f"Heartbeat validation failed: {e}")
```

### Availability Check

```python
# Check if validation is available
if ConfigurationValidator.is_validation_available():
    print("JSON Schema validation is available")
else:
    print("Validation disabled - jsonschema library not available")
```

## Configuration Schema

The validation schema covers all ggcommons configuration sections:

### Logging Configuration

```json
{
  "logging": {
    "level": "INFO",
    "format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "/var/log/component.log",
      "maxFileSize": "10MB",
      "backupCount": 5
    },
    "loggers": {
      "com.example.component": "DEBUG",
      "ggcommons": {"level": "INFO"}
    },
    "globalControl": false
  }
}
```

**Validation Rules:**
- `level`: Must be one of DEBUG, INFO, WARNING, ERROR, CRITICAL
- `format`: Must be a valid string
- `fileLogging.maxFileSize`: Must match pattern like "10MB", "1GB"
- `fileLogging.backupCount`: Must be non-negative integer
- `loggers`: Can be string level or object with level property

### Heartbeat Configuration

```json
{
  "heartbeat": {
    "enabled": true,
    "intervalSecs": 30,
    "measures": {
      "cpu": true,
      "memory": true,
      "disk": false,
      "files": true,
      "threads": true,
      "fds": false
    },
    "destination": "local"
  }
}
```

**Validation Rules:**
- `enabled`: Must be boolean (default true)
- `intervalSecs`: Must be a positive integer (minimum 1)
- `measures`: All properties must be boolean
- `destination`: Must be "local" or "iotcore" (the state keepalive's transport only)
- The legacy `targets[]` array is removed (UNS hard cut) and now fails validation

### Metric Emission Configuration

```json
{
  "metricEmission": {
    "target": "cloudwatch",
    "namespace": "MyApp/Metrics",
    "intervalSecs": 60,
    "largeFleetWorkaround": false,
    "targetConfig": {
      "logFileName": "metrics.log",
      "destination": "ipc"
    }
  }
}
```

**Validation Rules:**
- `target`: Must be "cloudwatch", "log", "messaging", "cloudwatchcomponent" or "prometheus"
- `intervalSecs`: Must be positive integer
- `targetConfig.destination`: Must be one of "ipc", "local", "iotcore", "iot_core"
- The former `targetConfig.topic` override is removed (UNS hard cut): the messaging target
  publishes to the UNS metric topic `ecv1/{device}/{component}/main/metric/{metricName}` and
  the cloudwatchcomponent topic is the fixed external contract `cloudwatch/metric/put`

### Tags Configuration

```json
{
  "tags": {
    "site": "factory-1",
    "line": "assembly-a",
    "station": "welding-01"
  }
}
```

**Validation Rules:**
- All tag values must be strings
- Tag keys can be any valid JSON property name

### Component Configuration

```json
{
  "component": {
    "global": {
      "serverUrl": "https://api.example.com",
      "timeout": 30
    },
    "instances": [
      {
        "id": "sensor-01",
        "type": "temperature",
        "config": {
          "interval": 1000
        }
      }
    ]
  }
}
```

**Validation Rules:**
- `instances[].id`: Required string identifier
- `global`: Can contain any valid JSON object
- Instance configurations are flexible (no strict schema)

## Integration with Configuration Manager

### Automatic Validation

The `EnhancedConfigManager` automatically validates configuration:

```python
from ggcommons.config.manager.enhanced_config_manager import EnhancedConfigManager

# Validation enabled by default
config_manager = EnhancedConfigManager("com.example.Component", validate_config=True)

try:
    config_manager.init()
except ConfigurationValidationException as e:
    print(f"Configuration validation failed: {e}")
```

### Disabling Validation

```python
# Disable validation for development
config_manager = EnhancedConfigManager("com.example.Component", validate_config=False)
```

### Runtime Validation

```python
# Check if validation is enabled
if config_manager.is_validation_enabled():
    print("Configuration validation is active")

# Validate configuration changes
new_config = load_new_configuration()
try:
    config_manager.configuration_changed(new_config)
except ConfigurationValidationException as e:
    print(f"Configuration change rejected: {e}")
```

## Error Handling

### Validation Exception

```python
try:
    ConfigurationValidator.validate(invalid_config)
except ConfigurationValidationException as e:
    # Main error message
    print(f"Validation failed: {e}")
    
    # Detailed error information
    for error in e.validation_errors:
        print(f"Error: {error['message']}")
        print(f"Path: {'.'.join(error['path'])}")
        print(f"Invalid value: {error['invalid_value']}")
        print(f"Schema path: {'.'.join(error['schema_path'])}")
```

### Common Validation Errors

1. **Type Mismatches**:
   ```
   Error: 'string_value' is not of type 'integer'
   Path: heartbeat.intervalSecs
   ```

2. **Enum Violations**:
   ```
   Error: 'TRACE' is not one of ['DEBUG', 'INFO', 'WARNING', 'ERROR', 'CRITICAL']
   Path: logging.level
   ```

3. **Required Properties**:
   ```
   Error: 'id' is a required property
   Path: component.instances[0]
   ```

4. **Pattern Mismatches**:
   ```
   Error: '10XB' does not match pattern '^\\d+[KMGT]?B$'
   Path: logging.fileLogging.maxFileSize
   ```

## Custom Validation

### Extending the Schema

You can extend validation for custom configuration sections:

```python
# Add custom schema section
custom_schema = {
    "type": "object",
    "properties": {
        "myCustomSection": {
            "type": "object",
            "properties": {
                "enabled": {"type": "boolean"},
                "endpoint": {"type": "string", "format": "uri"}
            },
            "required": ["enabled"]
        }
    }
}

# Validate with custom schema
# (This would require extending ConfigurationValidator)
```

### Application-Specific Validation

```python
def validate_business_rules(config):
    """Custom validation for business-specific rules."""
    
    # Example: Heartbeat interval must be reasonable
    heartbeat = config.get('heartbeat', {})
    interval = heartbeat.get('intervalSecs', 30)
    
    if interval < 5:
        raise ConfigurationValidationException(
            "Heartbeat interval too short (minimum 5 seconds)"
        )
    
    if interval > 300:
        raise ConfigurationValidationException(
            "Heartbeat interval too long (maximum 300 seconds)"
        )
    
    # Example: Validate tag consistency
    tags = config.get('tags', {})
    required_tags = ['site', 'line']
    
    for tag in required_tags:
        if tag not in tags:
            raise ConfigurationValidationException(
                f"Required tag '{tag}' is missing"
            )

# Use custom validation
try:
    ConfigurationValidator.validate(config)
    validate_business_rules(config)
    print("Configuration passed all validation")
except ConfigurationValidationException as e:
    print(f"Validation failed: {e}")
```

## Best Practices

### Schema Design

1. **Be Specific**: Use specific types and constraints
2. **Provide Defaults**: Document default values in descriptions
3. **Use Patterns**: Validate string formats with regex patterns
4. **Enum Values**: Use enums for limited value sets

### Error Handling

1. **Graceful Degradation**: Continue with warnings for non-critical errors
2. **Clear Messages**: Provide actionable error messages
3. **Context Information**: Include path and value information
4. **Recovery Options**: Suggest fixes when possible

### Development Workflow

1. **Enable During Development**: Use validation to catch errors early
2. **Test Invalid Configs**: Test with intentionally invalid configurations
3. **Document Schema**: Keep schema documentation up to date
4. **Version Schema**: Version schema changes for compatibility

## Performance Considerations

- **Validation Overhead**: Schema validation adds minimal overhead
- **Caching**: Schema is loaded once and cached
- **Optional Validation**: Can be disabled in production if needed
- **Lazy Loading**: Schema is loaded only when first used

## Dependencies

Configuration validation requires the `jsonschema` library:

```bash
pip install jsonschema
```

If not available, validation is automatically disabled with a warning message.

## Testing

### Unit Tests

```python
import pytest
from ggcommons.validation import ConfigurationValidator, ConfigurationValidationException

def test_valid_configuration():
    config = {
        "logging": {"level": "INFO"},
        "heartbeat": {"intervalSecs": 30},
        "component": {"global": {}}
    }
    
    # Should not raise exception
    ConfigurationValidator.validate(config)

def test_invalid_logging_level():
    config = {
        "logging": {"level": "INVALID_LEVEL"}
    }
    
    with pytest.raises(ConfigurationValidationException) as exc_info:
        ConfigurationValidator.validate(config)
    
    assert "is not one of" in str(exc_info.value)

def test_missing_required_field():
    config = {
        "component": {
            "instances": [{}]  # Missing required 'id' field
        }
    }
    
    with pytest.raises(ConfigurationValidationException) as exc_info:
        ConfigurationValidator.validate(config)
    
    assert "'id' is a required property" in str(exc_info.value)
```

### Integration Tests

```python
def test_config_manager_validation():
    # Test that config manager properly validates configuration
    config_manager = EnhancedConfigManager("test.component", validate_config=True)
    
    invalid_config = {"logging": {"level": "INVALID"}}
    
    with pytest.raises(ConfigurationValidationException):
        config_manager._validate_configuration(invalid_config)
```

This validation system ensures that configuration errors are caught early and provide clear guidance for fixing issues, improving the overall reliability of ggcommons-based applications.