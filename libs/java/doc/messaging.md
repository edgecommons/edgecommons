TODO: This file was GenAI generated and needs enriching/corrections

# EdgeCommons Messaging Documentation

## Overview
The EdgeCommons library provides a unified messaging abstraction layer that supports multiple runtime environments:
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
Messages in EdgeCommons follow a header-payload model consisting of:
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
   - JSON values are carried directly.
   - Small binary payloads (`byte[]`) are carried as a first-class bounded marker in
     `body._edgecommonsBinary` with `encoding: "base64"`, a decoded `length`, and
     base64 `data`. Decoded binary bodies are limited to 64 KiB; use
     `Message.isBinaryBody()` / `Message.getBinaryBody()` to detect and decode them.

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

Framework lifecycle subscriptions use `subscribeAcknowledged(...)`. Successful return means the
MQTT SUBACK was observed or the Greengrass IPC subscription operation completed; the operation has
a positive bounded timeout and never falls back to the older best-effort `subscribe`. On timeout or
failure the built-in providers remove the processor/stream and best-effort unsubscribe any late
partial subscription.

`CommandInbox` exposes `STARTING`, `ACTIVE`, `FAILED`, and `STOPPED` through `startupStatus()`.
`ACTIVE` is published only after the acknowledged subscription returns. Up to 256 deliveries that
arrive through the acknowledged transport before activation are retained in arrival order and
dispatched only after `ACTIVE`; overflow is dropped explicitly. A failed, stopped, or stale start
discards that generation's retained deliveries, and callbacks from stale restart generations never
dispatch. `stop()` unsubscribes one generation and permits a deterministic retry; `close()` is
permanent. Failure diagnostics are control-character sanitized and bounded. Runtime readiness
requires `ACTIVE`, so an acknowledged-subscription failure cannot leave a component briefly ready.

For a durable outbox, use strict confirmed publishing with an explicit QoS-1 value and bounded
timeout. MQTT returns only after Paho observes the matching PUBACK; Greengrass returns only after
the IPC publish operation completes successfully. Timeout, disconnect, interruption, and transport
errors throw `PublishConfirmationException`. A provider without acknowledgement support throws
`UnsupportedOperationException`; it never falls back to immediate `publish`.

```java
client.publishConfirmed(topic, exactEnvelopeBytes,
        Qos.AT_LEAST_ONCE, Duration.ofSeconds(5));
client.publishNorthboundConfirmed(topic, exactEnvelopeBytes,
        Qos.AT_LEAST_ONCE, Duration.ofSeconds(5));
```

Confirmed operations are bounded to 1,024 in-flight acknowledgement waits per provider. The timeout
includes waiting for that capacity, so saturation applies blocking backpressure without creating an
unbounded provider-side waiter registry.

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

#### Explicit command outcomes and deferred replies

`CommandInbox.register(...)` remains the legacy synchronous `JsonObject` handler API.
`registerOutcome(...)` adds the tagged `CommandOutcome` path:

- `ImmediateSuccess(result)` sends the normal `{ "ok": true, "result": ... }` wrapper;
- `ImmediateError(code, message)` sends the normal coded error wrapper; and
- `Deferred(token)` suppresses automatic reply and releases the inbox delivery callback as soon as
  the handler returns.

Provision before the durable insert, activate only after the insert commits, and discard on insert
failure. The application retains only the opaque handle, not the received request or its
`reply_to`.

```java
commands.registerOutcome("sb/capture", request -> {
    CommandInbox.DeferredReply deferred =
            commands.defer(request, Duration.ofSeconds(95));
    try {
        durableCatalog.insertAccepted(/* original request and effective profile */);
        deferred.activate();
        queueCapture(deferred);
        return CommandOutcome.deferred(deferred);
    } catch (Exception e) {
        deferred.discard();
        return CommandOutcome.error("PERSISTENCE_FAILED", "acceptance was not durable");
    }
});

// Later, from job completion:
deferred.settleSuccess(result);       // or settleError(code, message)
```

The inbox registry holds at most 1,024 active tokens and accepts lifetimes from 1 ms through
1,860,000 ms. A token moves `PROVISIONAL -> OPEN -> SETTLING -> SETTLED`; discard, timer expiration,
and shutdown are terminal alternatives. Settlement uses one compare-and-set winner, retries strict
guarded reply publication with bounded backoff until expiration, and exposes current state plus
registry counters through `DeferredReply.state()` and `deferredReplySnapshot()`. Missing `reply_to`
is rejected before provisioning with `REPLY_REQUIRED`. Shutdown attempts a `COMPONENT_STOPPING`
reply for each open token while messaging is still available, then marks it
`CANCELLED_ON_SHUTDOWN`.

