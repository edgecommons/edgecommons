# Java Component Skeleton

A sample Java component demonstrating best practices for using the GGCommons library. This component showcases configuration management, messaging patterns, metrics emission, and proper resource cleanup using modern service-oriented architecture.

## Features

- **Configuration Management**: Dynamic configuration loading with change listeners
- **Dual Messaging**: Supports both local MQTT and AWS IoT Core connectivity
- **Request-Reply Pattern**: Demonstrates synchronous communication patterns
- **Metrics Emission**: Performance and error metrics collection
- **Graceful Shutdown**: Proper resource cleanup and subscription management
- **HOST Platform**: Run outside Greengrass in Docker or any container runtime (MQTT transport)

## Running the Component

### GREENGRASS Platform (Traditional)
```bash
# Build the component
mvn clean package

# Run on the GREENGRASS platform (IPC transport)
java -jar target/java-component-skeleton-1.0.0.jar --platform GREENGRASS -c FILE ./test-configs/config_2.json -t my-thing-name
```

### HOST Platform (Container-Ready)
```bash
# Build the component
mvn clean package

# Run on the HOST platform with the MQTT transport (dual MQTT connectivity)
java -jar target/java-component-skeleton-1.0.0.jar --platform HOST --transport MQTT ./standalone-messaging.json -c FILE ./test-configs/config_2.json -t my-thing-name
```

## Configuration

### Component Configuration (`config_2.json`)
The component uses a JSON configuration file that includes:
- Logging configuration with per-logger levels
- **UNS identity** — top-level `hierarchy` (ordered levels; the last is always the resolved thing
  name) + `identity` (a value for every level above the last). The library stamps this identity
  into every envelope built with `.withConfig(...)` and into every topic minted via `gg.getUns()`
- Heartbeat settings (`{enabled, intervalSecs, measures, destination}` — the heartbeat is the
  library-owned `state` keepalive on `ecv1/{device}/{component}/main/state`, on/5s/local by default)
- Metric emission configuration
- Component-specific settings (publish interval, etc.)

### MQTT Transport Configuration (`standalone-messaging.json`)
For non-Greengrass deployments, create a messaging configuration file:
```json
{
  "messaging": {
    "local": {
      "host": "localhost",
      "port": 1883,
      "clientId": "java-component-skeleton-local",
      "credentials": {
        "username": "mqtt-user",
        "password": "mqtt-password"
      }
    },
    "iotCore": {
      "endpoint": "your-iot-endpoint.iot.us-east-1.amazonaws.com",
      "port": 8883,
      "clientId": "java-component-skeleton-iotcore",
      "credentials": {
        "certPath": "/path/to/device-cert.pem",
        "keyPath": "/path/to/private-key.pem",
        "caPath": "/path/to/root-ca.pem"
      }
    }
  }
}
```

## Component Behavior

1. **Initialization**: Sets up services, configuration, and subscriptions; mints its publish and
   request topics via the UNS topic builder (`gg.getUns().topic(UnsClass.APP, ...)` →
   `ecv1/{device}/{component}/main/app/hello-world` and `.../app/request`)
2. **Request-Reply Demo**: Sends sample requests and processes replies
3. **Message Publishing**: Continuously publishes hello world messages to both local and IoT Core
4. **Metrics Emission**: Emits performance metrics including message count and latency
5. **Configuration Changes**: Responds to runtime configuration updates
6. **Graceful Shutdown**: Cleans up subscriptions and resources on termination

## Deployment Options

### AWS IoT Greengrass
- Traditional Greengrass component deployment
- Uses Greengrass IPC for inter-component communication
- Managed by Greengrass runtime

### Kubernetes

On the `KUBERNETES` platform the transport defaults to `MQTT` and the config source to `CONFIGMAP`
(mount dir `/etc/ggcommons`, key `config.json`). A single mounted ConfigMap file then doubles as
**both** the messaging config (its `messaging` section) **and** the component config — so you pass
**no** positional `--transport MQTT <path>` and **no** `-c` flag. Identity comes from the Downward
API: set `GGCOMMONS_THING_NAME` (from the `ggcommons.io/thing-name` pod annotation or an explicit
value) or fall back to `POD_NAME` (`metadata.name`).

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: java-component-skeleton
spec:
  replicas: 1
  selector:
    matchLabels:
      app: java-component-skeleton
  template:
    metadata:
      labels:
        app: java-component-skeleton
    spec:
      containers:
      - name: component
        image: java-component-skeleton:latest
        # KUBERNETES => MQTT + CONFIGMAP by default; the messaging-config path defaults to the
        # mounted ConfigMap file (/etc/ggcommons/config.json), so no positional path is needed.
        args: ["--platform", "KUBERNETES"]
        env:
        - name: GGCOMMONS_THING_NAME           # identity from the Downward API (FR-RT-7)
          valueFrom:
            fieldRef:
              fieldPath: metadata.annotations['ggcommons.io/thing-name']
        - name: POD_NAME                        # fallback identity
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        volumeMounts:
        - name: config
          mountPath: /etc/ggcommons             # ConfigMap mounted as a whole volume (NOT subPath)
        - name: certs
          mountPath: /certs
      volumes:
      - name: config
        configMap:
          name: component-config                # must contain a 'config.json' key
      - name: certs
        secret:
          name: iot-certificates                # only needed for the dual-MQTT (IoT Core) topology
```

### Docker
```bash
# Build Docker image
docker build -t java-component-skeleton .

# Run with volume mounts for configuration
docker run -v $(pwd)/config:/config -v $(pwd)/certs:/certs \
  java-component-skeleton \
  --platform HOST --transport MQTT /config/messaging.json -c FILE /config/config.json -t my-thing
```

## Development

### Building
```bash
mvn clean package
```

### Testing Locally
1. Start a local MQTT broker (e.g., Mosquitto)
2. Update `standalone-messaging.json` with local broker details
3. Run on the HOST platform with the MQTT transport

### Monitoring
- Check logs for component activity
- Monitor metrics emission in configured target (CloudWatch, logs, etc.)
- Use MQTT client tools to observe message flow on the UNS topics: subscribe to
  `ecv1/+/+/+/state` for heartbeat keepalives and `ecv1/+/+/+/app/#` for the demo app messages

## Best Practices Demonstrated

- **Service-oriented architecture** with dependency injection
- **Modern Java patterns** with lambda expressions and CompletableFuture
- **Proper error handling** with try-catch blocks and timeouts
- **Resource management** with graceful shutdown and cleanup
- **Configuration management** with change listeners
- **Asynchronous processing** for non-blocking operations
- **Comprehensive logging** with appropriate log levels
- **Metrics collection** for monitoring and observability

## License

Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
SPDX-License-Identifier: Apache-2.0

