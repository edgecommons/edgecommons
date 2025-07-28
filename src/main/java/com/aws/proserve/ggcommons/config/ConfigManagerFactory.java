/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.aws.proserve.ggcommons.ParsedCommandLine;
import com.aws.proserve.ggcommons.config.provider.ConfigProvider;
import com.aws.proserve.ggcommons.config.provider.ConfigProviderBuilder;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Factory for creating ConfigManager instances with proper validation and error handling.
 */
public class ConfigManagerFactory {
    private static final Logger LOGGER = LogManager.getLogger(ConfigManagerFactory.class);
    
    /**
     * Creates a ConfigManager instance for the specified component.
     *
     * @param componentName The name of the Greengrass component
     * @param cmdLine Parsed command line arguments containing configuration options
     * @return Configured ConfigManager instance
     * @throws ConfigurationException if configuration loading or validation fails
     */
    public static ConfigManager create(String componentName, ParsedCommandLine cmdLine) throws ConfigurationException {
        try {
            // Parse component name
            String componentFullName = componentName;
            String componentShortName = componentName.contains(".") 
                ? componentName.substring(componentName.lastIndexOf(".") + 1)
                : componentName;
            
            // Determine thing name
            String thingName = resolveThingName(cmdLine);
            
            // Load configuration
            ConfigProvider configProvider = ConfigProviderBuilder.build(null, componentName, thingName, cmdLine.configArgs);
            JsonObject fullConfig = configProvider.loadConfiguration();
            
            if (fullConfig == null) {
                throw new ConfigurationException("No configuration found");
            }
            
            // Validate configuration
            validateConfiguration(fullConfig);
            
            // Create ConfigManager instance
            ConfigManager configManager = new ConfigManager(componentFullName, componentShortName, thingName, 
                                                           configProvider, fullConfig);
            
            LOGGER.info("Configuration loaded from {}", configProvider.getConfigSource());
            return configManager;
            
        } catch (ConfigurationException e) {
            throw e;
        } catch (Exception e) {
            throw new ConfigurationException("Failed to create ConfigManager: " + e.getMessage(), e);
        }
    }
    
    /**
     * Resolves the thing name from command line or environment.
     */
    private static String resolveThingName(ParsedCommandLine cmdLine) {
        if (cmdLine.thingName != null) {
            return cmdLine.thingName;
        }
        if (System.getenv("AWS_IOT_THING_NAME") != null) {
            return System.getenv("AWS_IOT_THING_NAME");
        }
        return "NOT_GREENGRASS";
    }
    
    /**
     * Validates configuration against schema.
     */
    private static void validateConfiguration(JsonObject config) throws ConfigurationException {
        try {
            ConfigurationValidator.validate(config);
        } catch (ConfigurationValidator.ConfigurationValidationException e) {
            throw new ConfigurationException("Configuration validation failed: " + e.getMessage(), e);
        }
    }
}