# GGCommons Messaging Documentation

## Overview
The GGCommons library provides a unified messaging abstraction layer whose behavior is driven by the
`--transport` axis (derived from `--platform`):
- **`IPC` transport** (the `GREENGRASS` platform): Native AWS Greengrass IPC communication
- **`MQTT` transport** (the `HOST` platform, dual-MQTT): Dual MQTT clients for non-Greengrass
  environments (Kubernetes, Docker, etc.)

This documentation explains the key components, message structure, and usage patterns of the messaging system across all supported runtimes.

## Key Components

### MessagingService
The primary interface for applications to interact with the messaging system through dependency injection. It provides methods for:
- Publishing messages
- Subscribing to topics
- Making request-reply style calls
- Managing subscriptions

### Message Structure
Messages in GGCommons follow a header-payload model consisting of:
1. **Header** - Contains metadata like:
   - Message name
   - Version
   - Correlation ID
   - UUID
   - Timestamp
   - Reply-to address (for request-reply patterns)
   
2. **Tags** - Contextual information that can be:
   - Loaded from configuration
   - Added dynamically at runtime
   
3. **Body** - The actual payload/content of the message

### Communication Patterns

#### 1. Publish-Subscribe
Basic pub-sub messaging using either MQTT or Greengrass IPC:
```python
from ggcommons.interfaces import IMessagingService
from awsiot.greengrasscoreipc.model import QOS

# Get messaging service through dependency injection
messaging_service = ggcommons.get_service(IMessagingService)

# Publishing to local broker
messaging_service.publish(topic, message)

# Publishing to IoT Core
messaging_service.publish_to_iot_core(topic, message, QOS.AT_LEAST_ONCE)

# Subscribing to local broker
messaging_service.subscribe(topic_filter, message_handler)

# Subscribing to IoT Core
messaging_service.subscribe_to_iot_core(topic_filter, message_handler, QOS.AT_MOST_ONCE)
```

#### 2. Request-Reply
Synchronous and asynchronous request-reply patterns:
```python
# Making a request to local broker
future = messaging_service.request(topic, request_message)
success, response = future.get(timeout_seconds)  # Blocking

# Making a request to IoT Core
future = messaging_service.request_from_iot_core(topic, request_message)
success, response = future.get(timeout_seconds)  # Blocking

# Replying to requests (automatically routes to correct broker)
messaging_service.reply(request_message, reply_message)
```

### Providers
The library includes three messaging providers:

1. **GreengrassIpcProvider**: 
   - Implements native Greengrass v2 IPC communication
   - Used in production deployments on Greengrass cores
   - Single client for inter-component communication

2. **MqttProvider**: 
   - Single MQTT client simulating Greengrass IPC behavior
   - Useful for development and debugging
   - Supports local testing without Greengrass runtime

3. **StandaloneProvider** (NEW!):
   - **Dual MQTT clients** for maximum flexibility
   - **Local MQTT client**: For local/edge communication
   - **IoT Core MQTT client**: Direct AWS IoT Core connectivity
   - **Independent subscriptions**: Subscribe to same topic on both clients
   - **Multiple authentication**: Certificate-based and username/password
   - **Container-ready**: Perfect for Kubernetes, Docker, ECS, etc.
   - **Blocking connections**: Ensures reliable startup with connection confirmation

## Message Creation
Messages can be created using the MessageBuilder pattern:

```python
from ggcommons.builders import MessageBuilder

# Create message with builder pattern
message = MessageBuilder.create("DataUpdate", "1.0") \
    .with_payload({"temperature": 25.5, "humidity": 60}) \
    .with_config(config_service) \
    .with_correlation_id("req-123") \
    .build()

# Create message from existing object
existing_data = {"header": {...}, "body": {...}}
message = MessageBuilder.from_object(existing_data).build()
```

## Best Practices

