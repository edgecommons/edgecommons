# Migration Guide

This document provides guidance for migrating from the legacy ggcommons Python API to the enhanced version with dependency injection, builder patterns, and improved configuration management.

## Overview

The enhanced ggcommons maintains full backward compatibility while introducing new patterns and capabilities. You can migrate gradually, mixing old and new APIs during the transition.

## Key Changes

### 1. Dependency Injection System
- Services are now accessed through interfaces
- Components can be tested with mock services
- Better separation of concerns

### 2. Builder Patterns
- Fluent APIs for object construction
- Better parameter validation
- More readable code

### 3. Enhanced Configuration
- JSON schema validation
- Improved error handling
- Better lifecycle management

### 4. Service Interfaces
- Abstract interfaces for core functionality
- Easier testing and mocking
- Cleaner architecture

## Migration Strategies

### Strategy 1: Gradual Migration (Recommended)

Migrate components one at a time while maintaining existing functionality:

```python
# Phase 1: Update initialization only
from ggcommons.builders import GGCommonsBuilder

# Old way (still works)
# ggcommons = GGCommons("com.example.Component", args)

# New way
ggcommons = GGCommonsBuilder.create("com.example.Component") \
    .with_args(args) \
    .build()

# Phase 2: Migrate to service interfaces
messaging_service = ggcommons.get_service(IMessagingService)
config_service = ggcommons.get_service(IConfigurationService)

# Phase 3: Update message creation
from ggcommons.builders import MessageBuilder

# Old way (still works with deprecation warning)
# message = Message.build_from_config("data", "1.0", payload, config_manager)

# New way
message = MessageBuilder.create("data", "1.0") \
    .with_payload(payload) \
    .with_config(config_manager) \
    .build()
```

### Strategy 2: Component-by-Component

Migrate entire components to use new patterns:

```python
class ModernComponent:
    def __init__(self, ggcommons):
        # Use service interfaces
        self.messaging = ggcommons.get_service(IMessagingService)
        self.config = ggcommons.get_service(IConfigurationService)
        self.metrics = ggcommons.get_service(IMetricService)
        
    def send_data(self, data):
        # Use builder pattern
        message = MessageBuilder.create("sensor_data", "1.0") \
            .with_payload(data) \
            .with_config(self.config.config_manager) \
            .build()
            
        self.messaging.publish("data/sensor", message)
```

### Strategy 3: New Projects Only

Use enhanced patterns for new projects while leaving existing code unchanged:

```python
# new_component.py - Uses all new patterns
from ggcommons.builders import GGCommonsBuilder, MessageBuilder, MetricBuilder
from ggcommons.interfaces import IMessagingService, IConfigurationService

class NewComponent:
    def __init__(self, args):
        self.ggcommons = GGCommonsBuilder.create("com.example.NewComponent") \
            .with_args(args) \
            .build()
            
        self.messaging = self.ggcommons.get_service(IMessagingService)
        self.config = self.ggcommons.get_service(IConfigurationService)
```

## Detailed Migration Steps

### 1. GGCommons Initialization

#### Before
```python
import ggcommons

# Basic initialization
ggcommons_instance = ggcommons.init("com.example.Component", arg_parser)

# With custom options
ggcommons_instance = ggcommons.init("com.example.Component", arg_parser, False)
```

#### After
```python
from ggcommons.builders import GGCommonsBuilder

# Basic initialization
ggcommons_instance = GGCommonsBuilder.create("com.example.Component") \
    .with_args(args) \
    .build()

# With custom options
ggcommons_instance = GGCommonsBuilder.create("com.example.Component") \
    .with_args(args) \
    .with_app_options(custom_parser) \
    .receive_own_messages(False) \
    .build()
```

### 2. Configuration Access

#### Before
```python
config_manager = ggcommons_instance.get_config_manager()
global_config = config_manager.get_global_config()
thing_name = config_manager.get_thing_name()
```

