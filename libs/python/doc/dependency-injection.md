# Dependency Injection System

This document describes the dependency injection (DI) system in ggcommons Python, which enables loose coupling, better testability, and cleaner architecture.

## Overview

The ggcommons DI system is a lightweight container that manages service instances and their dependencies. It provides:

- **Service Registration**: Register implementations by interface type
- **Service Retrieval**: Get services by their interface type
- **Lifecycle Management**: Control service creation and cleanup
- **Thread Safety**: Safe concurrent access to services

## Core Components

### ServiceRegistry

The `ServiceRegistry` is the heart of the DI system:

```python
from ggcommons.di import ServiceRegistry
from ggcommons.interfaces import IMessagingService, IMetricService

# Create registry
registry = ServiceRegistry()

# Register services
registry.register(IMessagingService, messaging_service_instance)
registry.register(IMetricService, metric_service_instance)

# Retrieve services
messaging = registry.get(IMessagingService)
metrics = registry.get(IMetricService)

# Check registration
if registry.is_registered(IMessagingService):
    print("Messaging service is available")
```

#### Key Methods

- `register(service_type, implementation)`: Register a service implementation
- `get(service_type)`: Retrieve a service by type
- `is_registered(service_type)`: Check if a service is registered
- `unregister(service_type)`: Remove a service registration
- `clear()`: Remove all registrations

### ServiceFactory

The `ServiceFactory` creates and registers default service implementations:

```python
from ggcommons.di import ServiceFactory

# Register all default services
ServiceFactory.register_default_services(registry, config_manager)

# Create individual services
config_service = ServiceFactory.create_configuration_service(config_manager)
messaging_service = ServiceFactory.create_messaging_service()
metric_service = ServiceFactory.create_metric_service(config_manager)
```

## Service Interfaces

### IConfigurationService

Provides configuration management capabilities:

```python
from ggcommons.interfaces import IConfigurationService

config_service = registry.get(IConfigurationService)

# Access configuration
global_config = config_service.get_global_config()
instance_config = config_service.get_instance_config("instance1")

# Template resolution
topic = config_service.resolve_template("{ThingName}/{ComponentName}/data")

# Change listeners
config_service.add_config_change_listener(my_listener)
```

### IMessagingService

Abstracts messaging operations:

```python
from ggcommons.interfaces import IMessagingService
from awsiot.greengrasscoreipc.model import QOS

messaging_service = registry.get(IMessagingService)

# Subscribe to messages
messaging_service.subscribe("data/+", message_handler)
messaging_service.subscribe_to_iot_core("cloud/data", message_handler, QOS.AT_LEAST_ONCE)

# Publish messages
messaging_service.publish("data/sensor1", message)
messaging_service.publish_to_iot_core("cloud/data", message, QOS.AT_LEAST_ONCE)

# Request-response
future = messaging_service.request("service/request", request_message)
response = future.result(timeout=30)
```

### IMetricService

Handles metric operations:

```python
from ggcommons.interfaces import IMetricService

metric_service = registry.get(IMetricService)

# Define metrics
metric_service.define_metric(cpu_metric)

# Emit metrics
metric_service.emit_metric("cpu_usage", {"usage": 75.5})
metric_service.emit_metric_now("memory_usage", {"used": 1024.0})
```

## Integration with GGCommons

### Automatic Service Registration

When GGCommons initializes, it automatically registers default services:

```python
ggcommons = GGCommons("com.example.Component", args)

# Services are automatically available
messaging = ggcommons.get_service(IMessagingService)
config = ggcommons.get_service(IConfigurationService)
metrics = ggcommons.get_service(IMetricService)
```

### Custom Service Registration

You can override default services with custom implementations:

```python
# Create custom service
custom_messaging = MyCustomMessagingService()

# Register it
ggcommons.register_service(IMessagingService, custom_messaging)

# Now all components will use the custom service
messaging = ggcommons.get_service(IMessagingService)  # Returns custom_messaging
```

## Service Implementation

