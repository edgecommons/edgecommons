TODO: This file was GenAI generated and needs enriching/corrections

# GGCommons Messaging Documentation

## Overview
The GGCommons library provides a unified messaging abstraction layer that supports both AWS Greengrass IPC and MQTT-based communication patterns. This documentation explains the key components, message structure, and usage patterns of the messaging system.

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

### Providers
The library includes two messaging providers:

1. **GreengrassIpcProvider**: 
   - Implements native Greengrass v2 IPC communication
   - Used in production deployments on Greengrass cores

2. **MqttProvider**: 
   - Simulates Greengrass IPC behavior using MQTT
   - Useful for development and debugging
   - Supports local testing without Greengrass runtime

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

## Development vs Production
- Use MQTT provider during development for easier debugging
- Switch to Greengrass IPC provider for production deployments
- Code remains unchanged; only provider initialization differs

## Common Use Cases

1. **Component Communication**
   - Inter-component messaging in Greengrass
   - Local development using MQTT simulation

2. **Request-Response Workflows**
   - Service invocation patterns
   - Blocking and non-blocking requests

3. **Event Broadcasting**
   - Publishing state changes
   - Broadcasting metrics or telemetry

4. **Configuration Updates**
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