/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Configuration change listener specifically for logging configuration changes.
 * This listener is responsible for applying logging configuration changes when they occur.
 */
public class LoggingConfigChangeListener implements ConfigurationChangeListener {
    private static final Logger LOGGER = LogManager.getLogger(LoggingConfigChangeListener.class);
    
    private final ConfigManager configManager;
    
    /**
     * Creates a new logging configuration change listener.
     *
     * @param configManager The configuration manager to use for reconfiguring logging
     */
    public LoggingConfigChangeListener(ConfigManager configManager) {
        this.configManager = configManager;
    }
    
    /**
     * Handles configuration changes by reconfiguring the logging system.
     * 
     * @return true if the configuration change was handled successfully
     */
    @Override
    public boolean onConfigurationChanged() {
        LOGGER.info("Logging configuration changed, applying new settings");
        configManager.reconfigureLogging();
        return true;
    }
}