1. **Topic Structure**
   - Use consistent topic hierarchies
   - Follow Greengrass/IoT Core topic naming conventions
   - Example: `{ThingName}/{ComponentName}/{InstanceId}/data`

2. **Message Versioning**
   - Always include message versions in headers
   - Helps with backward compatibility
   - Use semantic versioning (e.g., "1.0", "1.1", "2.0")

3. **Error Handling**
   - Use try-catch blocks around messaging operations
   - Handle connection failures gracefully
   - Implement retry logic for critical messages

4. **Resource Cleanup**
   - Unsubscribe from topics when no longer needed
   - Cancel outstanding requests if no longer waiting for replies

## Runtime Environment Options

### GREENGRASS platform / IPC transport (Traditional)
```bash
python3 main.py --platform GREENGRASS -c GG_CONFIG -t thing-name
```
- Native Greengrass v2 IPC communication
- Automatic device provisioning
- Managed by Greengrass runtime

### HOST platform / dual-MQTT transport (Container-Ready)
```bash
python3 main.py --platform HOST --transport MQTT ./messaging-config.json -c FILE ./config.json -t thing-name
```
- **Kubernetes**: Deploy as pods with dual connectivity
- **Docker**: Run in containers with external MQTT broker
- **Edge Computing**: Industrial gateways, edge servers
- **Development**: Local testing without Greengrass
- **Hybrid Architectures**: Mix Greengrass and container deployments

> The legacy `-m/--mode` flag has been removed: `-m GREENGRASS` → `--platform GREENGRASS`;
> `-m STANDALONE <path>` → `--platform HOST --transport MQTT <path>`.

### Code Compatibility
- **Same application code** works across every platform/transport combination
- **Only configuration changes** between environments
- **Seamless migration** from Greengrass to containers or vice versa

## Common Use Cases

### GREENGRASS platform (IPC transport)
1. **Inter-Component Communication**
   - Native Greengrass component messaging
   - Managed device deployments
   - Edge computing with AWS management

### HOST platform (dual-MQTT transport)
1. **Kubernetes Deployments**
   - Microservices architecture with dual connectivity
   - ConfigMaps for configuration, Secrets for certificates
   - Horizontal scaling with load balancers

2. **Industrial IoT Gateways**
   - Local MQTT for sensor data collection
   - IoT Core for cloud telemetry and commands
   - Edge processing with cloud connectivity

3. **Hybrid Cloud-Edge Architecture**
   - Some components in Greengrass, others in containers
   - Consistent messaging patterns across environments
   - Flexible deployment based on requirements

4. **Development and Testing**
   - Local development without Greengrass installation
   - CI/CD pipelines with containerized testing
   - Easier debugging with standard MQTT tools

### Universal Use Cases (any platform / transport)
1. **Request-Response Workflows**
   - Service invocation patterns
   - Blocking and non-blocking requests

2. **Event Broadcasting**
   - Publishing state changes
   - Broadcasting metrics or telemetry

3. **Configuration Updates**
   - Distributing configuration changes
   - Component coordination

## Message Tags
Tags provide contextual information about messages and can be used for:
- Source identification
- Message routing
- Filtering and processing logic
- Debugging and tracking

Tags are automatically populated from configuration and can be accessed via:
```python
message_tags = message.get_tags()
component_name = message_tags.get_component_name()
thing_name = message_tags.get_thing_name()
```

## MQTT transport configuration

The `MQTT` transport (e.g. the `HOST` platform) requires a messaging configuration file that defines both local and IoT Core MQTT connections:

```json
{
  "messaging": {
    "local": {
      "type": "mqtt",
      "host": "mqtt-broker.local",
      "port": 1883,
      "clientId": "my-component-local",
      "credentials": {
        "username": "mqtt-user",
        "password": "mqtt-password"
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

### Local Broker Authentication
- **Username/Password**: For development or brokers with basic auth
- **Certificate-based**: For production with mutual TLS authentication
- **No authentication**: Allowed for development environments

### IoT Core Authentication
- **Certificate-based**: Required for production
- Must provide `certPath`, `keyPath`, and `caPath`
- No username/password option available

### Dual Connectivity Benefits
1. **Local Communication**: Fast, low-latency messaging for edge processing
2. **Cloud Integration**: Direct AWS IoT Core connectivity for telemetry and commands
3. **Independent Subscriptions**: Subscribe to same topic on both brokers
4. **Flexible Routing**: Route messages based on content, priority, or destination

### Kubernetes Example
```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: messaging-config
data:
  messaging.json: |
    {
      "messaging": {
        "local": {
          "type": "mqtt",
          "host": "mosquitto-service",
          "port": 1883,
          "clientId": "my-component-local"
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

## Advanced Features

### Dual Subscription Testing
Test that local and IoT Core subscriptions work independently:
```python
def test_dual_subscription():
    topic = "test/dualTopic"
    local_received = []
    iot_core_received = []
    
    def local_handler(t, m):
        local_received.append(m)
        
    def iot_core_handler(t, m):
        iot_core_received.append(m)
    
    # Subscribe to same topic on both brokers
    messaging_service.subscribe(topic, local_handler)
    messaging_service.subscribe_to_iot_core(topic, iot_core_handler, QOS.AT_MOST_ONCE)
    
    # Publish to local - only local handler should receive
    local_msg = MessageBuilder.create("LocalMessage", "1.0") \
        .with_payload({"source": "local"}) \
        .build()
    messaging_service.publish(topic, local_msg)
    
    # Publish to IoT Core - only IoT Core handler should receive
    iot_msg = MessageBuilder.create("IoTCoreMessage", "1.0") \
        .with_payload({"source": "iotcore"}) \
        .build()
    messaging_service.publish_to_iot_core(topic, iot_msg, QOS.AT_LEAST_ONCE)
```

### Connection Management
The STANDALONE provider includes robust connection management:
- **Blocking connections**: Waits for connection confirmation before proceeding
- **Blocking subscriptions**: Waits for SUBACK confirmation before returning
- **Automatic cleanup**: Handles disconnections and timeouts gracefully
- **5-second timeouts**: For both connections and subscriptions

### Topic Filtering
Support for MQTT topic wildcards:
```python
# Subscribe with wildcards
messaging_service.subscribe("sensors/+/temperature", temperature_handler)
messaging_service.subscribe("sensors/#", all_sensor_handler)
```

## Migration Guide

### From Legacy MessagingClient
```python
# Old way (still supported)
from ggcommons import MessagingClient
MessagingClient.publish(topic, message)

# New way (recommended)
from ggcommons.interfaces import IMessagingService
messaging_service = ggcommons.get_service(IMessagingService)
messaging_service.publish(topic, message)
```

### Enhanced Builder Pattern
```python
# Use enhanced message builder
message = MessageBuilder.create("DataUpdate", "1.0") \
    .with_payload(sensor_data) \
    .with_config(config_service) \
    .with_correlation_id(request_id) \
    .build()
```

## Troubleshooting

### Common Issues
- **Connection failures**: Check network connectivity and credentials
- **Subscription not working**: Verify topic names and QoS settings
- **Messages not received**: Check topic filters and callback registration
- **Certificate errors**: Verify certificate paths and permissions

### Debug Logging
Enable debug logging for messaging components:
```json
{
  "logging": {
    "level": "DEBUG",
    "loggers": {
      "ggcommons.messaging": "DEBUG"
    }
  }
}
```

### Connection Verification
```python
# Check native clients
clients = messaging_service.get_native_local_client()
local_client = clients.get('local')
iot_core_client = clients.get('iot_core')

print(f"Local connected: {local_client.is_connected() if local_client else False}")
print(f"IoT Core connected: {iot_core_client.is_connected() if iot_core_client else False}")
```