### Creating Custom Services

To create a custom service, implement the appropriate interface:

```python
from ggcommons.interfaces import IMessagingService

class CustomMessagingService(IMessagingService):
    def subscribe(self, topic, handler, max_messages=10):
        # Custom implementation
        pass
        
    def publish(self, topic, message):
        # Custom implementation
        pass
        
    # Implement other required methods...
```

### Service Dependencies

Services can depend on other services:

```python
class MyMetricService(IMetricService):
    def __init__(self, config_service: IConfigurationService):
        self.config_service = config_service
        self.namespace = config_service.get_global_config().get('namespace', 'default')
        
    def emit_metric(self, name, values):
        # Use config_service for configuration
        pass
```

## Testing with DI

### Mock Services

The DI system makes testing easy with mock services:

```python
import unittest.mock
from ggcommons.interfaces import IMessagingService

class TestMyComponent(unittest.TestCase):
    def setUp(self):
        # Create mock service
        self.mock_messaging = unittest.mock.Mock(spec=IMessagingService)
        
        # Register mock
        registry = ServiceRegistry()
        registry.register(IMessagingService, self.mock_messaging)
        
        # Create component with mocked services
        self.component = MyComponent(registry)
        
    def test_publish_message(self):
        # Test component behavior
        self.component.send_data({"value": 42})
        
        # Verify mock was called correctly
        self.mock_messaging.publish.assert_called_once_with(
            "data/sensor", 
            unittest.mock.ANY
        )
```

### TestableGGCommons

For integration testing, use a testable version:

```python
from ggcommons.test import TestableGGCommons

class TestIntegration(unittest.TestCase):
    def setUp(self):
        self.ggcommons = TestableGGCommons("test.component", [])
        
        # Services are automatically mocked
        self.messaging = self.ggcommons.get_service(IMessagingService)
        self.metrics = self.ggcommons.get_service(IMetricService)
        
    def test_heartbeat_publishing(self):
        # Test that heartbeat publishes correctly
        # Mock services will capture calls for verification
        pass
```

## Best Practices

### Service Design

1. **Interface Segregation**: Keep interfaces focused and cohesive
2. **Dependency Injection**: Accept dependencies through constructor or setter
3. **Stateless When Possible**: Prefer stateless services for thread safety
4. **Error Handling**: Provide clear error messages and proper exception handling

### Registration Patterns

1. **Register Early**: Register services during application startup
2. **Single Registration**: Register each service type only once
3. **Validation**: Validate service implementations before registration
4. **Cleanup**: Unregister services during shutdown if needed

### Testing Strategies

1. **Mock External Dependencies**: Use mocks for external services
2. **Test Service Contracts**: Verify services implement interfaces correctly
3. **Integration Testing**: Test service interactions
4. **Isolation**: Test components in isolation using mocked services

## Thread Safety

The DI system is designed to be thread-safe:

- **ServiceRegistry**: All operations use locks for thread safety
- **Service Retrieval**: Safe to call from multiple threads
- **Service Registration**: Safe to register services concurrently
- **Service Implementations**: Individual services are responsible for their own thread safety

## Performance Considerations

- **Lazy Loading**: Services are created only when first requested
- **Caching**: Service instances are cached after first retrieval
- **Minimal Overhead**: DI adds minimal runtime overhead
- **Memory Efficiency**: Services are stored efficiently in hash maps

## Migration from Static Access

### Before (Static Access)
```python
from ggcommons.messaging.messaging_client import MessagingClient

MessagingClient.publish("topic", message)
```

### After (Dependency Injection)
```python
messaging_service = ggcommons.get_service(IMessagingService)
messaging_service.publish("topic", message)
```

### Gradual Migration

You can mix both approaches during migration:

```python
# Old code continues to work
MessagingClient.publish("topic", message)

# New code uses DI
messaging_service = ggcommons.get_service(IMessagingService)
messaging_service.publish("topic", message)
```

The static classes are now implemented as wrappers around the DI services, ensuring consistency.