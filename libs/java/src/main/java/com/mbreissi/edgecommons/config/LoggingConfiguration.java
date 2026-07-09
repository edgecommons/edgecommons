/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.Level;

import java.util.Collections;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Configuration class for managing logging settings in Greengrass components.
 * Handles log level, format, output locations, and other logging-related settings.
 * Supports dynamic reconfiguration of log levels and formats.
 */
public class LoggingConfiguration
{
    // Default logging configuration values
    static String DEFAULT_LEVEL = "INFO";
    static String DEFAULT_FORMAT = "%d{yyyy-MM-dd HH:mm:ss} [%-5p] %-25.25c{1}(%4L): %m%n";
    
    // Basic logging properties
    private String level = DEFAULT_LEVEL;
    private String format = DEFAULT_FORMAT;
    // Whether `logging.java_format` was explicitly present in the config (vs. the library default).
    // Drives the logging-format precedence (FR-RT-3): an explicit config value wins over the
    // platform-profile default (e.g. `json` on KUBERNETES); see ConfigManager#reconfigureLogging.
    private boolean formatExplicit = false;
    
    // File logging properties
    static String DEFAULT_MAX_FILE_SIZE = "10MB";
    static int DEFAULT_BACKUP_COUNT = 5;

    private boolean fileLoggingEnabled = false;
    private String logFilePath = null;
    private String maxFileSize = DEFAULT_MAX_FILE_SIZE;
    private int backupCount = DEFAULT_BACKUP_COUNT;
    
    // Logger-specific level configuration
    private Map<String, Level> loggerLevels = new HashMap<>();
    
    // Global control flag
    private boolean globalControl = false;

    private LogPublishConfiguration publishConfig = new LogPublishConfiguration(null);

    /**
     * Creates a new logging configuration from a JSON configuration object.
     * Parses and stores various logging settings including level, format,
     * file logging options, and logger-specific levels.
     *
     * @param jsonConfig The JSON object containing logging settings
     */
    public LoggingConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            // Parse basic logging properties
            if (jsonConfig.has("level"))
                level = jsonConfig.get("level").getAsString();
            // Per-language format key (replaces the former language-agnostic `format`).
            if (jsonConfig.has("java_format")) {
                format = jsonConfig.get("java_format").getAsString();
                formatExplicit = true;
            }
                
            // Parse file logging configuration if present
            if (jsonConfig.has("fileLogging") && jsonConfig.get("fileLogging").isJsonObject()) {
                JsonObject fileConfig = jsonConfig.get("fileLogging").getAsJsonObject();
                
                if (fileConfig.has("enabled"))
                    fileLoggingEnabled = fileConfig.get("enabled").getAsBoolean();

                if (fileConfig.has("filePath"))
                    logFilePath = fileConfig.get("filePath").getAsString();

                if (fileConfig.has("maxFileSize"))
                    maxFileSize = fileConfig.get("maxFileSize").getAsString();

                if (fileConfig.has("backupCount"))
                    backupCount = fileConfig.get("backupCount").getAsInt();
            }
            
            // Parse logger-specific levels if provided
            if (jsonConfig.has("loggers") && jsonConfig.get("loggers").isJsonObject()) {
                JsonObject loggers = jsonConfig.get("loggers").getAsJsonObject();
                for (Map.Entry<String, JsonElement> entry : loggers.entrySet()) {
                    String loggerName = entry.getKey();
                    String levelName = entry.getValue().getAsString().toUpperCase();
                    loggerLevels.put(loggerName, Level.getLevel(levelName));
                }
            }
            
            // Parse global control flag
            if (jsonConfig.has("globalControl"))
                globalControl = jsonConfig.get("globalControl").getAsBoolean();

