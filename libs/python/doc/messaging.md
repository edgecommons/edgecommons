# EdgeCommons Messaging Documentation

## Overview
The EdgeCommons library provides a unified messaging abstraction layer whose behavior is driven by the
`--transport` axis (derived from `--platform`):
- **`IPC` transport** (the `GREENGRASS` platform): Native AWS Greengrass IPC communication
- **`MQTT` transport** (the `HOST` platform, dual-MQTT): Dual MQTT clients for non-Greengrass
  environments (Kubernetes, Docker, etc.)

This documentation explains the key components, message structure, and usage patterns of the messaging system across all supported runtimes.

## Key Components

### MessagingClient
Applications obtain the concrete static messaging handle with `gg.get_messaging()`. It provides methods for:
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
from edgecommons import Qos

# Get the concrete messaging handle
messaging_service = gg.get_messaging()

# Publishing to local broker
messaging_service.publish(topic, message)

# Publishing northbound
messaging_service.publish_northbound(topic, message, Qos.AT_LEAST_ONCE)

# Subscribing to local broker
messaging_service.subscribe(topic_filter, message_handler)

# Subscribing northbound
messaging_service.subscribe_northbound(topic_filter, message_handler, Qos.AT_MOST_ONCE)
```

#### 2. Request-Reply
Synchronous and asynchronous request-reply patterns:
```python
# Making a request to local broker
future = messaging_service.request(topic, request_message)
success, response = future.get(timeout_seconds)  # Blocking

# Making a request northbound
future = messaging_service.request_northbound(topic, request_message)
success, response = future.get(timeout_seconds)  # Blocking

# Replying to requests (automatically routes to correct broker)
messaging_service.reply(request_message, reply_message)
```

##### Request deadline (`messaging.requestTimeoutSeconds`)
Every `request()` carries a **framework-owned deadline** (default **30 s**, configurable via
`messaging.requestTimeoutSeconds` in the component config; `0` disables). When it expires the
library unsubscribes the ephemeral reply topic, removes the pending entry and completes the `Iou`
**exceptionally** — a waiting (or later) `iou.get()` **raises** `RequestTimeoutError`
(`edgecommons.messaging.errors`) instead of blocking forever, even if the caller never calls
`get()`, so an unanswered request can no longer leak its reply subscription. Reply-arrival, the
deadline and `cancel_request` settle a request exactly once (idempotent); a straggler reply after
settle is dropped with a DEBUG log.

```python
from edgecommons import RequestTimeoutError

# Per-call deadline: an explicit value always wins over the configured default.
iou = MessagingClient.request(topic, msg, timeout_secs=5)
# 0 disables the deadline for this one call.
iou = MessagingClient.request(topic, msg, timeout_secs=0)

try:
    done, reply = iou.get(timeout=10)
except RequestTimeoutError:
    ...  # the framework deadline fired; issue a FRESH request to retry
```

Note (init order): the messaging client is initialized before the config loads, so the configured
default is late-bound right after the `ConfigManager` exists; until then the built-in 30 s applies
(deliberately — the CONFIG_COMPONENT bootstrap request gets a deadline too).

#### Deferred command replies

Handlers registered with `CommandInbox.register(...)` retain the existing immediate `dict`/`None`
contract. Long-running commands use the additive `register_outcome(...)` contract and return one of
`ImmediateSuccess`, `ImmediateError`, or `Deferred`:

```python
from edgecommons import CommandOutcome

def capture(request):
    # Provision first. This rejects fire-and-forget requests with REPLY_REQUIRED,
    # validates the guarded reply_to, and reserves one bounded registry entry.
    token = commands.defer(request, lifetime_secs=95)
    try:
        capture_id = catalog.insert_accepted(request.get_body())
    except Exception as error:
        token.discard()
        return CommandOutcome.error("CATALOG_ERROR", str(error))

    token.activate()                 # only after durable acceptance commits

    def finish():
        token.settle_success({"captureId": capture_id, "state": "SUCCEEDED"})

    return CommandOutcome.deferred_with_continuation(token, finish)

commands.register_outcome("sb/capture", capture)

