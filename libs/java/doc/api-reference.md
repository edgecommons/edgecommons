# EdgeCommons Java Library - API Reference

This document provides a complete reference for the public API of the EdgeCommons Java library.

## Table of Contents

1. [Core Classes](#core-classes)
2. [Configuration API](#configuration-api)
3. [Messaging API](#messaging-api)
4. [Metrics API](#metrics-api)
5. [Utility Classes](#utility-classes)
6. [Interfaces](#interfaces)
7. [Exceptions](#exceptions)

---

## Core Classes

### EdgeCommons

Main entry point for the EdgeCommons framework.

Prefer `EdgeCommonsBuilder`. Lifecycle-sensitive components can set the readiness gate before any
runtime initialization, install all command handlers before the inbox subscribes, and register
ordered pre-commit configuration validators:

```java
EdgeCommons gg = EdgeCommonsBuilder.create("com.example.CameraAdapter")
        .withArgs(args)
        .initialReady(false)
        .withConfigValidationTimeout(Duration.ofSeconds(5))
        .withConfigurationValidator("camera-endpoints", (candidate, prior, phase) ->
                validateEndpoints(candidate, prior, phase))
        .configureCommands(inbox -> inbox.register("sb/capture", this::capture))
        .build();
// Finish component-owned startup, then explicitly release the application gate.
gg.setReady(true);
```

`initialReady(false)` is applied before argument parsing, transport setup, provider startup, or the
health endpoint, so readiness cannot briefly report true. Runtime readiness also requires the
command inbox to be `ACTIVE`.

#### Constructors

```java
public EdgeCommons(String componentName, String[] args)
```
Creates a EdgeCommons instance with default options.
- **componentName**: Fully qualified component name
- **args**: Command line arguments

```java
public EdgeCommons(String componentName, String[] args, Options appOptions)
```
Creates a EdgeCommons instance with custom command line options.
- **appOptions**: Additional Apache Commons CLI options

```java
public EdgeCommons(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
```
Creates a EdgeCommons instance with full customization.
- **receiveOwnMessages**: Whether to receive messages published by this component (IPC only)

#### Methods

```java
public ConfigManager getConfigManager()
```
Returns the configuration manager instance.

```java
public LogService getLogs()
```
Returns the structured UNS log publisher. It publishes `edgecommons.log.v1` records on
`ecv1/{device}/{component}/main/log/{level}` through the reserved `log` class seam.

```java
public static ParsedCommandLine processArgs(String componentName, String[] args, Options appOptions)
```
Processes command line arguments and returns parsed result.

---

## Configuration API

### ConfigManager

Manages component configuration from multiple sources.

#### Methods

```java
public JsonObject getGlobalConfig()
```
Returns the global configuration section shared across all instances.

```java
public Collection<String> getInstanceIds()
```
Returns collection of all configured instance IDs.

```java
public JsonObject getInstanceConfig(String instanceId)
```
Returns configuration for a specific instance.
- **instanceId**: The instance identifier

```java
public JsonObject getFullConfig()
```
Returns a defensive copy of the atomically committed configuration generation.

```java
public boolean tryApplyConfig(JsonObject candidate)
public long getConfigGeneration()
public List<ConfigurationValidationError> getLastCandidateValidationErrors()
```

`tryApplyConfig` runs schema validation and every registered component validator before commit.
Validators receive defensive copies of the candidate and the redacted prior snapshot, plus
`INITIAL` or `RELOAD`. They share one bounded deadline. Reject, timeout, callback failure, or
preparation failure retains the exact prior generation and does not invoke change listeners or the
effective-config publisher. A successful candidate is prepared off to the side, atomically committed,
then logging/effective publication/listeners observe the new generation.

```java
public TagConfiguration getTagConfig()
```
Returns the tag configuration.

```java
public HeartbeatConfiguration getHeartbeatConfig()
```
Returns the heartbeat configuration.

```java
public LoggingConfiguration getLoggingConfig()
```
Returns the logging configuration.

```java
public MetricConfiguration getMetricConfig()
```
Returns the metric emission configuration.

```java
public String getThingName()
```
Returns the AWS IoT Thing name.

```java
public String getComponentName()
```
Returns the short component name.

```java
public String getComponentFullName()
```
Returns the fully qualified component name.

```java
public String resolveTemplate(String template)
```
Resolves template variables in a string.
- **template**: String containing template variables like `{ThingName}`

```java
public void addConfigChangeListener(ConfigurationChangeListener listener)
```
Registers a configuration change listener.

```java
public void removeConfigChangeListener(ConfigurationChangeListener listener)
```
Removes a configuration change listener.

```java
public void notifyConfigurationChanged()
```
Manually triggers configuration change notifications.

### Configuration Classes

#### TagConfiguration

```java
public Set<String> getKeys()
```
Returns all tag keys.

```java
public String getKeyValue(String key)
```
Returns the value for a specific tag key.

#### HeartbeatConfiguration

```java
public boolean isEnabled()
```
Whether the heartbeat (state keepalive + `sys` measures metric) runs (default `true`).

```java
public int getIntervalSecs()
```
Returns the heartbeat interval in seconds.

```java
public String getDestination()
```
The state keepalive's publish destination — `"local"` (default) or `"northbound"`. (The legacy
`targets[]` array is removed; the measures route through the metric subsystem as the `sys` metric.)

```java
public HeartbeatMeasures getMeasures()
```
Returns configured measures to collect.

#### MetricConfiguration

```java
public String getTarget()
```
Returns the metric emission target type.

```java
public String getNamespace()
```
Returns the metric namespace.

```java
public String getLogFileNameTemplate()
```
Returns the log file name template (for log target).

```java
public String getMaxFileSize()
```
Returns the maximum file size for log rotation.

```java
public String getTopic()
```
Returns the fixed `cloudwatch/metric/put` contract topic for the `cloudwatchcomponent` target, or
`null` otherwise. (The `messaging` target publishes to the UNS metric topic
`ecv1/{device}/{component}/main/metric/{metricName}` — no configured topic.)

```java
public int getIntervalSecs()
```
Returns the emission interval in seconds.

```java
public String getDestination()
```
Returns the messaging destination.

```java
public boolean getLargeFleetWorkaround()
```
Returns whether large fleet workaround is enabled.

#### LoggingConfiguration

```java
public Level getLevel()
```
Returns the root logging level.

```java
public String getFormat()
```
Returns the log message format pattern.

```java
public boolean isFileLoggingEnabled()
```
Returns whether file logging is enabled.

```java
public String getLogFilePath()
```
Returns the log file path template.

```java
public Map<String, Level> getLoggerLevels()
```
Returns map of logger names to their specific levels.

```java
public LogPublishConfiguration getPublishConfig()
```
Returns the parsed `logging.publish` configuration for the structured log bus publisher.

#### LogService

```java
public void publish(LogRecord record)
```
Queues a structured log record for publication. The call is non-blocking; if the bounded queue is full,
the oldest queued record is dropped and counted.

```java
public boolean flush(Duration timeout)
```
Waits for records queued at the time of the call to publish or for the timeout to expire.

```java
public LogStats stats()
```
Returns publisher counters: enqueued, published, dropped, filtered, redacted, truncated, failed, and
currently queued records.

---

## Messaging API

### MessagingClient

Static utility class for messaging operations.

#### Subscription Methods

```java
public static void subscribe(String topic, MessageHandler handler, int maxMessages)
```
Subscribes to IPC messages.
- **topic**: Topic pattern (supports wildcards)
- **handler**: Message handler function
- **maxMessages**: Maximum concurrent messages

```java
public static void subscribeNorthbound(String topic, MessageHandler handler, Qos qos)
```
Subscribes to northbound messages.

```java
public static void subscribeNorthbound(String topic, MessageHandler handler, Qos qos, int maxMessages)
```
Subscribes to northbound messages with concurrency control.

#### Publishing Methods

```java
public static void publish(String topic, Message message)
```
Publishes message via IPC.

```java
public static void publishNorthbound(String topic, Message message, Qos qos)
```
Publishes message to the configured northbound transport.

```java
public void publishConfirmed(String topic, Message message, Qos qos, Duration timeout)
public void publishConfirmed(String topic, byte[] encodedMessage, Qos qos, Duration timeout)
public void publishNorthboundConfirmed(String topic, Message message, Qos qos, Duration timeout)
public void publishNorthboundConfirmed(String topic, byte[] encodedMessage, Qos qos, Duration timeout)
```
Publishes at explicit `Qos.AT_LEAST_ONCE` and returns only after broker PUBACK (MQTT) or successful
Greengrass IPC operation completion. The byte-array overload retains exact durable-outbox bytes.
Failure throws `PublishConfirmationException`; unsupported providers throw
`UnsupportedOperationException`.

```java
public static void publishRaw(String topic, JsonObject payload)
```
Publishes raw JSON payload via IPC.

#### Request-Response Methods

```java
public static CompletableFuture<Message> request(String topic, Message message)
```
Sends request via IPC and returns future for response.

```java
public static CompletableFuture<Message> requestNorthbound(String topic, Message message)
```
Sends request via the configured northbound transport and returns future for response.

```java
public static void reply(Message originalMessage, Message replyMessage)
```
Sends reply to a received message.

```java
public void replyConfirmed(Message originalMessage, Message replyMessage, Duration timeout)
public void replyNorthboundConfirmed(Message originalMessage, Message replyMessage, Duration timeout)
```
Sends a guarded correlated reply through the strict acknowledgement path.

### Command outcomes

```java
public void registerOutcome(String verb, OutcomeCommandHandler handler)
public DeferredReply defer(Message request, Duration lifetime) throws CommandException
public DeferredReplySnapshot deferredReplySnapshot()
```

`OutcomeCommandHandler` returns one of `CommandOutcome.ImmediateSuccess`,
`CommandOutcome.ImmediateError`, or `CommandOutcome.Deferred`. `DeferredReply` exposes
`activate()`, `discard()`, `settleSuccess(...)`, `settleError(...)`, and `state()`. The inbox owns
the guarded reply metadata, timer, retry policy, 1,024-entry capacity bound, and shutdown
settlement.

For asynchronous application work, return
`CommandOutcome.deferredWithContinuation(token, continuation)` after the durable commit and
`token.activate()`. The inbox validates that the exact token is still `OPEN`, then starts the
bounded continuation (maximum 256 running or queued). The continuation captures and settles the
opaque token; it never receives a raw reply topic. The legacy `deferred(token)` form remains
unchanged.

### Prepared application messages

```java
public PreparedAppMessage prepare(String name, String channel, JsonObject body)
public PreparedAppMessage prepareCorrelated(String name, String channel, JsonObject body,
                                             Message request)
public PreparedAppMessage prepareCorrelated(String name, String channel, JsonObject body,
                                             String correlationId)
public void publishConfirmed(PreparedAppMessage prepared, Duration timeout)
public void publishConfirmed(PreparedAppMessage prepared, Channel routing, Duration timeout)
```

`PreparedAppMessage` carries `topic()`, `message()`, and defensive `encodedBytes()`. Confirmed
publication uses those exact bytes so outbox retries preserve UUID and envelope encoding.

### Message

Represents a message with header and payload.

#### Static Factory Methods

```java
public static Message buildFromConfig(String name, String version, JsonObject payload, ConfigManager configManager)
```
Creates message with automatic header population.

#### Methods

```java
public MessageHeader getHeader()
```
Returns the message header.

```java
public JsonObject getPayload()
```
Returns the message payload.

```java
public JsonObject getRaw()
```
Returns raw message content (for messages without headers).

```java
public String getCorrelationId()
```
Returns the correlation ID for request-response patterns.

### MessageHeader

Message header with metadata.

#### Methods

```java
public String getName()
```
Returns the message name.

```java
public String getVersion()
```
Returns the message version.

```java
public String getReplyTo()
```
Returns the reply-to topic.

```java
public String getCorrelationId()
```
Returns the correlation ID.

```java
public long getTimestamp()
```
Returns the message timestamp.

```java
public MessageTags getTags()
```
Returns the message tags.

---

## Metrics API

### MetricEmitter

Static utility class for metric operations.

#### Methods

```java
public static void init(ConfigManager configManager)
```
Initializes the metric emitter (called automatically by EdgeCommons).

```java
public static void defineMetric(Metric metric)
```
Defines a new metric for emission.

```java
public static void emitMetric(String name, Map<String, Float> measureValues)
```
Emits metric values (may be batched).

```java
public static void emitMetricNow(String name, Map<String, Float> measureValues)
```
Immediately emits metric values (bypasses batching).

### Metric

Represents a metric definition with measures and dimensions.

#### Constructors

```java
public Metric(String name)
```
Creates metric with default namespace and dimensions.

```java
public Metric(String name, String namespace)
```
Creates metric with custom namespace.

#### Methods

```java
public String getName()
```
Returns the metric name.

```java
public String getNamespace()
```
Returns the metric namespace.

```java
public void addMeasure(Measure measure)
```
Adds a measure to the metric.

```java
public Measure getMeasure(String name)
```
Returns a specific measure by name.

```java
public Map<String, Measure> getMeasures()
```
Returns all measures.

```java
public void addDimension(String key, String value)
```
Adds a custom dimension.

```java
public Map<String, String> getDimensions()
```
Returns all dimensions.

### Measure

Represents a metric measure with unit and storage resolution.

#### Constructor

```java
public Measure(String name, String unit, int storageResolution)
```
Creates a measure.
- **name**: Measure name
- **unit**: CloudWatch unit (e.g., "Count", "Bytes", "Percent")
- **storageResolution**: Storage resolution in seconds (1 or 60)

#### Methods

```java
public String getName()
```
Returns the measure name.

```java
public String getUnit()
```
Returns the CloudWatch unit.

```java
public int getStorageResolution()
```
Returns the storage resolution.

---

## Utility Classes

### Utils

General utility methods.

#### Methods

```java
public static void sleep(long milliseconds)
```
Thread sleep utility that handles InterruptedException.

### ParsedCommandLine

Contains parsed command line arguments.

#### Fields

```java
public CommandLine commandLine
```
Apache Commons CLI CommandLine object.

```java
public String[] configArgs
```
Configuration source arguments.

```java
public String[] messagingArgs
```
Messaging provider arguments.

```java
public String thingName
```
Specified thing name.

---

## Interfaces

### ConfigurationChangeListener

Interface for receiving configuration change notifications.

```java
public interface ConfigurationChangeListener {
    boolean onConfigurationChanged();
}
```

Implement this interface to receive notifications when configuration changes.
Return `true` if the change was handled successfully.

### MessageHandler

Functional interface for handling received messages.

```java
@FunctionalInterface
public interface MessageHandler {
    void handle(String topic, Message message);
}
```

Used with subscription methods to process incoming messages.

---

## Exceptions

### Custom Exception Hierarchy

The library uses standard Java exceptions. Common exceptions you may encounter:

#### RuntimeExceptions
- **IllegalArgumentException**: Invalid configuration or parameters
- **IllegalStateException**: Component not properly initialized

#### Checked Exceptions
- **IOException**: File or network I/O errors
- **InterruptedException**: Thread interruption during operations
- **ExecutionException**: Errors in asynchronous operations
- **TimeoutException**: Request timeout in request-response patterns

---

## Usage Patterns

### Basic Component Initialization

```java
public class MyComponent {
    private EdgeCommons edgeCommons;
    private ConfigManager configManager;
    
    public void initialize(String[] args) {
        edgeCommons = new EdgeCommons("com.example.MyComponent", args);
        configManager = edgeCommons.getConfigManager();
    }
}
```

### Configuration Access Pattern

```java
// Access global configuration
JsonObject global = configManager.getGlobalConfig();
String serverUrl = global.get("serverUrl").getAsString();

// Process all instances
for (String instanceId : configManager.getInstanceIds()) {
    JsonObject instance = configManager.getInstanceConfig(instanceId);
    processInstance(instanceId, instance);
}
```

### Messaging Pattern

```java
// Subscribe to messages
MessagingClient.subscribe("data/+", (topic, message) -> {
    JsonObject payload = message.getPayload();
    processData(payload);
}, 10);

// Publish message
JsonObject data = new JsonObject();
data.addProperty("value", 42);
Message msg = Message.buildFromConfig("DataUpdate", "1.0", data, configManager);
MessagingClient.publish("data/sensor1", msg);
```

### Metrics Pattern

```java
// Define metric
Metric metric = new Metric("data_processed");
metric.addMeasure(new Measure("count", "Count", 1));
metric.addMeasure(new Measure("bytes", "Bytes", 1));
MetricEmitter.defineMetric(metric);

// Emit values
Map<String, Float> values = Map.of(
    "count", 100.0f,
    "bytes", 1024.0f
);
MetricEmitter.emitMetric("data_processed", values);
```

### Configuration Change Handling

```java
public class MyConfigListener implements ConfigurationChangeListener {
    @Override
    public boolean onConfigurationChanged() {
        try {
            reloadConfiguration();
            restartServices();
            return true;
        } catch (Exception e) {
            logger.error("Failed to handle configuration change", e);
            return false;
        }
    }
}

// Register listener
configManager.addConfigChangeListener(new MyConfigListener());
```

---

## Thread Safety

### Thread-Safe Classes
- **MessagingClient**: All methods are thread-safe
- **MetricEmitter**: All methods are thread-safe
- **ConfigManager**: Read operations are thread-safe

### Non-Thread-Safe Classes
- **Message**: Immutable after creation
- **Metric**: Not thread-safe during construction, immutable after definition
- **Configuration objects**: Immutable after creation

### Best Practices
- Access configuration objects from multiple threads safely
- Message handlers may be called concurrently
- Metric emission is safe from multiple threads
- Configuration change listeners should be thread-safe