#### Prepared and correlated application messages

`AppFacade.prepare(...)` returns `PreparedAppMessage`: the facade-generated topic, the built
`Message`, and defensively copied exact encoded bytes. `prepareCorrelated(...)` accepts either a
received request or a non-empty correlation ID and stamps it in the normal envelope header.
`publishConfirmed(prepared, routing, timeout)` sends the retained bytes rather than rebuilding the
envelope, so retries preserve the original UUID, timestamp, correlation, identity, and encoding.

```java
AppFacade.PreparedAppMessage prepared = app.prepareCorrelated(
        "ImageCaptured", "image/captured", body, request);
outbox.insert(prepared.topic(), prepared.encodedBytes());
app.publishConfirmed(prepared, Duration.ofSeconds(5));
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

### Reserved-class publish guard (UNS)

The UNS classes `state`, `metric`, `cfg` and `log` are **library-owned** (UNS-CANONICAL-DESIGN
§4.1): the heartbeat publishes the `state` keepalive, the metric subsystem publishes `metric`,
and the effective-config publisher announces `cfg`. Every publish path that takes a client-chosen
topic — `publish`, `publishConfirmed`, `publishRaw`, `publishNorthbound`,
`publishNorthboundConfirmed`, `publishNorthboundRaw`, `request`, `requestNorthbound`, and
`reply`/`replyConfirmed` (via the request's `reply_to`) — rejects a
topic whose UNS class position holds a reserved token with a `ReservedTopicException`:

```
ecv1/{device}/{component}/{instance}/{class}          // class position 4 — always checked
ecv1/{site}/{device}/{component}/{instance}/{class}   // position 5 — only when topic.includeRoot=true
```

Position 5 is checked only when *this component's* `topic.includeRoot` is `true` (late-bound from
the config, like the request-deadline default) — checking it unconditionally would false-positive
on legitimate `app` channels such as `ecv1/d/c/i/app/state`. Non-`ecv1` topics pass untouched
(`edgecommons/reply-…` reply topics, `cloudwatch/metric/put`, foreign-broker bridging), and
`subscribe*` is never guarded — consumers must read the reserved classes.

The guard is **misuse prevention, not a security boundary** (per-device broker ACLs are). The
library's own publishers reach the reserved classes through `MessagingClient.reservedPublisher()`
— a `ReservedPublisher` (`publish` / `publishRaw` / `publishNorthbound`) that bypasses the guard.
It is public only because the library publishers live in other packages; component code should
not use it.

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
   - **IoT Core MQTT client**: Direct the northbound transport connectivity
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
   - Follow Greengrass/northbound topic naming conventions

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
    "northbound": {
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

### MQTT QoS defaults (`messaging.local.qos` / `messaging.northbound.qos`)
Each broker's `qos` object configures MQTT QoS for operations on that broker that do not carry an
explicit QoS argument. Standalone local and northbound MQTT brokers accept QoS `0`, `1`, or `2`.
Greengrass northbound IPC accepts the EdgeCommons `Qos` enum but supports only QoS `0` and `1`.

```json
{
  "messaging": {
    "local":  {
      "host": "mqtt-broker.local",
      "port": 1883,
      "clientId": "bridge-local",
      "qos": { "publish": 1, "subscribe": 1 }
    },
    "northbound": {
      "host": "northbound-broker.example.com",
      "port": 8883,
      "clientId": "bridge-northbound",
      "qos": { "publish": 2, "subscribe": 1 }
    }
  }
}
```

- `local.qos.publish`: local `publish`, `publishRaw`, request publish, and reply publish.
- `local.qos.subscribe`: local `subscribe` and request reply subscriptions.
- `northbound.qos.publish`: northbound MQTT request publish and reply publish when no explicit QoS
  argument exists.
- `northbound.qos.subscribe`: northbound MQTT request reply subscriptions when no explicit QoS
  argument exists.

### MQTT Last-Will

Generic component messaging config does not define an MQTT Last-Will. The first-party LWT use is
the `uns-bridge` site-broker uplink, where the bridge derives a private Last-Will from its resolved
UNS state topic and the site broker publishes whole-device `UNREACHABLE`.

### Dual Connectivity Benefits
1. **Local Communication**: Fast, low-latency messaging for edge processing
2. **Cloud Integration**: Direct the northbound transport connectivity for telemetry and commands
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
        "northbound": {
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
