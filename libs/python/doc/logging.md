# Logging System Documentation

## 1. Overview

The logging system in ggcommons-python-lib provides comprehensive logging capabilities built on Python's standard logging framework. It supports both component-specific logging and global application logging control, with configurable output formats, levels, and destinations. The system is designed to provide:

- Centralized logging configuration management
- Dynamic log level adjustment
- Multiple output destinations (console, file)
- Template-based configuration with variable substitution
- Integration with component lifecycle management

## 2. Behavior

The logging system operates by configuring Python's standard logging framework based on the component configuration.

### Standard Operation
- Uses Python's logging framework with configurable handlers
- Applies component-specific logging settings
- Supports both console and file logging
- Maintains compatibility with existing logging configurations

### File Logging
- Optional file-based logging with rotation support
- Template variable substitution in file paths
- Configurable file size limits and backup counts

## 3. Configuration

The logging system is configured through the `logging` section of the component configuration.

### Basic Configuration Options

- **`level`**: Root logging level (Default: "INFO")
  - Valid values: "TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"
- **`format`**: Log message pattern (Default: "%(asctime)s [%(levelname)s] %(name)s: %(message)s")

### File Logging Options

- **`fileLogging`**: File logging configuration object
  - **`enabled`**: Enable file-based logging (Default: false)
  - **`filePath`**: Path to log file (supports template variables)
  - **`maxFileSize`**: Maximum file size before rotation (Default: "10MB")
  - **`backupCount`**: Number of backup files to keep (Default: 5)

### Logger-Specific Configuration

- **`loggers`**: Dictionary of logger names to specific log levels
- Allows fine-grained control over individual logger output

### Template Variables

The following template variables are supported in configuration strings:
- `{ComponentName}`: Short component name
- `{ComponentFullName}`: Full component name including version
- `{ThingName}`: AWS IoT Thing name
- `{TagName}`: Any configured tag value (e.g., `{site}`, `{environment}`)

## 4. Sample Configurations

### Sample 1: Basic Console Logging
```json
{
  "logging": {
    "level": "INFO",
    "format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s"
  }
}
```
This configuration provides basic console logging with INFO level and a standard format.

### Sample 2: File and Console Logging
```json
{
  "logging": {
    "level": "DEBUG",
    "format": "%(asctime)s [%(levelname)s] %(name)s (%(filename)s:%(lineno)d): %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "/var/log/{ComponentName}-{ThingName}.log",
      "maxFileSize": "50MB",
      "backupCount": 10
    }
  }
}
```
This configuration enables both console and file logging with DEBUG level, using template variables in the file path.

### Sample 3: Logger-Specific Levels
```json
{
  "logging": {
    "level": "WARN",
    "format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "/greengrass/v2/logs/{ComponentFullName}.log"
    },
    "loggers": {
      "ggcommons.metrics": "DEBUG",
      "ggcommons.messaging": "INFO",
      "ggcommons.heartbeat": "WARN",
      "ggcommons.config": "INFO"
    }
  }
}
```
This configuration demonstrates different log levels for specific packages while maintaining a higher default level.

### Sample 4: Development Configuration
```json
{
  "logging": {
    "level": "DEBUG",
    "format": "%(asctime)s [%(threadName)s] %(levelname)-5s %(name)s - %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "./logs/debug-{ComponentName}.log",
      "maxFileSize": "100MB"
    },
    "loggers": {
      "root": "DEBUG",
      "ggcommons": "DEBUG",
      "boto3": "WARN",
      "botocore": "WARN",
      "paho": "ERROR"
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
    "format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    "fileLogging": {
      "enabled": true,
      "filePath": "/greengrass/v2/logs/{ComponentName}.log",
      "maxFileSize": "20MB",
      "backupCount": 5
    },
    "loggers": {
      "ggcommons.metrics": "WARN",
      "ggcommons.messaging": "INFO",
      "ggcommons.config": "WARN",
      "ggcommons.heartbeat": "INFO"
    }
  }
}
```
This production configuration focuses on essential logging while reducing verbosity for routine operations.

## 5. Integration with Other Systems

### Metric Logging
The logging system integrates with the metric emission system to provide file-based metric logging with rotation support. When using the "log" metric target, metrics are written to separate log files with EMF (Embedded Metric Format) formatting.

### Configuration Changes
The logging system can respond to configuration changes through the configuration change listener system, allowing dynamic reconfiguration without component restart.

### Template Resolution
Log file paths and other configuration strings support the same template variable system used throughout ggcommons, enabling consistent naming patterns across all components.

## 6. Usage in Code

### Basic Logging
```python
import logging

# Get logger for your module
logger = logging.getLogger(__name__)

# Log at different levels
logger.debug("Detailed debugging information")
logger.info("General information about program execution")
logger.warning("Warning about potential issues")
logger.error("Error occurred but program continues")
logger.critical("Critical error that may cause program termination")
```