# Later, from the worker that owns the terminal result. Exactly one concurrent
# settler receives SettlementResult.ACCEPTED.
token.settle_success({"captureId": capture_id, "state": "SUCCEEDED"})
# or: token.settle_error("CAPTURE_FAILED", "camera did not return a frame")
```

The inbox owns a maximum of 1,024 provisional/open/settling replies. A token is opaque: component
code cannot read or directly publish to the retained reply topic. The registry retains only guarded
reply metadata, expires tokens from an explicit timer (maximum lifetime 31 minutes), and retries a
failed confirmed reply with the same envelope UUID until expiration. An open token that expires
records a stable diagnostic. `close()` makes new deferrals fail, attempts a bounded
`COMPONENT_STOPPING` reply for open tokens while messaging is still available, and marks them
`CANCELLED_ON_SHUTDOWN`. Deferred paths are ephemeral and are not recovered after restart; durable
application status and terminal messages provide recovery.

`CommandOutcome.deferred_with_continuation(token, callback)` is the race-free handoff when work
must start asynchronously after durable acceptance. The inbox validates the exact `OPEN` token
before starting its bounded callback (maximum 256 running or queued); it never invokes the
callback for an invalid token. The callback captures and settles the opaque token and never gets a
raw reply topic. `CommandOutcome.deferred(token)` remains available for established callers.

#### Observable command-inbox activation

`CommandInbox.start(timeout_secs=10)` exposes a `CommandInboxStartupStatus` with `STARTING`,
`ACTIVE`, `FAILED`, or `STOPPED`. `ACTIVE` means all built-in and builder-configured component
handlers exist, the exact inbox filter was submitted, and MQTT SUBACK or the Greengrass initial
subscription response succeeded. Failure retains a bounded sanitized diagnostic and performs
best-effort partial-subscription cleanup; `stop()` invalidates that generation and a later `start()`
can retry. `close()` is terminal.

Deliveries that race acknowledged subscribe are not lost: the inbox retains at most 256 while
`STARTING` and while the activation drain runs, preserves arrival order, and drops the newest with a
warning on overflow. Callbacks are generation-bound, so a stopped/failed generation cannot dispatch
after restart. Components install handlers before subscribe through the builder:

```python
from edgecommons import EdgeCommonsBuilder

gg = (
    EdgeCommonsBuilder.create("com.example.CameraAdapter")
    .configure_commands(
        lambda inbox: inbox.register_outcome("sb/capture", capture)
    )
    .build()
)
```

`MessagingClient.subscribe_acknowledged(...)` is the additive strict transport primitive. It never
falls back to ordinary `subscribe(...)` on a provider that cannot prove acknowledgement.

#### Confirmed publication and prepared application messages

Ordinary `publish(...)` retains its immediate submission semantics. A durable outbox uses the strict
confirmed path with explicit QoS 1 and a positive timeout:

```python
from edgecommons import Qos

MessagingClient.publish_confirmed(
    topic, exact_encoded_envelope, Qos.AT_LEAST_ONCE, timeout_secs=5
)
```

On MQTT, success means the matching broker PUBACK was observed. On Greengrass, success means the IPC
publish operation completed successfully. Timeout, disconnect, and a provider without positive
confirmation support raise; an ambiguous timeout is never reported as success. Exact-byte calls first
parse the value through the canonical EdgeCommons envelope codec and reject malformed bytes before
touching the transport, while still sending the caller's original representation after validation.

The `app()` facade can prepare one stable envelope before storing or publishing it:

```python
prepared = gg.app().prepare_correlated(
    "ImageCaptured",
    "image/captured",
    {"captureId": capture_id, "absolutePath": absolute_path},
    request,                           # or an explicit correlation-id string
)

outbox.insert(prepared.topic, prepared.encoded_bytes)
gg.app().publish_confirmed(prepared, timeout_secs=5)
```

`PreparedAppMessage` contains the facade-generated topic, the `Message`, and exact serialized bytes.
Retries of the same prepared value therefore preserve the UUID, timestamp, correlation, identity,
body, and encoded envelope. `publish_prepared(...)` uses the existing immediate path;
`publish_confirmed(...)` propagates failures, including for northbound routing, so an outbox can
leave the record pending.

### Reserved-class publish guard (UNS)

The UNS classes `state`, `metric`, `cfg` and `log` are **library-owned** (UNS-CANONICAL-DESIGN
§4.1): the heartbeat publishes the `state` keepalive, the metric subsystem publishes `metric`,
and the effective-config publisher announces `cfg`. Every publish path that takes a client-chosen
topic — `publish`, `publish_raw`, `publish_northbound`, `publish_northbound_raw`, `request`,
`request_northbound`, and `reply`/`reply_northbound` (via the request's `reply_to`) — rejects
a topic whose UNS class position holds a reserved token with a `ReservedTopicError`:

```
ecv1/{device}/{component}/{instance}/{class}          # class position 4 - always checked
ecv1/{site}/{device}/{component}/{instance}/{class}   # position 5 - only when topic.includeRoot=true
```

Position 5 is checked only when *this component's* effective root mode is on
(`topic.includeRoot` AND a multi-level hierarchy; late-bound from the config, like the
request-deadline default) — checking it unconditionally would false-positive on legitimate `app`
channels such as `ecv1/d/c/i/app/state`. Non-`ecv1` topics pass untouched (`edgecommons/reply-…`
reply topics, `cloudwatch/metric/put`, foreign-broker bridging), and `subscribe*` is never
guarded — consumers must read the reserved classes.

The guard is **misuse prevention, not a security boundary** (per-device broker ACLs are). The
library's own publishers reach the reserved classes through the
`MessagingClient._publish_reserved` / `_publish_reserved_raw` / `_publish_reserved_northbound`
staticmethods, which bypass the guard. The underscore marks them library-internal — component
code should not use them.

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
from edgecommons.builders import MessageBuilder

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

## Message Identity (UNS)

Messages built with a config-bound builder carry a top-level **`identity`** element
(UNS-CANONICAL-DESIGN §1) between `header` and `tags`: the ordered enterprise hierarchy (`hier`,
whose **last entry is the device**), the precomputed `path`, the publishing `component` token and
the optional per-message `instance` (present ⇒ instance-scoped, absent ⇒ component/global scope;
omitted from the wire when absent and never a reserved class token).

```python
identity = message.get_identity()      # MessageIdentity or None
if identity is not None:
    identity.device                    # last hier entry's value (computed, not a wire field)
    identity.path                      # "dallas/zone-3/gw-01"
    identity.component, identity.instance
