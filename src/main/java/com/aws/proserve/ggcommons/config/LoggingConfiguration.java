/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.Level;

import java.util.Collections;
import java.util.HashMap;
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
    
    // File logging properties
    private boolean fileLoggingEnabled = false;
    private String logFilePath = null;
    
    // Logger-specific level configuration
    private Map<String, Level> loggerLevels = new HashMap<>();
    
    // Global control flag
    private boolean globalControl = false;

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
            if (jsonConfig.has("format"))
                format = jsonConfig.get("format").getAsString();
                
            // Parse file logging configuration if present
            if (jsonConfig.has("fileLogging") && jsonConfig.get("fileLogging").isJsonObject()) {
                JsonObject fileConfig = jsonConfig.get("fileLogging").getAsJsonObject();
                
                if (fileConfig.has("enabled"))
                    fileLoggingEnabled = fileConfig.get("enabled").getAsBoolean();
                    
                if (fileConfig.has("filePath"))
                    logFilePath = fileConfig.get("filePath").getAsString();
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
        retVal.addProperty("format", format);
        
        // Add file logging configuration if enabled
        if (fileLoggingEnabled) {
            JsonObject fileConfig = new JsonObject();
            fileConfig.addProperty("enabled", fileLoggingEnabled);
            if (logFilePath != null) {
                fileConfig.addProperty("filePath", logFilePath);
            }
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
     * @return true if GGCommons should control all application logging
     */
    public boolean isGlobalControlEnabled() {
        return globalControl;
    }
}