#### After
```python
# Option 1: Continue using config manager
config_manager = ggcommons_instance.get_config_manager()
global_config = config_manager.get_global_config()

# Option 2: Use service interface
config_service = ggcommons_instance.get_service(IConfigurationService)
global_config = config_service.get_global_config()
thing_name = config_service.get_thing_name()
```

### 3. Messaging Operations

#### Before
```python
from ggcommons.messaging.messaging_client import MessagingClient

# Subscribe
MessagingClient.subscribe("data/+", message_handler)

# Publish
MessagingClient.publish("data/sensor", message)
```

#### After
```python
# Option 1: Continue using MessagingClient (no changes needed)
from ggcommons.messaging.messaging_client import MessagingClient
MessagingClient.subscribe("data/+", message_handler)
MessagingClient.publish("data/sensor", message)

# Option 2: Use service interface
messaging_service = ggcommons_instance.get_service(IMessagingService)
messaging_service.subscribe("data/+", message_handler)
messaging_service.publish("data/sensor", message)
```

### 4. Message Creation

#### Before
```python
from ggcommons.messaging.message import Message

message = Message.build_from_config("sensor_data", "1.0", payload, config_manager)
```

#### After
```python
from ggcommons.builders import MessageBuilder

message = MessageBuilder.create("sensor_data", "1.0") \
    .with_payload(payload) \
    .with_config(config_manager) \
    .build()
```

### 5. Metric Definition

#### Before
```python
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.measure import Measure

metric = Metric("cpu_usage", namespace="MyApp/Metrics")
metric.add_measure(Measure("usage", "Percent", 1))
```

#### After
```python
from ggcommons.builders import MetricBuilder

metric = MetricBuilder.create("cpu_usage") \
    .with_namespace("MyApp/Metrics") \
    .add_measure("usage", "Percent", 1) \
    .build()
```

### 6. Metric Emission

#### Before
```python
from ggcommons.metrics.metric_emitter import MetricEmitter

MetricEmitter.emit_metric("cpu_usage", {"usage": 75.5})
```

#### After
```python
# Option 1: Continue using MetricEmitter
from ggcommons.metrics.metric_emitter import MetricEmitter
MetricEmitter.emit_metric("cpu_usage", {"usage": 75.5})

# Option 2: Use service interface
metric_service = ggcommons_instance.get_service(IMetricService)
metric_service.emit_metric("cpu_usage", {"usage": 75.5})
```

## Testing Migration

### Before
```python
import unittest
from unittest.mock import patch

class TestComponent(unittest.TestCase):
    @patch('ggcommons.messaging.messaging_client.MessagingClient.publish')
    def test_send_data(self, mock_publish):
        component = MyComponent()
        component.send_data({"value": 42})
        mock_publish.assert_called_once()
```

### After
```python
import unittest
from unittest.mock import Mock
from ggcommons.interfaces import IMessagingService

class TestComponent(unittest.TestCase):
    def setUp(self):
        # Create mock services
        self.mock_messaging = Mock(spec=IMessagingService)
        
        # Create testable ggcommons instance
        self.ggcommons = TestableGGCommons("test.component", [])
        self.ggcommons.register_service(IMessagingService, self.mock_messaging)
        
        self.component = MyComponent(self.ggcommons)
        
    def test_send_data(self):
        self.component.send_data({"value": 42})
        self.mock_messaging.publish.assert_called_once()
```

## Configuration Migration

### Enhanced Configuration Manager

#### Before
```python
from ggcommons.config.manager.file_config_manager import FileConfigManager

config_manager = FileConfigManager("MyComponent", "config.json")
```

#### After
```python
from ggcommons.config.manager.enhanced_config_manager import EnhancedConfigManager

# With validation enabled (default)
config_manager = EnhancedConfigManager("MyComponent", validate_config=True)

# With validation disabled
config_manager = EnhancedConfigManager("MyComponent", validate_config=False)
```