```

`MessageBuilder.build()` is the single stamping site: an explicit `with_identity(...)` override
wins; otherwise a `with_config(...)` builder stamps the component's resolved identity with the
optional `with_instance(...)` token (absent ⇒ component scope, no `instance` key); with neither,
`identity` stays `None`
(bootstrap/raw messages legally omit it). Inbound parsing is lenient: a malformed `identity`
yields `None` plus a WARN and the message still delivers.

## Message Tags
Tags provide contextual information about messages and can be used for:
- Source identification
- Message routing
- Filtering and processing logic
- Debugging and tracking

Tags are automatically populated from the `tags` config section and exposed as a plain dict:
```python
message_tags = message.get_tags()
site = message_tags.tags.get("site")
```

The legacy `tags.thing` special-casing is **removed** (hard cut): the publisher's device now
travels in the top-level `identity` element; a stray inbound `thing` key is just a generic tag.

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
    "northbound": {
      "endpoint": "your-endpoint.iot.us-east-1.amazonaws.com",
      "port": 8883,
      "clientId": "my-component-northbound",
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

### MQTT Last-Will

Generic component messaging config does not define an MQTT Last-Will. The first-party LWT use is
the `uns-bridge` site-broker uplink, where the bridge derives a private Last-Will from its resolved
UNS state topic and the site broker publishes whole-device `UNREACHABLE`.

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
        "northbound": {
          "endpoint": "your-endpoint.iot.us-east-1.amazonaws.com",
          "port": 8883,
          "clientId": "my-component-northbound",
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
Test that local and northbound subscriptions work independently:
```python
def test_dual_subscription():
    topic = "test/dualTopic"
    local_received = []
    northbound_received = []
    
    def local_handler(t, m):
        local_received.append(m)
        
    def northbound_handler(t, m):
        northbound_received.append(m)
    
    # Subscribe to same topic on both brokers
    messaging_service.subscribe(topic, local_handler)
    messaging_service.subscribe_northbound(topic, northbound_handler, Qos.AT_MOST_ONCE)
    
    # Publish to local - only local handler should receive
    local_msg = MessageBuilder.create("LocalMessage", "1.0") \
        .with_payload({"source": "local"}) \
        .build()
    messaging_service.publish(topic, local_msg)
    
    # Publish northbound - only northbound handler should receive
    northbound_msg = MessageBuilder.create("NorthboundMessage", "1.0") \
        .with_payload({"source": "northbound"}) \
        .build()
    messaging_service.publish_northbound(topic, northbound_msg, Qos.AT_LEAST_ONCE)
```

### Connection Management
The STANDALONE provider includes robust connection management:
- **Blocking connections**: Waits for connection confirmation before proceeding
- **Blocking subscriptions**: built-in MQTT subscriptions wait for positive SUBACK; lifecycle code
  uses the explicit `subscribe_acknowledged` contract with its own timeout
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

### Accessing messaging
```python
messaging_service = gg.get_messaging()
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
      "edgecommons.messaging": "DEBUG"
    }
  }
}
```

### Connection Verification
```python
# Check native clients
clients = messaging_service.get_native_client()
local_client = clients.get('local')
northbound_client = clients.get('northbound')

print(f"Local connected: {local_client.is_connected() if local_client else False}")
print(f"Northbound connected: {northbound_client.is_connected() if northbound_client else False}")
```
