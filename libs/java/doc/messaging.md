TODO: This file was GenAI generated and needs enriching/corrections

# GGCommons Messaging Documentation

## Overview
The GGCommons library provides a unified messaging abstraction layer that supports multiple runtime environments:
- **GREENGRASS platform (IPC transport)**: Native AWS Greengrass IPC communication
- **HOST / KUBERNETES platform (MQTT transport)**: Dual MQTT clients for non-Greengrass environments (Kubernetes, Docker, etc.)

This documentation explains the key components, message structure, and usage patterns of the messaging system across all supported runtimes.

## Key Components

### MessagingClient
The primary interface for applications to interact with the messaging system. It provides static methods for:
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
   - Timestamp
   - Reply-to address (for request-reply patterns)
   
2. **Tags** - Contextual information that can be:
   - Loaded from configuration
   - Added dynamically at runtime
   
3. **Body** - The actual payload/content of the message

### Communication Patterns

#### 1. Publish-Subscribe
Basic pub-sub messaging using either MQTT or Greengrass IPC:
```java
// Publishing
MessagingClient.publish(topic, message);

// Subscribing
MessagingClient.subscribe(topicFilter, (topic, message) -> {
    // Handle received message
});
```

#### 2. Request-Reply
Synchronous and asynchronous request-reply patterns:
```java
// Making a request
ReplyFuture future = MessagingClient.request(topic, requestMessage);
Message response = future.get(); // Blocking
// OR
future.thenAccept(response -> { /* Handle response */ }); // Async

// Replying to requests
MessagingClient.reply(requestMessage, replyMessage);
```

##### Request deadline (`messaging.requestTimeoutSeconds`)
Every `request()` carries a **framework-owned deadline** (default **30 s**, configurable via
`messaging.requestTimeoutSeconds`; `0` disables). When it expires the library unsubscribes the
ephemeral reply topic, removes the pending entry and completes the `ReplyFuture`
**exceptionally** with a `java.util.concurrent.TimeoutException` — even if the caller never
awaits the future, so an unanswered request can no longer leak its reply subscription.
Reply-arrival, the deadline and `cancelRequest` settle a request exactly once (idempotent);
a straggler reply after settle is dropped with a DEBUG log.

```java
// Per-call deadline: an explicit value always wins over the configured default.
ReplyFuture f1 = client.request(topic, msg, Duration.ofSeconds(5));
// Duration.ZERO disables the deadline for this one call.
ReplyFuture f2 = client.request(topic, msg, Duration.ZERO);
```

Note (init order): the messaging client is built before the config loads, so the configured
default is late-bound right after the `ConfigManager` exists; until then the built-in 30 s
applies (deliberately — the CONFIG_COMPONENT bootstrap request gets a deadline too).

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

3. **StandaloneMessagingProvider** (NEW!):
   - **Dual MQTT clients** for maximum flexibility
   - **Local MQTT client**: For local/edge communication
   - **IoT Core MQTT client**: Direct AWS IoT Core connectivity
   - **Independent subscriptions**: Subscribe to same topic on both clients
   - **Multiple authentication**: Certificate-based and username/password
   - **Container-ready**: Perfect for Kubernetes, Docker, ECS, etc.

## Message Creation
Messages can be created in two ways:

1. From configuration:
```java
Message msg = Message.buildFromConfig(name, version, payload, tags);
```

2. Directly:
```java
Message msg = Message.build(messageContents);
```

## Best Practices

1. **Topic Structure**
   - Use consistent topic hierarchies
   - Follow Greengrass/IoT Core topic naming conventions

2. **Message Versioning**
   - Always include message versions in headers
   - Helps with backward compatibility

3. **Error Handling**
   - Use try-catch blocks around messaging operations
   - Handle connection failures gracefully

4. **Resource Cleanup**
   - Unsubscribe from topics when no longer needed
   - Cancel outstanding requests if no longer waiting for replies

## Runtime Environment Options

### GREENGRASS platform (Traditional)
```bash
java -jar component.jar --platform GREENGRASS -c GG_CONFIG -t thing-name
```
- Native Greengrass v2 IPC communication (default transport `IPC`)
- Automatic device provisioning
- Managed by Greengrass runtime

### HOST / KUBERNETES platform (Container-Ready)
```bash
java -jar component.jar --platform HOST --transport MQTT ./messaging-config.json -c FILE ./config.json -t thing-name
```
- **Kubernetes**: Deploy as pods with dual connectivity
- **Docker**: Run in containers with external MQTT broker
- **Edge Computing**: Industrial gateways, edge servers
- **Development**: Local testing without Greengrass
- **Hybrid Architectures**: Mix Greengrass and container deployments

> The legacy `-m/--mode` flag is removed: `-m GREENGRASS` → `--platform GREENGRASS`,
> `-m STANDALONE <path>` → `--platform HOST --transport MQTT <path>`.

### Code Compatibility
- **Same application code** works across all platforms
- **Only configuration changes** between environments
- **Seamless migration** from Greengrass to containers or vice versa

## Common Use Cases

### GREENGRASS platform
1. **Inter-Component Communication**
   - Native Greengrass component messaging
   - Managed device deployments
   - Edge computing with AWS management

### HOST / KUBERNETES platform
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

### Universal Use Cases (All Platforms)
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

Tags can be:
1. Loaded from configuration files
2. Added programmatically using `injectTag()`

## MQTT Transport Configuration (`--transport MQTT`)

The MQTT transport (used by the `HOST` and `KUBERNETES` platforms) requires a messaging configuration file — the
`--transport MQTT <messaging_config.json>` payload — that defines both local and IoT Core MQTT connections:

```json
{
  "messaging": {
    "local": {
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

### MQTT Last-Will-and-Testament (`messaging.lwt`)
An optional `lwt` section registers an MQTT will on the **local-broker connection only** at
CONNECT (re-registered automatically on reconnect). The broker publishes it if the component
disconnects ungracefully — it never passes through `publish()`. There is **no retain option by
design** (the will is always registered with `retain=false`). The IPC (GREENGRASS) transport has
no CONNECT packet, so `lwt` is an explicit no-op there (DEBUG log).

```json
{
  "messaging": {
    "local":  { "host": "mqtt-broker.local", "port": 1883, "clientId": "bridge" },
    "lwt": {
      "topic":   "ecv1/gw-01/uns-bridge/main/state",
      "payload": { "status": "UNREACHABLE" },
      "qos": 1
    }
  }
}
```

- `topic` (required): the will topic.
- `payload`: a **string** is published verbatim as UTF-8 bytes; an **object** is serialized to
  compact JSON bytes; absent ⇒ empty payload.
- `qos`: `0` or `1`, default `1`.

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