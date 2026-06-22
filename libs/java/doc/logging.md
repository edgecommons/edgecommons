# Logging System Documentation

## 1. Overview

The logging system in GGCommons Java library provides comprehensive logging capabilities built on Apache Log4j2. It supports both component-specific logging and global application logging control, with configurable output formats, levels, and destinations. The system is designed to provide:

- Centralized logging configuration management
- Dynamic log level adjustment
- Multiple output destinations (console, file)
- Template-based configuration with variable substitution
- Integration with component lifecycle management

## 2. Behavior

The logging system operates in two modes:

### Standard Mode (Default)
- Uses existing Log4j2 configuration (typically log4j2.xml)
- Applies component-specific logging settings
- Maintains compatibility with external logging configurations

### Global Control Mode
- Takes complete control of the logging system
- Replaces entire Log4j2 configuration dynamically
- Provides centralized management of all loggers
- Enables runtime reconfiguration

## 3. Configuration

The logging system is configured through the `logging` section of the component configuration.

### Basic Configuration Options

- **`level`**: Root logging level (Default: "INFO")
  - Valid values: "TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"
- **`format`**: Log message pattern (Default: "%d{yyyy-MM-dd HH:mm:ss.SSS} [%-5p] %C{1} (%L) [%t] : %m%n")
- **`globalControl`**: Enable global logging control (Default: false)

### File Logging Options

- **`fileLogging`**: Enable file-based logging (Default: false)
- **`logFilePath`**: Path to log file (supports template variables)

### Logger-Specific Configuration

- **`loggers`**: Map of logger names to specific log levels
- Allows fine-grained control over individual logger output

### Template Variables

The following template variables are supported in configuration strings:
- `{ComponentName}`: Short component name
- `{ComponentFullName}`: Full component name including version
- `{ThingName}`: AWS IoT Thing name
- `{TagName}`: Any configured tag value (e.g., `{site}`, `{shop}`)

## 4. Sample Configurations

### Sample 1: Basic Console Logging
```json
{
  "logging": {
    "level": "INFO",
    "format": "%d{HH:mm:ss.SSS} [%level] %logger{36} - %msg%n"
  }
}
```
This configuration provides basic console logging with INFO level and a simplified format.

### Sample 2: File and Console Logging
```json
{
  "logging": {
    "level": "DEBUG",
    "format": "%d{yyyy-MM-dd HH:mm:ss.SSS} [%-5p] %C{1} (%L) [%t] : %m%n",
    "fileLogging": true,
    "logFilePath": "/var/log/{ComponentName}-{ThingName}.log"
  }
}
```
This configuration enables both console and file logging with DEBUG level, using template variables in the file path.

### Sample 3: Global Control with Logger-Specific Levels
```json
{
  "logging": {
    "level": "WARN",
    "format": "%d{ISO8601} [%level] %logger - %msg%n",
    "globalControl": true,
    "fileLogging": true,
    "logFilePath": "/greengrass/v2/logs/{ComponentFullName}.log",
    "loggers": {
      "com.aws.proserve.ggcommons.metrics": "DEBUG",
      "com.aws.proserve.ggcommons.messaging": "INFO",
      "com.aws.proserve.ggcommons.heartbeat": "WARN"
    }
  }
}
```
This configuration demonstrates global control with different log levels for specific packages.

### Sample 4: Development Configuration
```json
{
  "logging": {
    "level": "TRACE",
    "format": "%d{HH:mm:ss.SSS} [%t] %-5level %logger{36} - %msg%n",
    "globalControl": true,
    "fileLogging": true,
    "logFilePath": "./logs/debug-{ComponentName}.log",
    "loggers": {
      "root": "DEBUG",
      "com.aws.proserve.ggcommons": "TRACE",
      "software.amazon.awssdk": "WARN",
      "org.eclipse.paho": "ERROR"
    }
  }
}
```
This configuration is optimized for development with detailed logging for ggcommons components while suppressing verbose output from third-party libraries.

### Sample 5: Production Configuration
```json
{
  "logging": {
    "level": "INFO",
    "format": "%d{yyyy-MM-dd HH:mm:ss.SSS} [%-5p] %C{1} [%t] : %m%n",
    "globalControl": false,
    "fileLogging": true,
    "logFilePath": "/greengrass/v2/logs/{ComponentName}.log",
    "loggers": {
      "com.aws.proserve.ggcommons.metrics": "WARN",
      "com.aws.proserve.ggcommons.messaging": "INFO",
      "com.aws.proserve.ggcommons.config": "WARN"
    }
  }
}
```
This production configuration focuses on essential logging while reducing verbosity for routine operations.

## 5. Integration with Other Systems

### Metric Logging
The logging system integrates with the metric emission system to provide file-based metric logging with rotation support. When using the "log" metric target, metrics are written to separate log files with EMF (Embedded Metric Format) formatting.

### Configuration Changes
The logging system responds to configuration changes through the `LoggingConfigChangeListener`, allowing dynamic reconfiguration without component restart.

### Template Resolution
Log file paths and other configuration strings support the same template variable system used throughout ggcommons, enabling consistent naming patterns across all components.

## 6. Best Practices

### Log Levels
- **TRACE**: Detailed diagnostic information for troubleshooting
- **DEBUG**: General debugging information
- **INFO**: Informational messages about normal operation
- **WARN**: Warning messages about potential issues
- **ERROR**: Error conditions that don't stop the application
- **FATAL**: Critical errors that may cause application termination

### Performance Considerations
- Use appropriate log levels to avoid performance impact
- Consider file logging impact on disk I/O
- Global control mode has higher overhead than standard mode
- Logger-specific levels can help reduce log volume

### File Management
- Use template variables for consistent file naming
- Consider log rotation for long-running components
- Ensure adequate disk space for log files
- Use appropriate file permissions for security

### Development vs Production
- Use higher verbosity (DEBUG/TRACE) in development
- Reduce to INFO/WARN in production
- Enable file logging in production for troubleshooting
- Consider centralized log collection in distributed deployments

## 7. Troubleshooting

### Common Issues
- **Logs not appearing**: Check log level configuration
- **File logging not working**: Verify file path permissions and disk space
- **Performance issues**: Reduce log level or disable verbose loggers
- **Configuration not applied**: Ensure globalControl is enabled for full control

### Debugging Configuration
- Enable DEBUG level for `com.aws.proserve.ggcommons.config` to see configuration loading
- Check for configuration validation errors in startup logs
- Verify template variable resolution in resolved file paths