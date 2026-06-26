/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.builder.api.*;
import org.apache.logging.log4j.core.config.builder.impl.BuiltConfiguration;

import java.util.Map;

import static org.apache.logging.log4j.core.config.builder.api.ConfigurationBuilderFactory.newConfigurationBuilder;

/**
 * Manages logging configuration in a way that minimizes conflicts with embedding applications.
 * Uses isolated logger contexts and namespace prefixing to avoid interference.
 */
public class LoggingConfigurationManager {
    
    private static final String GGCOMMONS_LOGGER_PREFIX = "com.mbreissi.ggcommons";
    private final String componentName;
    private final ConfigManager configManager;
    
    public LoggingConfigurationManager(String componentName, ConfigManager configManager) {
        this.componentName = componentName;
        this.configManager = configManager;
    }
    
    /**
     * Configures logging in a way that minimizes impact on the embedding application.
     * Only configures loggers under the GGCommons namespace.
     */
    public void configureLogging() {
        try {
            LoggingConfiguration loggingConfig = configManager.getLoggingConfig();
            
            // Only configure GGCommons-specific loggers to avoid conflicts
            configureGGCommonsLoggers(loggingConfig);
            
        } catch (Exception e) {
            // Fallback to system logging if configuration fails
            System.err.println("Failed to configure GGCommons logging: " + e.getMessage());
        }
    }
    
    private void configureGGCommonsLoggers(LoggingConfiguration loggingConfig) {
        LoggerContext context = (LoggerContext) LogManager.getContext(false);
        Configuration config = context.getConfiguration();
        
        // Configure the main GGCommons logger
        org.apache.logging.log4j.core.config.LoggerConfig loggerConfig = 
            config.getLoggerConfig(GGCOMMONS_LOGGER_PREFIX);
        
        if (loggerConfig.getName().equals(GGCOMMONS_LOGGER_PREFIX)) {
            // Logger already exists, update its level
            loggerConfig.setLevel(loggingConfig.getLevel());
        } else {
            // Create new logger configuration for GGCommons
            org.apache.logging.log4j.core.config.LoggerConfig newLoggerConfig = 
                org.apache.logging.log4j.core.config.LoggerConfig.createLogger(
                    false, // additivity
                    loggingConfig.getLevel(),
                    GGCOMMONS_LOGGER_PREFIX,
                    "true",
                    new org.apache.logging.log4j.core.config.AppenderRef[0],
                    null,
                    config,
                    null
                );
            
            config.addLogger(GGCOMMONS_LOGGER_PREFIX, newLoggerConfig);
        }
        
        // Configure specific logger levels if defined
        Map<String, Level> loggerLevels = loggingConfig.getLoggerLevels();
        for (Map.Entry<String, Level> entry : loggerLevels.entrySet()) {
            String loggerName = entry.getKey();
            Level level = entry.getValue();
            
            // Only configure loggers under GGCommons namespace to avoid conflicts
            if (loggerName.startsWith(GGCOMMONS_LOGGER_PREFIX)) {
                org.apache.logging.log4j.core.config.LoggerConfig specificLogger = 
                    config.getLoggerConfig(loggerName);
                
                if (specificLogger.getName().equals(loggerName)) {
                    specificLogger.setLevel(level);
                } else {
                    org.apache.logging.log4j.core.config.LoggerConfig newSpecificLogger = 
                        org.apache.logging.log4j.core.config.LoggerConfig.createLogger(
                            false,
                            level,
                            loggerName,
                            "true",
                            new org.apache.logging.log4j.core.config.AppenderRef[0],
                            null,
                            config,
                            null
                        );
                    
                    config.addLogger(loggerName, newSpecificLogger);
                }
            }
        }
        
        context.updateLoggers();
    }
}