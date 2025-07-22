# Dynamic Logging Configuration in GGCommons

This document explains how to use the enhanced logging configuration capabilities in the GGCommons library.

## Overview

The GGCommons library now supports dynamic logging configuration through the configuration system. This allows you to:

1. Change log levels at runtime
2. Customize log message formats
3. Configure file-based logging
4. Set different log levels for specific packages or classes

## Configuration Format

The logging configuration is specified in the `logging` section of your component's configuration:

```json
{
  "logging": {
    "level": "INFO",                 // Root logger level (TRACE, DEBUG, INFO, WARN, ERROR, FATAL)
    "format": "%d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %-5level %logger{36} - %msg%n",  // Log4j2 pattern layout
    "fileLogging": {                 // Optional file logging configuration
      "enabled": true,               // Enable/disable file logging
      "filePath": "/var/log/{ComponentName}-{ThingName}.log"  // Log file path (supports templates)
    },
    "loggers": {                     // Optional logger-specific levels
      "com.aws.proserve.ggcommons": "DEBUG",
      "com.aws.proserve.ggcommons.messaging": "TRACE",
      "org.apache.http": "WARN"
    }
  }
}
```

## Template Variables

The `filePath` property supports template variables that are replaced at runtime:

- `{ThingName}`: The AWS IoT thing name
- `{ComponentName}`: The short name of the component
- `{ComponentFullName}`: The fully qualified component name
- Any tag defined in the `tags` section of the configuration

## Log Levels

The following log levels are supported (in order of increasing severity):

1. `TRACE` - Most detailed logging
2. `DEBUG` - Debugging information
3. `INFO` - General information
4. `WARN` - Warning messages
5. `ERROR` - Error messages
6. `FATAL` - Critical errors

## Pattern Layout

The `format` property uses Log4j2's PatternLayout syntax. Some common pattern elements:

- `%d{pattern}` - Date/time (e.g., `%d{yyyy-MM-dd HH:mm:ss.SSS}`)
- `%t` - Thread name
- `%-5level` - Log level, left-justified, minimum 5 characters
- `%logger{length}` - Logger name, truncated to specified length
- `%msg` - Log message
- `%n` - Platform-specific line separator
- `%X{key}` - MDC (Mapped Diagnostic Context) value

## Dynamic Reconfiguration

When the configuration changes (e.g., through Greengrass deployment), the logging system is automatically reconfigured. This allows you to change log levels and other settings without restarting the component.

## Implementation Details

The dynamic logging configuration is implemented in the `ConfigManager.reconfigureLogging()` method, which:

1. Creates a new Log4j2 configuration programmatically
2. Sets up console and optional file appenders
3. Configures the root logger with the specified level
4. Configures individual loggers with their specific levels
5. Applies the new configuration to the LoggerContext

## Example Usage

```java
// Get the current log level
Level currentLevel = configManager.getLoggingConfig().getLevel();

// Check if a specific logger has a custom level
Map<String, Level> loggerLevels = configManager.getLoggingConfig().getLoggerLevels();
if (loggerLevels.containsKey("com.aws.proserve.ggcommons")) {
    Level customLevel = loggerLevels.get("com.aws.proserve.ggcommons");
    // Use the custom level...
}

// Check if file logging is enabled
if (configManager.getLoggingConfig().isFileLoggingEnabled()) {
    String logFilePath = configManager.getLoggingConfig().getLogFilePath();
    // Use the log file path...
}
```

## Troubleshooting

If logging reconfiguration fails, the error is logged and the previous configuration is maintained. Check the logs for messages like:

```
ERROR Failed to reconfigure logging: <error message>
WARN Continuing with previous logging configuration
```

Common issues include:
- Invalid log level names
- Invalid pattern layout syntax
- Permission issues when writing to log files
- Missing parent directories for log files
