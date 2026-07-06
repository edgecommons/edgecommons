# Builder Pattern Migration Guide

This document outlines the recommended client changes for migrating from deprecated constructors and factory methods to the new builder pattern implementations in EdgeCommons.

## Overview

The following classes now provide builder patterns for more flexible and maintainable object construction:

- `EdgeCommons` → `EdgeCommonsBuilder`
- `Message` → `MessageBuilder`
- `Metric` → `MetricBuilder`

## Migration Examples

### 1. EdgeCommons Construction

**Old (Deprecated):**
```java
// Basic construction
EdgeCommons edgeCommons = new EdgeCommons("com.example.MyComponent", args);

// With app options
EdgeCommons edgeCommons = new EdgeCommons("com.example.MyComponent", args, appOptions);

// With all parameters
EdgeCommons edgeCommons = new EdgeCommons("com.example.MyComponent", args, appOptions, false);
```

**New (Recommended):**
```java
// Basic construction
EdgeCommons edgeCommons = EdgeCommonsBuilder.create("com.example.MyComponent")
    .withArgs(args)
    .build();

// With app options
EdgeCommons edgeCommons = EdgeCommonsBuilder.create("com.example.MyComponent")
    .withArgs(args)
    .withAppOptions(appOptions)
    .build();

// With all parameters
EdgeCommons edgeCommons = EdgeCommonsBuilder.create("com.example.MyComponent")
    .withArgs(args)
    .withAppOptions(appOptions)
    .receiveOwnMessages(false)
    .build();
```

### 2. Message Construction

**Old (Deprecated):**
```java
// Basic message from config
Message message = Message.buildFromConfig("heartbeat", "1.0", payload, configManager);

// With correlation ID
Message message = Message.buildFromConfig("heartbeat", "1.0", payload, configManager, correlationId);

// From generic object
Message message = Message.build(jsonObject);
```

**New (Recommended):**
```java
// Basic message from config
Message message = MessageBuilder.create("heartbeat", "1.0")
    .withPayload(payload)
    .withConfig(configManager)
    .build();

// With correlation ID
Message message = MessageBuilder.create("heartbeat", "1.0")
    .withPayload(payload)
    .withConfig(configManager)
    .withCorrelationId(correlationId)
    .build();

// From generic object
Message message = MessageBuilder.fromObject(jsonObject);
```

### 3. Metric Construction

**Old (Deprecated):**
```java
// Basic metric
Metric metric = new Metric("cpu_usage");

// With full configuration
Metric metric = new Metric("cpu_usage", "MyApp/Metrics", measures, dimensions);
```

**New (Recommended):**
```java
// Basic metric
Metric metric = MetricBuilder.create("cpu_usage")
    .build();

// With full configuration
Metric metric = MetricBuilder.create("cpu_usage")
    .withNamespace("MyApp/Metrics")
    .addMeasure("usage", "Percent", 1)
    .addDimension("instance", "main")
    .build();
```

## Benefits of Builder Pattern

1. **Fluent API**: More readable method chaining
2. **Optional Parameters**: Only specify what you need
3. **Validation**: Builders validate required parameters
4. **Extensibility**: Easy to add new parameters without breaking changes
5. **Immutability**: Objects are fully constructed before use

## Migration Timeline

- **Deprecated methods remain functional** for backward compatibility
- **No immediate action required** - existing code will continue to work
- **Recommended to migrate** new code to use builders
- **Consider migrating existing code** during maintenance cycles

## IDE Support

Most modern IDEs will show deprecation warnings for the old methods and suggest the builder alternatives. Enable deprecation warnings to identify code that should be migrated.