            if (jsonConfig.has("publish") && jsonConfig.get("publish").isJsonObject())
                publishConfig = new LogPublishConfiguration(jsonConfig.getAsJsonObject("publish"));
        }
    }

    /**
     * Converts the logging configuration to a JSON object.
     *
     * @return JsonObject representation of the logging configuration
     */
    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        retVal.addProperty("level", level);
        retVal.addProperty("java_format", format);
        
        // Add file logging configuration if enabled
        if (fileLoggingEnabled) {
            JsonObject fileConfig = new JsonObject();
            fileConfig.addProperty("enabled", fileLoggingEnabled);
            if (logFilePath != null) {
                fileConfig.addProperty("filePath", logFilePath);
            }
            fileConfig.addProperty("maxFileSize", maxFileSize);
            fileConfig.addProperty("backupCount", backupCount);
            retVal.add("fileLogging", fileConfig);
        }
        
        // Add logger-specific levels if any are defined
        if (!loggerLevels.isEmpty()) {
            JsonObject loggersConfig = new JsonObject();
            for (Map.Entry<String, Level> entry : loggerLevels.entrySet()) {
                loggersConfig.addProperty(entry.getKey(), entry.getValue().toString());
            }
            retVal.add("loggers", loggersConfig);
        }
        
        // Add global control flag if enabled
        if (globalControl) {
            retVal.addProperty("globalControl", globalControl);
        }

        retVal.add("publish", publishConfig.toDict());
        
        return retVal;
    }

    @Override
    public String toString()
    {
        Gson gson = new Gson();
        return gson.toJson(toDict(), JsonObject.class);
    }

    /**
     * Gets the root logger level.
     *
     * @return The Log4j2 Level object for the root logger
     */
    public Level getLevel()
    {
        return Level.toLevel(level, Level.TRACE);
    }

    /**
     * Gets the log message format pattern.
     *
     * @return The pattern string for log formatting
     */
    public String getFormat()
    {
        return format;
    }

    /**
     * Whether {@code logging.java_format} was explicitly set in the component configuration (as
     * opposed to falling back to the library default {@link #DEFAULT_FORMAT}). The logging-format
     * precedence (FR-RT-3) gives an explicit config value priority over the platform-profile default
     * (e.g. the {@code json} stdout-JSON sink defaulted on KUBERNETES).
     *
     * @return {@code true} if the config supplied an explicit {@code java_format}
     */
    public boolean isFormatExplicitlySet()
    {
        return formatExplicit;
    }

    /**
     * Checks if file logging is enabled.
     *
     * @return true if file logging is enabled, false otherwise
     */
    public boolean isFileLoggingEnabled() {
        return fileLoggingEnabled;
    }
    
    /**
     * Gets the file path for log output when file logging is enabled.
     *
     * @return The path to the log file or null if not configured
     */
    public String getLogFilePath() {
        return logFilePath;
    }

    /**
     * Gets the maximum size a log file may reach before it is rotated.
     *
     * @return A size string such as {@code 10MB} (accepts KB/MB/GB suffixes)
     */
    public String getMaxFileSize() {
        return maxFileSize;
    }

    /**
     * Gets the number of rotated backup files to retain.
     *
     * @return The backup count (0 keeps no backups)
     */
    public int getBackupCount() {
        return backupCount;
    }

    /**
     * Gets the map of logger-specific level configurations.
     *
     * @return An unmodifiable map of logger names to their configured levels
     */
    public Map<String, Level> getLoggerLevels() {
        return Collections.unmodifiableMap(loggerLevels);
    }
    
    /**
     * Checks if global logging control is enabled.
     *
     * @return true if EdgeCommons should control all application logging
     */
    public boolean isGlobalControlEnabled() {
        return globalControl;
    }

    /** Returns the log bus publish configuration. */
    public LogPublishConfiguration getPublishConfig() {
        return publishConfig;
    }

    /** Configuration for {@code logging.publish}. */
    public static final class LogPublishConfiguration {
        public enum Destination {
            LOCAL,
            NORTHBOUND;

            static Destination parse(String value) {
                if (value == null) return LOCAL;
                return switch (value.trim().toLowerCase()) {
                    case "local" -> LOCAL;
                    case "northbound" -> NORTHBOUND;
                    default -> throw new IllegalArgumentException(
                            "logging.publish.destination must be local or northbound");
                };
            }

            String wire() {
                return name().toLowerCase();
            }
        }

        public enum QueueOnFull {
            DROP_OLDEST;

            static QueueOnFull parse(String value) {
                if (value == null) return DROP_OLDEST;
                if ("dropOldest".equals(value) || "dropoldest".equals(value.toLowerCase())) {
                    return DROP_OLDEST;
                }
                throw new IllegalArgumentException(
                        "logging.publish.queue.onFull must be dropOldest");
            }

            String wire() {
                return "dropOldest";
            }
        }

        private boolean enabled = false;
        private Destination destination = Destination.LOCAL;
        private String minLevel = "INFO";
        private boolean captureNative = true;
        private boolean captureConsole = false;
        private int maxRecordBytes = 8192;
        private int queueMaxRecords = 1000;
        private QueueOnFull queueOnFull = QueueOnFull.DROP_OLDEST;
        private boolean redactionEnabled = true;
        private String redactionReplacement = "***";
        private List<String> redactionExtraPatterns = new ArrayList<>();

        LogPublishConfiguration(JsonObject jsonConfig) {
            if (jsonConfig == null) {
                return;
            }
            if (jsonConfig.has("enabled")) enabled = jsonConfig.get("enabled").getAsBoolean();
            if (jsonConfig.has("destination")) {
                destination = Destination.parse(jsonConfig.get("destination").getAsString());
            }
            if (jsonConfig.has("minLevel")) {
                String parsed = jsonConfig.get("minLevel").getAsString().toUpperCase();
                // Validate against the log bus level set while keeping this config package
                // independent from the public logging API enum.
                switch (parsed) {
                    case "TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL" -> minLevel = parsed;
                    default -> throw new IllegalArgumentException(
                            "logging.publish.minLevel must be TRACE, DEBUG, INFO, WARN, ERROR or FATAL");
                }
            }
            if (jsonConfig.has("captureNative")) {
                captureNative = jsonConfig.get("captureNative").getAsBoolean();
            }
            if (jsonConfig.has("captureConsole")) {
                captureConsole = jsonConfig.get("captureConsole").getAsBoolean();
            }
            if (jsonConfig.has("maxRecordBytes")) {
                maxRecordBytes = positiveInt(jsonConfig.get("maxRecordBytes"), "maxRecordBytes");
            }
            if (jsonConfig.has("queue") && jsonConfig.get("queue").isJsonObject()) {
                JsonObject queue = jsonConfig.getAsJsonObject("queue");
                if (queue.has("maxRecords")) {
                    queueMaxRecords = positiveInt(queue.get("maxRecords"), "queue.maxRecords");
                }
                if (queue.has("onFull")) {
                    queueOnFull = QueueOnFull.parse(queue.get("onFull").getAsString());
                }
            }
            if (jsonConfig.has("redaction") && jsonConfig.get("redaction").isJsonObject()) {
                JsonObject redaction = jsonConfig.getAsJsonObject("redaction");
                if (redaction.has("enabled")) {
                    redactionEnabled = redaction.get("enabled").getAsBoolean();
                }
                if (redaction.has("replacement")) {
                    redactionReplacement = redaction.get("replacement").getAsString();
                }
                if (redaction.has("extraPatterns") && redaction.get("extraPatterns").isJsonArray()) {
                    redactionExtraPatterns = new ArrayList<>();
                    for (JsonElement element : redaction.getAsJsonArray("extraPatterns")) {
                        redactionExtraPatterns.add(element.getAsString());
                    }
                }
            }
        }

        private static int positiveInt(JsonElement element, String field) {
            int value = element.getAsInt();
            if (value <= 0) {
                throw new IllegalArgumentException("logging.publish." + field + " must be positive");
            }
            return value;
        }

        JsonObject toDict() {
            JsonObject retVal = new JsonObject();
            retVal.addProperty("enabled", enabled);
            retVal.addProperty("destination", destination.wire());
            retVal.addProperty("minLevel", minLevel);
            retVal.addProperty("captureNative", captureNative);
            retVal.addProperty("captureConsole", captureConsole);
            retVal.addProperty("maxRecordBytes", maxRecordBytes);
            JsonObject queue = new JsonObject();
            queue.addProperty("maxRecords", queueMaxRecords);
            queue.addProperty("onFull", queueOnFull.wire());
            retVal.add("queue", queue);
            JsonObject redaction = new JsonObject();
            redaction.addProperty("enabled", redactionEnabled);
            redaction.addProperty("replacement", redactionReplacement);
            com.google.gson.JsonArray patterns = new com.google.gson.JsonArray();
            for (String pattern : redactionExtraPatterns) {
                patterns.add(pattern);
            }
            redaction.add("extraPatterns", patterns);
            retVal.add("redaction", redaction);
            return retVal;
        }

        public boolean isEnabled() { return enabled; }
        public Destination getDestination() { return destination; }
        public String getMinLevel() { return minLevel; }
        public boolean isCaptureNative() { return captureNative; }
        public boolean isCaptureConsole() { return captureConsole; }
        public int getMaxRecordBytes() { return maxRecordBytes; }
        public int getQueueMaxRecords() { return queueMaxRecords; }
        public QueueOnFull getQueueOnFull() { return queueOnFull; }
        public boolean isRedactionEnabled() { return redactionEnabled; }
        public String getRedactionReplacement() { return redactionReplacement; }
        public List<String> getRedactionExtraPatterns() {
            return Collections.unmodifiableList(redactionExtraPatterns);
        }
    }
}
