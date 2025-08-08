/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.interfaces;

import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.aws.proserve.ggcommons.config.LoggingConfiguration;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.config.TagConfiguration;
import com.google.gson.JsonObject;
import java.util.Collection;

/**
 * Interface for configuration management services.
 * Provides access to component configuration and change notifications.
 */
public interface IConfigurationService {
    
    /**
     * Returns the global configuration section shared across all instances.
     * 
     * @return JsonObject containing global configuration
     */
    JsonObject getGlobalConfig();
    
    /**
     * Returns configuration for a specific instance.
     * 
     * @param instanceId The instance identifier
     * @return JsonObject containing instance-specific configuration, or null if not found
     */
    JsonObject getInstanceConfig(String instanceId);
    
    /**
     * Returns collection of all configured instance IDs.
     * 
     * @return Collection of instance identifier strings
     */
    Collection<String> getInstanceIds();
    
    /**
     * Returns the complete configuration object.
     * 
     * @return JsonObject containing the full configuration
     */
    JsonObject getFullConfig();
    
    /**
     * Returns the AWS IoT Thing name.
     * 
     * @return The thing name or null if not available
     */
    String getThingName();
    
    /**
     * Returns the short component name.
     * 
     * @return The component name
     */
    String getComponentName();
    
    /**
     * Returns the fully qualified component name.
     * 
     * @return The fully qualified component name
     */
    String getComponentFullName();
    
    /**
     * Resolves template variables in a string.
     * 
     * @param template String containing template variables like {ThingName}
     * @return Resolved string with substituted values
     */
    String resolveTemplate(String template);
    
    /**
     * Registers a configuration change listener.
     * 
     * @param listener The listener to add
     */
    void addConfigChangeListener(ConfigurationChangeListener listener);
    
    /**
     * Removes a configuration change listener.
     * 
     * @param listener The listener to remove
     */
    void removeConfigChangeListener(ConfigurationChangeListener listener);
    
    /**
     * Manually triggers configuration change notifications.
     */
    void notifyConfigurationChanged();
    
    /**
     * Returns the heartbeat configuration settings.
     * 
     * @return HeartbeatConfiguration object containing heartbeat-related settings
     */
    HeartbeatConfiguration getHeartbeatConfig();
    
    /**
     * Returns the tag configuration settings.
     * 
     * @return TagConfiguration object containing tag-related settings
     */
    TagConfiguration getTagConfig();
    
    /**
     * Returns the logging configuration settings.
     * 
     * @return LoggingConfiguration object containing logging-related settings
     */
    LoggingConfiguration getLoggingConfig();
    
    /**
     * Returns the metric configuration settings.
     * 
     * @return MetricConfiguration object containing metric-related settings
     */
    MetricConfiguration getMetricConfig();
}