### Configuration Validation

#### New Feature
```python
from ggcommons.validation import ConfigurationValidator, ConfigurationValidationException

try:
    ConfigurationValidator.validate(config)
    print("Configuration is valid")
except ConfigurationValidationException as e:
    print(f"Configuration validation failed: {e}")
    for error in e.validation_errors:
        print(f"  - {error['message']} at {error['path']}")
```

## Handling Deprecation Warnings

### Suppressing Warnings During Migration

```python
import warnings

# Temporarily suppress deprecation warnings
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    
    # Use legacy API without warnings
    message = Message.build_from_config("data", "1.0", payload, config_manager)
```

### Filtering Specific Warnings

```python
import warnings

# Filter only ggcommons deprecation warnings
warnings.filterwarnings("ignore", category=DeprecationWarning, module="ggcommons.*")
```

## Common Migration Issues

### 1. Import Changes

Some imports may need to be updated:

```python
# Old imports that still work
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter

# New imports for enhanced features
from ggcommons.builders import GGCommonsBuilder, MessageBuilder, MetricBuilder
from ggcommons.interfaces import IMessagingService, IConfigurationService, IMetricService
from ggcommons.validation import ConfigurationValidator
```

### 2. Service Availability

Check service availability before use:

```python
messaging_service = ggcommons.get_service(IMessagingService)
if messaging_service is None:
    # Fall back to static client
    from ggcommons.messaging.messaging_client import MessagingClient
    MessagingClient.publish(topic, message)
else:
    messaging_service.publish(topic, message)
```

### 3. Configuration Manager Access

The service interface wraps the config manager:

```python
config_service = ggcommons.get_service(IConfigurationService)

# Access underlying config manager if needed
if hasattr(config_service, 'config_manager'):
    config_manager = config_service.config_manager
```

## Migration Checklist

### Phase 1: Basic Migration
- [ ] Update GGCommons initialization to use builder
- [ ] Test existing functionality works unchanged
- [ ] Update imports if needed
- [ ] Handle any deprecation warnings

### Phase 2: Service Interface Adoption
- [ ] Replace direct static calls with service interfaces
- [ ] Update component constructors to accept services
- [ ] Modify tests to use mock services
- [ ] Verify all functionality works with services

### Phase 3: Builder Pattern Adoption
- [ ] Replace direct object construction with builders
- [ ] Update message creation to use MessageBuilder
- [ ] Update metric creation to use MetricBuilder
- [ ] Test builder validation and error handling

### Phase 4: Enhanced Features
- [ ] Enable configuration validation
- [ ] Add enhanced logging configuration
- [ ] Implement custom validation rules if needed
- [ ] Update documentation and examples

## Rollback Strategy

If issues arise during migration, you can easily rollback:

### 1. Revert to Legacy Initialization
```python
# Change from builder back to direct construction
# ggcommons = GGCommonsBuilder.create("component").build()
ggcommons = GGCommons("component", args)
```

### 2. Disable New Features
```python
# Disable configuration validation
config_manager = EnhancedConfigManager("component", validate_config=False)
```

### 3. Use Legacy APIs
```python
# Continue using static clients
from ggcommons.messaging.messaging_client import MessagingClient
MessagingClient.publish(topic, message)
```

## Performance Impact

The enhanced features have minimal performance impact:

- **Service Registry**: Hash-based lookups are very fast
- **Builder Patterns**: Objects created only once during build()
- **Configuration Validation**: Performed only during configuration loading
- **Interface Wrappers**: Minimal overhead for method delegation

## Support and Resources

- **Documentation**: Comprehensive docs in the `doc/` directory
- **Examples**: Migration examples in `examples/` directory
- **Tests**: Reference implementations in test suite
- **Deprecation Warnings**: Clear guidance on migration paths

The migration can be done incrementally, allowing you to adopt new features at your own pace while maintaining full functionality throughout the process.