### Structured Logging
```python
import logging

logger = logging.getLogger(__name__)

# Log with additional context
logger.info("Processing data", extra={
    'component': 'data_processor',
    'instance_id': 'main',
    'record_count': 150
})

# Log exceptions with stack trace
try:
    # Some operation
    pass
except Exception as e:
    logger.exception("Failed to process data: %s", str(e))
```

### Dynamic Log Level Changes
```python
import logging

# Change log level at runtime
logging.getLogger('ggcommons.messaging').setLevel(logging.DEBUG)
logging.getLogger('my_component').setLevel(logging.INFO)
```

## 7. Best Practices

### Log Levels
- **DEBUG**: Detailed diagnostic information for troubleshooting
- **INFO**: Informational messages about normal operation
- **WARNING**: Warning messages about potential issues
- **ERROR**: Error conditions that don't stop the application
- **CRITICAL**: Critical errors that may cause application termination

### Performance Considerations
- Use appropriate log levels to avoid performance impact
- Consider file logging impact on disk I/O
- Logger-specific levels can help reduce log volume
- Avoid expensive string formatting in debug messages

### File Management
- Use template variables for consistent file naming
- Configure appropriate file rotation sizes and backup counts
- Ensure adequate disk space for log files
- Use appropriate file permissions for security

### Development vs Production
- Use higher verbosity (DEBUG) in development
- Reduce to INFO/WARNING in production
- Enable file logging in production for troubleshooting
- Consider centralized log collection in distributed deployments

### Message Formatting
- Include relevant context in log messages
- Use consistent formatting across your application
- Avoid logging sensitive information (passwords, keys, etc.)
- Use structured logging for better searchability

## 8. File Rotation

The logging system supports automatic file rotation based on file size:

### Configuration
```json
{
  "logging": {
    "fileLogging": {
      "enabled": true,
      "filePath": "/var/log/mycomponent.log",
      "maxFileSize": "10MB",
      "backupCount": 5
    }
  }
}
```

### Behavior
- When the log file reaches `maxFileSize`, it's rotated
- The current file is renamed to `.1`, previous `.1` becomes `.2`, etc.
- Up to `backupCount` backup files are kept
- Oldest backup files are automatically deleted

### File Size Formats
Supported file size formats:
- `"10MB"` - 10 megabytes
- `"1GB"` - 1 gigabyte  
- `"500KB"` - 500 kilobytes
- `"1024"` - 1024 bytes (no suffix)

## 9. Troubleshooting

### Common Issues
- **Logs not appearing**: Check log level configuration and logger names
- **File logging not working**: Verify file path permissions and disk space
- **Performance issues**: Reduce log level or disable verbose loggers
- **File rotation not working**: Check file permissions and disk space

### Debug Configuration
Enable debug logging for the logging system itself:
```json
{
  "logging": {
    "level": "DEBUG",
    "loggers": {
      "ggcommons.config": "DEBUG"
    }
  }
}
```

### Verification
```python
import logging

# Check current log levels
logger = logging.getLogger('ggcommons.messaging')
print(f"Current level: {logger.level}")
print(f"Effective level: {logger.getEffectiveLevel()}")

# List all loggers
for name in logging.Logger.manager.loggerDict:
    logger = logging.getLogger(name)
    if logger.handlers or logger.level != logging.NOTSET:
        print(f"Logger: {name}, Level: {logger.level}")
```

### Log File Monitoring
- Monitor log file sizes and rotation
- Set up alerts for critical error messages
- Use log aggregation tools for distributed deployments
- Regularly clean up old log files to manage disk space

## 10. Integration Examples

### With Configuration Changes
```python
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
import logging

class LoggingConfigListener(ConfigurationChangeListener):
    def on_configuration_change(self, configuration):
        # Update logging configuration dynamically
        logging_config = configuration.get('logging', {})
        level = logging_config.get('level', 'INFO')
        
        # Update root logger level
        logging.getLogger().setLevel(getattr(logging, level.upper()))
        
        # Update specific loggers
        loggers_config = logging_config.get('loggers', {})
        for logger_name, logger_level in loggers_config.items():
            logging.getLogger(logger_name).setLevel(getattr(logging, logger_level.upper()))
        
        return True
```

### With Metrics
```python
import logging
from ggcommons.metrics.metric_emitter import MetricEmitter

logger = logging.getLogger(__name__)

def process_data(data):
    try:
        # Process data
        result = perform_processing(data)
        
        # Log success
        logger.info(f"Successfully processed {len(data)} records")
        
        # Emit success metric
        MetricEmitter.emit_metric("data_processing", {"success_count": len(data)})
        
        return result
        
    except Exception as e:
        # Log error with context
        logger.error(f"Failed to process data: {e}", exc_info=True)
        
        # Emit error metric
        MetricEmitter.emit_metric("data_processing", {"error_count": 1})
        
        raise
```